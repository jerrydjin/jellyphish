import fs from "node:fs/promises";

const envText = await fs.readFile(new URL("../.env", import.meta.url), "utf8").catch(() => "");
const fileEnv = Object.fromEntries(
  envText
    .split(/\r?\n/)
    .filter((line) => line && !line.startsWith("#") && line.includes("="))
    .map((line) => line.split(/=(.*)/s).slice(0, 2)),
);
const apiKey = process.env.ELEVENLABS_API_KEY || fileEnv.ELEVENLABS_API_KEY;
const transferNumber = process.env.HUMAN_TRANSFER_NUMBER || fileEnv.HUMAN_TRANSFER_NUMBER;
const agentId = process.env.ELEVENLABS_AGENT_ID || fileEnv.ELEVENLABS_AGENT_ID || "agent_1101m2b4acdnedz9y2yky3w9vwfm";

if (!apiKey) throw new Error("ELEVENLABS_API_KEY is required in the environment or .env");
if (!/^\+[1-9]\d{7,14}$/.test(transferNumber || "")) {
  throw new Error("HUMAN_TRANSFER_NUMBER must be a valid E.164 number");
}

const template = JSON.parse(await fs.readFile(new URL("../transfer-tool.template.json", import.meta.url), "utf8"));
const transferTool = template.transfer_to_number;
transferTool.params.transfers[0].transfer_destination.phone_number = transferNumber;

const endpoint = `https://api.elevenlabs.io/v1/convai/agents/${agentId}`;
const headers = { "content-type": "application/json", "xi-api-key": apiKey };
const currentResponse = await fetch(endpoint, { headers });
if (!currentResponse.ok) throw new Error(`Reading agent failed (${currentResponse.status}): ${await currentResponse.text()}`);
const current = await currentResponse.json();
const prompt = current.conversation_config.agent.prompt;
if ((prompt.tools || []).length) delete prompt.tool_ids;
prompt.built_in_tools = { ...(prompt.built_in_tools || {}), transfer_to_number: transferTool };

const updateResponse = await fetch(endpoint, {
  method: "PATCH",
  headers,
  body: JSON.stringify({
    conversation_config: current.conversation_config,
    version_description: "Enable Rust-authorized fast green human handoff",
  }),
});
if (!updateResponse.ok) throw new Error(`Updating agent failed (${updateResponse.status}): ${await updateResponse.text()}`);

const verifiedResponse = await fetch(endpoint, { headers });
if (!verifiedResponse.ok) throw new Error(`Verifying agent failed (${verifiedResponse.status}): ${await verifiedResponse.text()}`);
const verified = await verifiedResponse.json();
const deployedTransfer = verified.conversation_config?.agent?.prompt?.built_in_tools?.transfer_to_number
  || (verified.conversation_config?.agent?.prompt?.tools || []).find(({ name }) => name === "transfer_to_number");
const deployedNumber = deployedTransfer?.params?.transfers?.[0]?.transfer_destination?.phone_number;
if (deployedNumber !== transferNumber) throw new Error("Transfer update returned successfully but the destination did not verify");

console.log(`Rust-authorized fast green transfer applied and verified for ${agentId}.`);
