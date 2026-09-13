import fs from "node:fs";

const apiKey = process.env.ELEVENLABS_API_KEY;
const baseUrl = String(process.env.PUBLIC_BASE_URL || "").replace(/\/$/, "");
const agentId = process.env.ELEVENLABS_AGENT_ID || "agent_1101m2b4acdnedz9y2yky3w9vwfm";

if (!apiKey) throw new Error("ELEVENLABS_API_KEY is required");
if (!/^https:\/\//.test(baseUrl)) throw new Error("PUBLIC_BASE_URL must be a public HTTPS origin");

const listResponse = await fetch("https://api.elevenlabs.io/v1/convai/tools", {
  headers: { "xi-api-key": apiKey },
});
const listBody = listResponse.ok ? await listResponse.json() : { tools: [] };
const existingTools = Array.isArray(listBody) ? listBody : listBody.tools || [];

async function createTool(file) {
  const template = fs.readFileSync(file, "utf8").replaceAll("__PUBLIC_BASE_URL__", baseUrl);
  const config = JSON.parse(template);
  const existing = existingTools.find((tool) => tool.tool_config?.name === config.tool_config.name);
  if (existing?.id) return existing.id;
  const response = await fetch("https://api.elevenlabs.io/v1/convai/tools", {
    method: "POST",
    headers: { "content-type": "application/json", "xi-api-key": apiKey },
    body: template,
  });
  if (!response.ok) throw new Error(`Creating ${file} failed (${response.status}): ${await response.text()}`);
  return (await response.json()).id;
}

const toolIds = [];
for (const file of ["tools/assess-call.template.json", "tools/finish-call.template.json"]) {
  toolIds.push(await createTool(file));
}

const agentResponse = await fetch(`https://api.elevenlabs.io/v1/convai/agents/${agentId}`, {
  headers: { "xi-api-key": apiKey },
});
if (!agentResponse.ok) throw new Error(`Reading agent failed (${agentResponse.status}): ${await agentResponse.text()}`);
const agent = await agentResponse.json();
const prompt = agent.conversation_config.agent.prompt;
prompt.tool_ids = [...new Set([...(prompt.tool_ids || []), ...toolIds])];

const response = await fetch(`https://api.elevenlabs.io/v1/convai/agents/${agentId}`, {
  method: "PATCH",
  headers: { "content-type": "application/json", "xi-api-key": apiKey },
  body: JSON.stringify({ conversation_config: agent.conversation_config }),
});
if (!response.ok) throw new Error(`Attaching tools failed (${response.status}): ${await response.text()}`);
console.log(`Attached assess_call and finish_call to ${agentId}.`);
