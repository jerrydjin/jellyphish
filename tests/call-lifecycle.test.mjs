import assert from "node:assert/strict";
import test from "node:test";

import {
  applyLiveTurn,
  conversationListFingerprint,
  effectiveConversationStatus,
  handoffSidecarTranscript,
  humanOutcome,
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

test("duplicate callback feeds do not repeat an already-finalized utterance", () => {
  let turns = applyLiveTurn([], { role: "user", message: "I need a haircut", eventId: 22, pending: false });
  turns = applyLiveTurn(turns, { role: "agent", message: "What is your name?", eventId: 23, pending: false });
  const duplicate = applyLiveTurn(turns, { role: "user", message: "I need a haircut", eventId: null, pending: false });
  assert.strictEqual(duplicate, turns);
  assert.equal(duplicate.filter((turn) => turn.message === "I need a haircut").length, 1);
});

test("a contained red call is never presented as a staff handoff", () => {
  assert.equal(humanOutcome({ data: { outcome: "distracted no transfer" } }), "Call contained");
  assert.equal(humanOutcome({ data: { outcome: "transfer denied" } }), "Call contained");
  assert.equal(humanOutcome({ data: { outcome: "transferred" } }), "Handed to staff");
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
  turns = applyLiveTurn(turns, { role: "agent", message: "Thanks for calling Barbershop.", eventId: 3, pending: false });
  assert.deepEqual(
    turns.map((turn) => `${turn.role}:${turn.pending ? "partial" : "final"}:${turn.message}`),
    ["agent:final:Thanks for calling Barbershop.", "user:final:Hi I need a cut tomorrow"],
  );
});

test("late caller finals slot in before Sol speech that already arrived", () => {
  let turns = applyLiveTurn([], {
    role: "agent",
    message: "Thanks for calling Barbershop, this is Konner. How can I help today?",
    eventId: "opening",
    pending: false,
  });
  turns = applyLiveTurn(turns, {
    role: "agent",
    message: "Of course, I can help you with that. May I please have your name?",
    eventId: 10,
    pending: false,
  });
  turns = applyLiveTurn(turns, {
    role: "user",
    message: "Hey there. I just want to book a haircut appointment for tomorrow, if that's all right.",
    eventId: 11,
    pending: false,
  });
  turns = applyLiveTurn(turns, { role: "user", message: "Uh, Jerry.", eventId: 12, pending: false });
  turns = applyLiveTurn(turns, { role: "agent", message: "I'll put you through now.", eventId: 13, pending: false });
  assert.deepEqual(
    turns.map((turn) => `${turn.role}:${turn.message}`),
    [
      "agent:Thanks for calling Barbershop, this is Konner. How can I help today?",
      "user:Hey there. I just want to book a haircut appointment for tomorrow, if that's all right.",
      "agent:Of course, I can help you with that. May I please have your name?",
      "user:Uh, Jerry.",
      "agent:I'll put you through now.",
    ],
  );
});

test("the handoff panel keeps screening speech and appends staff/caller turns", () => {
  const live = {
    transcript: [
      { role: "agent", message: "I'll put you through now." },
      { role: "user", message: "Uh, Jerry." },
    ],
  };
  assert.deepEqual(handoffSidecarTranscript({ transcript: [] }, live), {
    source: "screening",
    turns: [
      { role: "agent", text: "I'll put you through now.", pending: false },
      { role: "user", text: "Uh, Jerry.", pending: false },
    ],
  });
  assert.deepEqual(
    handoffSidecarTranscript(
      {
        transcript: [
          { role: "staff", text: "Studio Sol, this is Alex." },
          { role: "caller", text: "Hi, I booked a haircut for tomorrow." },
        ],
      },
      live,
    ),
    {
      source: "full",
      turns: [
        { role: "agent", text: "I'll put you through now.", pending: false },
        { role: "user", text: "Uh, Jerry.", pending: false },
        { role: "staff", text: "Studio Sol, this is Alex.", pending: false },
        { role: "caller", text: "Hi, I booked a haircut for tomorrow.", pending: false },
      ],
    },
  );
  assert.equal(handoffSidecarTranscript({}, {}).source, "empty");
});

test("a later Sol reply does not overwrite the opening line", () => {
  let turns = applyLiveTurn([], { role: "agent", message: "Thanks for calling Barbershop.", eventId: "tentative-agent", pending: true });
  turns = applyLiveTurn(turns, { role: "user", message: "Haircut tomorrow for Jerry", eventId: 2, pending: false });
  turns = applyLiveTurn(turns, { role: "agent", message: "I can help with that.", eventId: 3, pending: false });
  assert.deepEqual(
    turns.map((turn) => `${turn.role}:${turn.message}`),
    ["agent:Thanks for calling Barbershop.", "user:Haircut tomorrow for Jerry", "agent:I can help with that."],
  );
});

test("tool-call JSON is not shown as spoken Sol text", () => {
  assert.equal(
    normalizeIncomingSocketEvent({
      type: "internal_tentative_agent_response",
      tentative_agent_response_internal_event: { tentative_agent_response: '{"name":"assess_call_browser"}' },
    }),
    null,
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
