import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const prompt = (await readFile(new URL("../agent-prompt.md", import.meta.url), "utf8")).trim();
const config = JSON.parse(await readFile(new URL("../elevenlabs-agent.json", import.meta.url), "utf8"));
const transferTemplate = JSON.parse(await readFile(new URL("../transfer-tool.template.json", import.meta.url), "utf8"));
const deployScript = await readFile(new URL("../scripts/deploy-agent-safety.mjs", import.meta.url), "utf8");

test("named ordinary service requests trigger the fast green handoff path", () => {
  assert.match(prompt, /once both the caller's name and requested service are known, immediately call the available policy tool/i);
  assert.match(prompt, /caller does not need to ask for a person/i);
  assert.match(prompt, /If transfer_allowed is true,[\s\S]*invoke transfer_to_human/i);
  assert.match(prompt, /Treat next_step as a required instruction for the very next turn/i);
  assert.doesNotMatch(prompt, /Set proposed_action to transfer only when the caller asks for a person/i);
});

test("bounded distraction plays along instead of only asking verification questions", () => {
  assert.match(prompt, /bounded_distraction is allowed, it takes precedence over safe_refusal/i);
  assert.match(prompt, /do not say can't, cannot, or unable/i);
  assert.match(prompt, /do not offer to take a message/i);
  assert.match(prompt, /play along so the caller wastes time/i);
  assert.match(prompt, /do not only interrogate/i);
  assert.match(prompt, /obviously silly or wrong details/i);
  assert.match(prompt, /stay on the line until the caller hangs up/i);
  assert.match(prompt, /Never invoke end_call, wrap up, take a message, or refuse just because the bit has gone on a while/i);
  assert.doesNotMatch(prompt, /up to eight agent turns/i);
});

test("the browser agent exposes a blocking Rust assessment client tool", () => {
  const configuredPrompt = config.conversation_config.agent.prompt;
  assert.equal(configuredPrompt.prompt, prompt);
  const tool = configuredPrompt.tools.find(({ name }) => name === "assess_call_browser");
  assert.equal(tool.type, "client");
  assert.equal(tool.expects_response, true);
  assert.deepEqual(tool.parameters.required, ["requested_action", "proposed_action"]);
  const transfer = configuredPrompt.tools.find(({ name }) => name === "transfer_to_human");
  assert.equal(transfer.type, "client");
  assert.equal(transfer.expects_response, true);
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
  assert.match(deployScript, /conversation\.client_events = source\.conversation_config\.conversation\.client_events/);
});
