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
  assert.match(prompt, /The spoken sentence does not connect anyone by itself/i);
  assert.match(prompt, /do not stop generating after the sentence without the tool call/i);
  assert.match(prompt, /never hang up a green or amber call instead of transferring/i);
  assert.match(prompt, /Wanting to speak to staff about an invoice or bank details is amber, not red/i);
  assert.match(prompt, /Do not transfer on the first amber turn unless those fields were already volunteered/i);
  assert.match(prompt, /If next_step asks for a name, company, or reference/i);
  assert.match(prompt, /ask for the reference next/i);
  assert.match(prompt, /classified the current facts as green, or as a screened amber caller/i);
  assert.doesNotMatch(prompt, /virtual front desk|AI virtual|silent note-taking/i);
  assert.match(prompt, /The first character you generate is the first word the caller hears/i);
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

test("voice safety blocks leaked thoughts without treating stalls as violations", () => {
  const guardrails = config.platform_settings.guardrails;
  const speechGuardrail = guardrails.custom.config.configs.find(({ name }) => name === "Caller-facing speech only");
  assert.equal(guardrails.focus.is_enabled, true);
  assert.equal(speechGuardrail.is_enabled, true);
  assert.equal(speechGuardrail.execution_mode, "blocking");
  assert.equal(speechGuardrail.trigger_action.type, "retry");
  assert.match(speechGuardrail.prompt, /Do not block a single natural sentence/i);
  assert.match(deployScript, /speechGuardrail\.execution_mode !== "blocking"/);
  assert.match(dentalDeployScript, /speechGuardrail\.execution_mode !== "blocking"/);
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
  assert.match(transfer.description, /Saying the sentence without this tool fails the handoff/i);
  const endCall = configuredPrompt.built_in_tools.end_call;
  assert.match(endCall.description, /Never use this tool during a red or bounded-distraction call/i);
  assert.match(endCall.description, /On green or screened amber, never use this instead of transferring/i);
  assert.doesNotMatch(endCall.description, /green or amber interaction is clearly complete/i);
  assert.match(config.conversation_config.agent.first_message, /this is Konner/i);
  assert.doesNotMatch(config.conversation_config.agent.first_message, /virtual/i);
  assert.equal(config.conversation_config.conversation.max_duration_seconds, 3600);
});

test("the phone transfer rule does not require an explicit human request", () => {
  const transfer = transferTemplate.transfer_to_number;
  assert.match(transfer.description, /name and requested service are known/i);
  assert.match(transfer.description, /screened amber caller/i);
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

test("browser handoff is line scoped and releases the agent after the handoff sentence can play", () => {
  assert.match(phoneSource, /line: current\.line/);
  assert.match(phoneSource, /await callerHandoff\.startCaller[\s\S]*scheduleAgentRelease\(current, current\.session\)/);
  assert.match(phoneSource, /function scheduleAgentRelease/);
  assert.doesNotMatch(phoneSource, /onDebug:/);
  assert.match(dashboardSource, /api\/monitor\/\$\{encodeURIComponent\(LINE\)\}\/events/);
  const tools = config.conversation_config.agent.prompt.tools;
  assert.equal(tools.find(({ name }) => name === "assess_call_browser")?.interruption_mode, "disable_during_tool_and_turn");
  assert.equal(tools.find(({ name }) => name === "transfer_to_human")?.interruption_mode, "disable_during_tool_and_turn");
});

test("the staff console owns same-device post-handoff captions", () => {
  assert.match(handoffSource, /captureCaptions = true/);
  assert.match(handoffSource, /caption_key: this\.captionKey/);
  assert.match(phoneSource, /captureCaptions: false/);
  assert.match(dashboardSource, /captionRole: \(\) => handoffSpeakerRole/);
  assert.match(dashboardSource, /isCaptureActive: \(\) => true/);
  assert.doesNotMatch(phoneSource, /BroadcastChannel\("jellyphish-handoff-speaker"\)/);
  assert.match(phoneSource, /id="handoff-remote" autoplay playsinline muted/);
  assert.match(dashboardSource, /id="handoff-remote" autoplay playsinline muted/);
  assert.match(handoffSource, /x-caption-key/);
  assert.match(handoffSource, /speechLevel\(\) >= SPEECH_RMS/);
  assert.match(handoffSource, /const roleChanged = getRole\(\) !== roleAtStart/);
  assert.doesNotMatch(handoffSource, /!speechDetected \|\| blob\.size/);
  assert.match(handoffSource, /async reviveLocalAudio/);
  assert.match(handoffSource, /replaceTrack/);
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
  assert.deepEqual(dental.platform_settings.guardrails.custom.config.configs[0].name, "Caller-facing speech only");
  assert.equal(dental.platform_settings.guardrails.custom.config.configs[0].execution_mode, "blocking");
});
