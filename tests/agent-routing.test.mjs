import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const prompt = (await readFile(new URL("../agent-prompt.md", import.meta.url), "utf8")).trim();
const config = JSON.parse(await readFile(new URL("../elevenlabs-agent.json", import.meta.url), "utf8"));
const transferTemplate = JSON.parse(await readFile(new URL("../transfer-tool.template.json", import.meta.url), "utf8"));
const deployScript = await readFile(new URL("../scripts/deploy-agent-safety.mjs", import.meta.url), "utf8");
const dentalDeployScript = await readFile(new URL("../scripts/deploy-dental-agent.mjs", import.meta.url), "utf8");
const phoneSource = await readFile(new URL("../web/phone/index.html", import.meta.url), "utf8");
const handoffSource = await readFile(new URL("../web/handoff-call.mjs", import.meta.url), "utf8");
const dashboardSource = await readFile(new URL("../web/index.html", import.meta.url), "utf8");

test("named ordinary service requests trigger the fast green handoff path", () => {
  assert.match(prompt, /You are the judge/i);
  assert.match(prompt, /once both name and requested service are known/i);
  assert.match(prompt, /caller does not need to ask for a person/i);
  assert.match(prompt, /If transfer_allowed is true,[\s\S]*invoke transfer_to_human/i);
  assert.match(prompt, /If transfer_allowed is true,[\s\S]*never invoke end_call/i);
  assert.match(prompt, /I'll put you through now/i);
  assert.match(prompt, /never hang up a green call instead of transferring/i);
  assert.doesNotMatch(prompt, /virtual front desk|AI virtual|silent note-taking/i);
  assert.match(prompt, /Treat next_step as a required instruction for the very next turn/i);
  assert.doesNotMatch(prompt, /You do not decide whether a caller is green/i);
});

test("bounded distraction plays along instead of only asking verification questions", () => {
  assert.match(prompt, /bounded_distraction is allowed, it takes precedence over safe_refusal/i);
  assert.match(prompt, /do not say can't, cannot, or unable/i);
  assert.match(prompt, /do not offer to take a message/i);
  assert.match(prompt, /play along so the caller wastes time/i);
  assert.match(prompt, /do not only interrogate/i);
  assert.match(prompt, /obviously silly or wrong details/i);
  assert.match(prompt, /stay on the line indefinitely/i);
  assert.match(prompt, /Never invoke end_call on this route/i);
  assert.match(prompt, /Never hang up. Keep talking until their line drops/i);
  assert.doesNotMatch(prompt, /up to eight agent turns/i);
  assert.match(prompt, /never repeat or closely paraphrase your previous spoken sentence/i);
  assert.match(prompt, /never repeat it or merely swap a few words/i);
});

test("voice safety cannot retry the same line or terminate a valid call", () => {
  const guardrails = config.platform_settings.guardrails;
  assert.equal(guardrails.focus.is_enabled, true);
  assert.deepEqual(guardrails.custom.config.configs, []);
  assert.match(deployScript, /enabledCustomGuardrails\.length !== 0/);
  assert.match(dentalDeployScript, /enabledCustomGuardrails\.length !== 0/);
  assert.doesNotMatch(JSON.stringify(guardrails), /"type":"retry"/);
});

test("the browser agent records Sol's classification on a client tool", () => {
  const configuredPrompt = config.conversation_config.agent.prompt;
  assert.equal(configuredPrompt.prompt, prompt);
  const tool = configuredPrompt.tools.find(({ name }) => name === "assess_call_browser");
  assert.equal(tool.type, "client");
  assert.equal(tool.expects_response, true);
  assert.deepEqual(tool.parameters.required, ["risk_level", "requested_action", "proposed_action"]);
  assert.deepEqual(tool.parameters.properties.risk_level.enum, ["green", "amber", "red"]);
  const transfer = configuredPrompt.tools.find(({ name }) => name === "transfer_to_human");
  assert.equal(transfer.type, "client");
  assert.equal(transfer.expects_response, true);
  const endCall = configuredPrompt.built_in_tools.end_call;
  assert.match(endCall.description, /Never use this tool during a red or bounded-distraction call/i);
  assert.match(endCall.description, /On green, never use this instead of transferring/i);
  assert.doesNotMatch(endCall.description, /green or amber interaction is clearly complete/i);
  assert.match(config.conversation_config.agent.first_message, /this is Konner/i);
  assert.doesNotMatch(config.conversation_config.agent.first_message, /virtual/i);
  assert.equal(config.conversation_config.conversation.max_duration_seconds, 3600);
});

test("the phone transfer rule does not require an explicit human request", () => {
  const transfer = transferTemplate.transfer_to_number;
  assert.match(transfer.description, /name and requested service are known/i);
  assert.match(transfer.params.transfers[0].condition, /no explicit request for a person is required/i);
});

test("repeated deploys do not submit both inline tools and tool IDs", () => {
  assert.match(deployScript, /delete current\.conversation_config\.agent\.prompt\.tool_ids/);
});

test("the live browser session requests streaming caller and Sol transcript events", () => {
  const events = config.conversation_config.conversation.client_events;
  for (const eventName of ["user_transcript", "tentative_user_transcript", "agent_response", "internal_tentative_agent_response"]) {
    assert.ok(events.includes(eventName), `missing ${eventName}`);
  }
  assert.ok(!events.includes("agent_chat_response_part"), "redundant agent stream must stay disabled");
  assert.match(deployScript, /conversation\.client_events = source\.conversation_config\.conversation\.client_events/);
});

test("browser handoff is line scoped and resolves its tool before releasing the agent", () => {
  assert.match(phoneSource, /line: current\.line/);
  assert.match(phoneSource, /await callerHandoff\.startCaller[\s\S]*setTimeout\(\(\) => Promise\.resolve\(agentSession\?\.endSession\(\)\)/);
  assert.doesNotMatch(phoneSource, /onDebug:/);
  assert.match(dashboardSource, /api\/monitor\/\$\{encodeURIComponent\(LINE\)\}\/events/);
});

test("only the caller peer produces gated post-handoff captions", () => {
  assert.match(handoffSource, /captureCaptions = role === "caller"/);
  assert.match(handoffSource, /x-caption-key/);
  assert.match(handoffSource, /speechLevel\(\) >= SPEECH_RMS/);
  assert.match(handoffSource, /await postJson\(`\/api\/handoff\/\$\{encodeURIComponent\(this\.line\)\}\/hangup`/);
});

test("Evan & Kevin Dental uses Net with the same routing contract", async () => {
  const dentalPrompt = (await readFile(new URL("../agent-prompt-dental.md", import.meta.url), "utf8")).trim();
  const dental = JSON.parse(await readFile(new URL("../elevenlabs-agent-dental.json", import.meta.url), "utf8"));
  assert.match(dentalPrompt, /You are Net, the front desk at Evan & Kevin Dental/i);
  assert.match(dentalPrompt, /once both name and requested service are known/i);
  assert.equal(dental.conversation_config.agent.prompt.prompt, dentalPrompt);
  assert.match(dental.conversation_config.agent.first_message, /this is Net/i);
  assert.equal(dental.name, "Evan & Kevin Dental Front Desk");
  assert.deepEqual(dental.platform_settings.guardrails.custom.config.configs, []);
});
