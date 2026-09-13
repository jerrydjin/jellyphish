import crypto from "node:crypto";
import fs from "node:fs/promises";

const envText = await fs.readFile(new URL("../.env", import.meta.url), "utf8").catch(() => "");
const fileEnv = Object.fromEntries(
  envText
    .split(/\r?\n/)
    .filter((line) => line && !line.startsWith("#") && line.includes("="))
    .map((line) => line.split(/=(.*)/s).slice(0, 2)),
);
const apiKey = process.env.ELEVENLABS_API_KEY || fileEnv.ELEVENLABS_API_KEY;
if (!apiKey) throw new Error("ELEVENLABS_API_KEY is required in the environment or .env");

const prompt = (await fs.readFile(new URL("../agent-prompt-dental.md", import.meta.url), "utf8")).trim();
const source = JSON.parse(await fs.readFile(new URL("../elevenlabs-agent-dental.json", import.meta.url), "utf8"));
const headers = { "content-type": "application/json", "xi-api-key": apiKey };

let agentId = process.env.ELEVENLABS_DENTAL_AGENT_ID || fileEnv.ELEVENLABS_DENTAL_AGENT_ID;
if (!agentId) {
  const created = await fetch("https://api.elevenlabs.io/v1/convai/agents/create", {
    method: "POST",
    headers,
    body: JSON.stringify({
      name: source.name,
      conversation_config: source.conversation_config,
      platform_settings: source.platform_settings,
    }),
  });
  if (!created.ok) throw new Error(`Creating dental agent failed (${created.status}): ${await created.text()}`);
  const body = await created.json();
  agentId = body.agent_id || body.agentId || body.id;
  if (!agentId) throw new Error(`Creating dental agent returned no id: ${JSON.stringify(body)}`);
  console.log(`Created dental agent ${agentId}. Add ELEVENLABS_DENTAL_AGENT_ID=${agentId} to .env`);
}

const endpoint = `https://api.elevenlabs.io/v1/convai/agents/${agentId}`;
const currentResponse = await fetch(endpoint, { headers });
if (!currentResponse.ok) throw new Error(`Reading agent failed (${currentResponse.status}): ${await currentResponse.text()}`);
const current = await currentResponse.json();

current.conversation_config.agent.prompt.prompt = prompt;
current.conversation_config.agent.first_message = source.conversation_config.agent.first_message;
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
current.conversation_config.conversation.max_duration_seconds = source.conversation_config.conversation.max_duration_seconds;

const updateResponse = await fetch(endpoint, {
  method: "PATCH",
  headers,
  body: JSON.stringify({
    name: source.name,
    conversation_config: current.conversation_config,
    platform_settings: current.platform_settings,
    version_description: "Block spoken internal thought without hanging up valid stalls",
  }),
});
if (!updateResponse.ok) throw new Error(`Updating agent failed (${updateResponse.status}): ${await updateResponse.text()}`);

const verifiedResponse = await fetch(endpoint, { headers });
if (!verifiedResponse.ok) throw new Error(`Verifying agent failed (${verifiedResponse.status}): ${await verifiedResponse.text()}`);
const verified = await verifiedResponse.json();
const deployedPrompt = verified.conversation_config?.agent?.prompt?.prompt || "";
const deployedFirstMessage = verified.conversation_config?.agent?.first_message || "";
const guardrails = verified.platform_settings?.guardrails;
const speechGuardrail = guardrails?.custom?.config?.configs?.find(({ name }) => name === "Caller-facing speech only");
const deployedEvents = verified.conversation_config?.conversation?.client_events || [];
if (
  deployedPrompt !== prompt ||
  deployedFirstMessage !== source.conversation_config.agent.first_message ||
  !guardrails?.focus?.is_enabled ||
  !speechGuardrail?.is_enabled ||
  speechGuardrail.execution_mode !== "blocking" ||
  speechGuardrail.trigger_action?.type !== "retry" ||
  deployedEvents.includes("agent_chat_response_part") ||
  !["tentative_user_transcript", "internal_tentative_agent_response", "agent_response"].every((name) => deployedEvents.includes(name))
) {
  throw new Error("Dental agent update returned successfully but the deployed prompt did not verify");
}
const hash = crypto.createHash("sha256").update(deployedPrompt).digest("hex").slice(0, 12);
console.log(`Deployed and verified dental agent ${agentId}: prompt ${hash}, Focus on, caller-facing speech guardrail on.`);
