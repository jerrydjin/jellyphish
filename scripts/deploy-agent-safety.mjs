import crypto from "node:crypto";
import fs from "node:fs/promises";

const agentId = process.env.ELEVENLABS_AGENT_ID || "agent_1101m2b4acdnedz9y2yky3w9vwfm";
const envText = await fs.readFile(new URL("../.env", import.meta.url), "utf8").catch(() => "");
const fileEnv = Object.fromEntries(
  envText
    .split(/\r?\n/)
    .filter((line) => line && !line.startsWith("#") && line.includes("="))
    .map((line) => line.split(/=(.*)/s).slice(0, 2)),
);
const apiKey = process.env.ELEVENLABS_API_KEY || fileEnv.ELEVENLABS_API_KEY;
if (!apiKey) throw new Error("ELEVENLABS_API_KEY is required in the environment or .env");

const prompt = (await fs.readFile(new URL("../agent-prompt.md", import.meta.url), "utf8")).trim();
const source = JSON.parse(await fs.readFile(new URL("../elevenlabs-agent.json", import.meta.url), "utf8"));
const headers = { "content-type": "application/json", "xi-api-key": apiKey };
const endpoint = `https://api.elevenlabs.io/v1/convai/agents/${agentId}`;

const currentResponse = await fetch(endpoint, { headers });
if (!currentResponse.ok) throw new Error(`Reading agent failed (${currentResponse.status}): ${await currentResponse.text()}`);
const current = await currentResponse.json();

current.conversation_config.agent.prompt.prompt = prompt;
const sourceTools = source.conversation_config.agent.prompt.tools || [];
const currentTools = (current.conversation_config.agent.prompt.tools || []).filter(
  ({ name, type }) => type !== "client" || !["assess_call", "assess_call_browser"].includes(name),
);
for (const sourceTool of sourceTools) {
  const index = currentTools.findIndex(({ name }) => name === sourceTool.name);
  if (index >= 0) currentTools[index] = sourceTool;
  else currentTools.push(sourceTool);
}
current.conversation_config.agent.prompt.tools = currentTools;
// Agent reads may include both resolved tools and their IDs; updates accept only one representation.
delete current.conversation_config.agent.prompt.tool_ids;
const endCallDescription = source.conversation_config.agent.prompt.built_in_tools.end_call.description;
for (const tool of current.conversation_config.agent.prompt.tools || []) {
  if (tool.name === "end_call") tool.description = endCallDescription;
}
if (current.conversation_config.agent.prompt.built_in_tools?.end_call) {
  current.conversation_config.agent.prompt.built_in_tools.end_call.description = endCallDescription;
}

current.platform_settings.guardrails = {
  ...current.platform_settings.guardrails,
  ...source.platform_settings.guardrails,
};
if (!current.conversation_config.conversation) current.conversation_config.conversation = {};
current.conversation_config.conversation.client_events = source.conversation_config.conversation.client_events;

const updateResponse = await fetch(endpoint, {
  method: "PATCH",
  headers,
  body: JSON.stringify({
    conversation_config: current.conversation_config,
    platform_settings: current.platform_settings,
    version_description: "Red distraction stays on the line until the caller quits",
  }),
});
if (!updateResponse.ok) throw new Error(`Updating agent failed (${updateResponse.status}): ${await updateResponse.text()}`);

const verifiedResponse = await fetch(endpoint, { headers });
if (!verifiedResponse.ok) throw new Error(`Verifying agent failed (${verifiedResponse.status}): ${await verifiedResponse.text()}`);
const verified = await verifiedResponse.json();
const deployedPrompt = verified.conversation_config?.agent?.prompt?.prompt || "";
const guardrails = verified.platform_settings?.guardrails;
const speechGuardrail = guardrails?.custom?.config?.configs?.find(({ name }) => name === "Caller-facing speech only");
const deployedEndCall = (verified.conversation_config?.agent?.prompt?.tools || []).find(({ name }) => name === "end_call");
const deployedAssessment = (verified.conversation_config?.agent?.prompt?.tools || []).find(({ name }) => name === "assess_call_browser");
const deployedTransfer = (verified.conversation_config?.agent?.prompt?.tools || []).find(({ name }) => name === "transfer_to_human");
const deployedEvents = verified.conversation_config?.conversation?.client_events || [];
if (
  deployedPrompt !== prompt ||
  !guardrails?.focus?.is_enabled ||
  !speechGuardrail?.is_enabled ||
  speechGuardrail.execution_mode !== "blocking" ||
  speechGuardrail.trigger_action?.type !== "retry" ||
  deployedEndCall?.description !== endCallDescription ||
  deployedAssessment?.type !== "client" ||
  deployedAssessment?.expects_response !== true ||
  deployedTransfer?.type !== "client" ||
  deployedTransfer?.expects_response !== true ||
  !["tentative_user_transcript", "internal_tentative_agent_response", "agent_response"].every((name) => deployedEvents.includes(name))
) {
  throw new Error("Agent update returned successfully but the deployed safety settings did not verify");
}

const hash = crypto.createHash("sha256").update(deployedPrompt).digest("hex").slice(0, 12);
console.log(`Deployed and verified ${agentId}: prompt ${hash}, assess_call_browser and transfer_to_human on, caller-facing guardrails on.`);
