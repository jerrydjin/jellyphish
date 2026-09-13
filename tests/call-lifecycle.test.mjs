import assert from "node:assert/strict";
import test from "node:test";

import { effectiveConversationStatus, liveElapsedSeconds, normalizeConversationMessage } from "../web/call-lifecycle.mjs";

test("an ended browser session overrides lagging remote statuses", () => {
  assert.equal(effectiveConversationStatus({ id: "conv-1", status: "in-progress" }, "conv-1"), "done");
  assert.equal(effectiveConversationStatus({ id: "conv-1", status: "processing" }, "conv-1"), "done");
  assert.equal(effectiveConversationStatus({ id: "conv-2", status: "in-progress" }, "conv-1"), "in-progress");
});

test("the timer freezes at the local disconnect time", () => {
  assert.equal(liveElapsedSeconds({ active: true, startedAt: 10_000, endedAt: 0 }, 31_900), 21);
  assert.equal(liveElapsedSeconds({ active: false, startedAt: 10_000, endedAt: 31_900 }, 90_000), 21);
});

test("official ElevenLabs message payloads normalize into transcript turns", () => {
  assert.deepEqual(normalizeConversationMessage({ role: "user", source: "user", message: " Hello ", event_id: 4 }), { role: "user", message: "Hello", eventId: 4 });
  assert.deepEqual(normalizeConversationMessage({ role: "agent", source: "ai", message: "Hi", event_id: 5 }), { role: "agent", message: "Hi", eventId: 5 });
  assert.deepEqual(normalizeConversationMessage({ source: "user", message: "Again" }), { role: "user", message: "Again", eventId: null });
  assert.equal(normalizeConversationMessage({ type: "audio", message: "noise" }), null);
});
