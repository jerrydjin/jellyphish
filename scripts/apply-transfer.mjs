import fs from "node:fs";

const apiKey = process.env.ELEVENLABS_API_KEY;
const transferNumber = process.env.HUMAN_TRANSFER_NUMBER;
const agentId = process.env.ELEVENLABS_AGENT_ID || "agent_1101m2b4acdnedz9y2yky3w9vwfm";

if (!apiKey) throw new Error("ELEVENLABS_API_KEY is required");
if (!/^\+[1-9]\d{7,14}$/.test(transferNumber || "")) {
  throw new Error("HUMAN_TRANSFER_NUMBER must be a valid E.164 number");
}

const config = JSON.parse(fs.readFileSync("elevenlabs-agent.json", "utf8"));
const template = JSON.parse(fs.readFileSync("transfer-tool.template.json", "utf8"));
template.transfer_to_number.params.transfers[0].transfer_destination.phone_number = transferNumber;

config.conversation_config.agent.prompt.built_in_tools.transfer_to_number =
  template.transfer_to_number;

const response = await fetch(`https://api.elevenlabs.io/v1/convai/agents/${agentId}`, {
  method: "PATCH",
  headers: {
    "content-type": "application/json",
    "xi-api-key": apiKey,
  },
  body: JSON.stringify(config),
});

if (!response.ok) {
  throw new Error(`ElevenLabs update failed (${response.status}): ${await response.text()}`);
}

console.log(`Safe-green transfer rule applied to ${agentId}.`);
