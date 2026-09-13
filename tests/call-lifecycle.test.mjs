import assert from "node:assert/strict";
import test from "node:test";

import {
  applyLiveTurn,
  conversationListFingerprint,
  effectiveConversationStatus,
  liveElapsedSeconds,
  normalizeConversationMessage,
  normalizeDebugEvent,
  normalizeIncomingSocketEvent,
  routePresentation,
  shouldAutoselectConversation,
} from "../web/call-lifecycle.mjs";

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
  assert.deepEqual(normalizeConversationMessage({ role: "user", source: "user", message: " Hello ", event_id: 4 }), {
    role: "user",
    message: "Hello",
    eventId: 4,
    pending: false,
  });
  assert.deepEqual(normalizeConversationMessage({ role: "agent", source: "ai", message: "Hi", event_id: 5 }), {
    role: "agent",
    message: "Hi",
    eventId: 5,
    pending: false,
  });
  assert.deepEqual(normalizeConversationMessage({ source: "user", message: "Again" }), {
    role: "user",
    message: "Again",
    eventId: null,
    pending: false,
  });
  assert.equal(normalizeConversationMessage({ type: "audio", message: "noise" }), null);
});

test("live assessment routes have explicit colour-state labels", () => {
  assert.deepEqual(routePresentation("green"), { route: "green", label: "Green · handoff" });
  assert.deepEqual(routePresentation("amber"), { route: "amber", label: "Amber · verify" });
  assert.deepEqual(routePresentation("red"), { route: "red", label: "Red · hold" });
  assert.deepEqual(routePresentation(null), { route: "unclassified", label: "Not classified" });
});

test("tentative user and agent socket events stream as pending turns", () => {
  assert.deepEqual(
    normalizeIncomingSocketEvent({
      type: "tentative_user_transcript",
      tentative_user_transcription_event: { user_transcript: " I need a ", event_id: 8 },
    }),
    { role: "user", message: "I need a", eventId: 8, pending: true },
  );
  assert.deepEqual(
    normalizeIncomingSocketEvent({
      type: "internal_tentative_agent_response",
      tentative_agent_response_internal_event: { tentative_agent_response: "Sure, I can help." },
    }),
    { role: "agent", message: "Sure, I can help.", eventId: "tentative-agent", pending: true },
  );
  assert.deepEqual(
    normalizeDebugEvent({ type: "tentative_agent_response", response: "Thanks for calling" }),
    { role: "agent", message: "Thanks for calling", eventId: "tentative-agent", pending: true },
  );
});

test("partial transcripts update in place and finals replace them without dropping the other speaker", () => {
  let turns = [];
  turns = applyLiveTurn(turns, { role: "agent", message: "Thanks for calling.", eventId: "opening", pending: true });
  turns = applyLiveTurn(turns, { role: "user", message: "Hi I", eventId: 1, pending: true });
  turns = applyLiveTurn(turns, { role: "user", message: "Hi I need a cut", eventId: 1, pending: true });
  turns = applyLiveTurn(turns, { role: "user", message: "Hi I need a cut tomorrow", eventId: 2, pending: false });
  turns = applyLiveTurn(turns, { role: "agent", message: "Thanks for calling Studio Sol.", eventId: 3, pending: false });
  assert.deepEqual(
    turns.map((turn) => `${turn.role}:${turn.pending ? "partial" : "final"}:${turn.message}`),
    ["agent:final:Thanks for calling Studio Sol.", "user:final:Hi I need a cut tomorrow"],
  );
});

test("live snapshots keep pending flags so the dashboard can stream captions", async () => {
  const { liveTranscriptPayload } = await import("../web/call-lifecycle.mjs");
  assert.deepEqual(
    liveTranscriptPayload([
      { role: "agent", message: "Thanks for calling", pending: true, eventId: "tentative-agent" },
      { role: "user", message: "Hi I need a", pending: true },
      { role: "system", message: "ignore me" },
    ]),
    [
      { role: "agent", message: "Thanks for calling", pending: true },
      { role: "user", message: "Hi I need a", pending: true },
    ],
  );
});

test("history polling does not steal a user-selected call after the list refreshes", () => {
  const conversations = [{ id: "conv-new" }, { id: "conv-old" }];
  assert.equal(
    shouldAutoselectConversation({
      selectedId: "conv-old",
      liveSessionId: "conv-new",
      liveActive: false,
      conversations,
      userPinned: true,
    }),
    null,
  );
  assert.equal(
    shouldAutoselectConversation({
      selectedId: "conv-old",
      liveSessionId: "conv-new",
      liveActive: false,
      conversations,
      userPinned: false,
    }),
    null,
  );
  assert.equal(
    shouldAutoselectConversation({
      selectedId: "conv-old",
      liveSessionId: "conv-new",
      liveActive: true,
      conversations,
      userPinned: false,
    }),
    "conv-new",
  );
  assert.equal(conversationListFingerprint(conversations), conversationListFingerprint([{ id: "conv-new" }, { id: "conv-old" }]));
});

test("operator home page rings only for a remote handed-off caller", async () => {
  const { staffCallPresentation } = await import("../web/call-lifecycle.mjs");
  assert.deepEqual(staffCallPresentation({ sessionId: "conv_1", handoffStatus: "ringing" }, "conv_1"), {
    phase: "waiting",
    isLocalCaller: true,
    sessionId: "conv_1",
  });
  assert.deepEqual(staffCallPresentation({ sessionId: "conv_1", handoffStatus: "ringing" }, "other"), {
    phase: "ringing",
    isLocalCaller: false,
    sessionId: "conv_1",
  });
  assert.equal(staffCallPresentation({ sessionId: "conv_1", handoffStatus: "connected" }).phase, "connected");
});

test("the operator staff line stays idle until a real ring or this console answers", async () => {
  const { operatorStaffPhase } = await import("../web/call-lifecycle.mjs");
  assert.equal(operatorStaffPhase({}), "idle");
  assert.equal(operatorStaffPhase({ sessionId: "demo_1", handoffStatus: "connected", source: "local-demo" }), "idle");
  assert.equal(operatorStaffPhase({ sessionId: "conv_1", handoffStatus: "ringing" }), "ringing");
  assert.equal(operatorStaffPhase({ sessionId: "conv_1", handoffStatus: "connected", source: "browser-handoff" }), "idle");
  assert.equal(operatorStaffPhase({ sessionId: "conv_1", handoffStatus: "connected", source: "browser-handoff" }, { answered: true }), "connected");
  assert.equal(operatorStaffPhase({ sessionId: "sip_1", handoffStatus: "connected", source: "verified-webhook" }), "monitoring");
  assert.equal(operatorStaffPhase({ sessionId: "conv_1", handoffStatus: "ended", source: "browser-handoff" }), "idle");
  assert.equal(operatorStaffPhase({ sessionId: "sip_1", handoffStatus: "ended", source: "verified-webhook" }), "ended");
});

test("caller previews keep a short name and request for the incoming staff line", async () => {
  const { callerPreviewFromTurns } = await import("../web/handoff-call.mjs");
  assert.deepEqual(callerPreviewFromTurns([{ role: "user", message: "Hi, my name is Jamie and I would like a haircut tomorrow" }]), {
    callerName: "Jamie",
    requestedAction: "Hi, my name is Jamie and I would like a haircut tomorrow",
  });
});
