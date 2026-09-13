const UNFINISHED_REMOTE_STATUSES = new Set(["initiated", "in-progress", "processing"]);

export const SOL_OPENING_LINE =
  "Thanks for calling Barbershop, this is Konner. How can I help today?";

export function effectiveConversationStatus(call, locallyEndedId = null) {
  if (call?.id === locallyEndedId && UNFINISHED_REMOTE_STATUSES.has(call?.status)) return "done";
  return call?.status;
}

export function liveElapsedSeconds(live, now = Date.now()) {
  if (!live?.startedAt) return 0;
  const endpoint = live.active ? now : live.endedAt || live.startedAt;
  return Math.max(0, Math.floor((endpoint - live.startedAt) / 1000));
}

export function normalizeConversationMessage(payload) {
  const message = spokenText(payload?.message);
  const source = String(payload?.role || payload?.source || "").toLowerCase();
  const role = source === "user" ? "user" : source === "agent" || source === "ai" ? "agent" : null;
  return message && role
    ? { role, message, eventId: payload?.event_id ?? null, pending: false }
    : null;
}

export function isStaffHandoffSpeech(text) {
  const message = String(text || "");
  return /\b(?:i(?:['’]?ll| will)|let me) (?:just )?(?:put you through|transfer you|connect you)\b/i.test(message)
    || /\bputting you through\b/i.test(message)
    || /\bi['’]?ll just find someone(?: for you)?\b/i.test(message);
}

export function routePresentation(route) {
  const normalized = String(route || "").toLowerCase();
  if (normalized === "green") return { route: "green", label: "Green · handoff" };
  if (normalized === "amber") return { route: "amber", label: "Amber · verify" };
  if (normalized === "red") return { route: "red", label: "Red · hold" };
  return { route: "unclassified", label: "Not classified" };
}

export function humanOutcome(call, locallyEndedId = null) {
  const status = effectiveConversationStatus(call, locallyEndedId);
  const outcome = String(call?.data?.outcome || status || "").trim();
  if (/distracted\s+no\s+transfer|transfer\s+(?:held|denied|blocked)|no\s+transfer/i.test(outcome)) return "Call contained";
  if (/caller disconnected|disconnected/i.test(outcome)) return "Call ended";
  if (/\btransferred\b|handed\s+(?:off|to\s+staff)/i.test(outcome)) return "Handed to staff";
  if (/message/i.test(outcome)) return "Message taken";
  if (/completed|complete|resolved/i.test(outcome)) return "Conversation complete";
  return outcome || "Processing";
}

function trimmed(value) {
  if (typeof value === "string") return value.trim();
  if (value && typeof value === "object" && typeof value.text === "string") return value.text.trim();
  return "";
}

const INTERNAL_THOUGHT = new RegExp(
  [
    "\\b(?:red|amber|green)\\s+(?:tarpit|route|routing|mode)\\b",
    "\\btarpit\\b",
    "\\brouting model\\b",
    "\\bi am still in\\b",
    "\\bthe user provided\\b",
    "\\bbounded[_ ]?distraction\\b",
    "\\ballowed_actions\\b",
    "\\brisk_level\\b",
    "\\btransfer_allowed\\b",
    "\\bnext_step\\b",
    "\\bassess_call(?:_browser)?\\b",
    "\\binternal (?:reasoning|monologue|note|thought|analysis)\\b",
    "\\bclassif(?:y|ied|ication) (?:as|this|the call)\\b",
    "\\bthis (?:call|caller) is (?:red|amber|green)\\b",
    "\\bI (?:will|should|need to) (?:now )?(?:classify|call the (?:assessment )?tool)\\b",
    "\\btool (?:result|payload|returned)\\b",
    "\\bsystem prompt\\b",
    "\\bhidden instruction",
    "\\bspeaker:\\s",
    "\\bthe caller (?:provided|is (?:red|amber|green|trying))",
  ].join("|"),
  "i",
);

export function callerFacingSpeech(value) {
  const message = trimmed(value);
  if (!message || message.startsWith("{") || message.startsWith("[")) return "";
  for (const paragraph of message
    .split(/\n+/)
    .map((part) => part.trim())
    .filter(Boolean)) {
    const spoken = paragraph
      .split(/(?<=[.!?])\s+(?=[A-Z])/)
      .map((part) => part.trim())
      .filter(Boolean)
      .filter((part) => !INTERNAL_THOUGHT.test(part));
    if (spoken.length) return spoken.join(" ");
  }
  return "";
}

function spokenText(value) {
  return callerFacingSpeech(value);
}

function isStreamPlaceholder(eventId) {
  return eventId == null || ["opening", "tentative-agent", "user-partial", "agent-stream"].includes(String(eventId));
}

export function normalizeIncomingSocketEvent(event) {
  if (!event || typeof event !== "object") return null;
  switch (event.type) {
    case "tentative_user_transcript": {
      const data = event.tentative_user_transcription_event || event;
      const message = spokenText(data.user_transcript);
      return message ? { role: "user", message, eventId: data.event_id ?? "user-partial", pending: true } : null;
    }
    case "user_transcript": {
      const data = event.user_transcription_event || event;
      const message = spokenText(data.user_transcript);
      return message ? { role: "user", message, eventId: data.event_id ?? null, pending: false } : null;
    }
    case "internal_tentative_agent_response": {
      const message = spokenText(event.tentative_agent_response_internal_event?.tentative_agent_response || event.tentative_agent_response);
      return message ? { role: "agent", message, eventId: "tentative-agent", pending: true } : null;
    }
    case "agent_response": {
      const data = event.agent_response_event || event;
      const message = spokenText(data.agent_response || data.message);
      return message ? { role: "agent", message, eventId: data.event_id ?? null, pending: false } : null;
    }
    // `internal_tentative_agent_response` already supplies the live preview.
    // Consuming this second streaming feed as well can replay a full response
    // or leave a trailing delta behind when its final callback arrives late.
    case "agent_chat_response_part":
      return null;
    default:
      return null;
  }
}

export function normalizeDebugEvent(info) {
  if (info?.type === "tentative_agent_response") {
    const message = spokenText(info.response);
    return message ? { role: "agent", message, eventId: "tentative-agent", pending: true } : null;
  }
  return normalizeIncomingSocketEvent(info);
}

function lastIndexWhere(items, predicate) {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    if (predicate(items[index])) return index;
  }
  return -1;
}

function turnIdentity(item, turn) {
  if (turn.eventId == null || item.eventId == null) return false;
  // These are local placeholders reused across multiple spoken turns, not
  // stable ElevenLabs event identities.
  if (isStreamPlaceholder(item.eventId) || isStreamPlaceholder(turn.eventId)) return false;
  return item.role === turn.role && String(item.eventId) === String(turn.eventId);
}

function comparableMessage(value) {
  return String(value || "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
}

function sameMessageFamily(left, right) {
  const a = comparableMessage(left);
  const b = comparableMessage(right);
  if (!a || !b) return false;
  if (a === b) return true;
  const shorter = a.length <= b.length ? a : b;
  const longer = a.length > b.length ? a : b;
  return shorter.length >= 12 && (longer.startsWith(shorter) || longer.includes(shorter));
}

// Caller finals often arrive after Sol has already started the next reply.
// Put a new caller line after the greeting (or after the last user + Sol's
// reply to that user), never after later Sol speech that already leaked in.
export function conversationalInsertIndex(turns = [], role) {
  if (role !== "user" && role !== "caller") return turns.length;
  const lastUser = lastIndexWhere(turns, (item) => item.role === "user" || item.role === "caller");
  if (lastUser < 0) {
    const opening = turns.findIndex((item) => item.role === "agent");
    return opening >= 0 ? opening + 1 : turns.length;
  }
  let skippedReply = false;
  for (let index = lastUser + 1; index < turns.length; index += 1) {
    if (turns[index].role !== "agent") continue;
    if (!skippedReply) {
      skippedReply = true;
      continue;
    }
    return index;
  }
  return turns.length;
}

function insertSpokenTurn(turns, turn) {
  const spoken = {
    role: turn.role,
    message: turn.message,
    eventId: turn.eventId ?? null,
    pending: Boolean(turn.pending),
  };
  turns.splice(conversationalInsertIndex(turns, turn.role), 0, spoken);
  return turns;
}

export function liveTranscriptPayload(turns = []) {
  return turns
    .filter((turn) => (turn.role === "user" || turn.role === "agent" || turn.role === "staff" || turn.role === "caller") && turn.message)
    .map((turn) => ({
      role: turn.role,
      message: turn.message,
      pending: Boolean(turn.pending),
    }));
}

function cleanScreeningTurns(turns) {
  const finals = turns.filter((turn) => !turn.pending);
  const unique = [];
  for (const turn of turns) {
    if (
      turn.pending &&
      finals.some((finalTurn) =>
        finalTurn.role === turn.role && sameMessageFamily(finalTurn.text, turn.text),
      )
    ) {
      continue;
    }
    const key = `${turn.role}\u001f${comparableMessage(turn.text)}`;
    if (unique.some((item) => item.key === key)) continue;
    unique.push({ key, turn });
  }
  const cleaned = unique.map(({ turn }) => turn);
  const agents = cleaned.filter((turn) => turn.role === "agent");
  const users = cleaned.filter((turn) => turn.role === "user");
  const canRestoreAlternation =
    cleaned[0]?.role === "agent" &&
    agents.length >= users.length &&
    agents.length <= users.length + 1;
  if (!canRestoreAlternation) return cleaned;
  const ordered = [];
  for (let index = 0; index < agents.length; index += 1) {
    ordered.push(agents[index]);
    if (users[index]) ordered.push(users[index]);
  }
  return ordered;
}

export function applyLiveTurn(transcript = [], turn) {
  if (!turn?.role) return transcript;
  const next = transcript.slice();
  const identified = () => next.findIndex((item) => turnIdentity(item, turn));
  const compatiblePending = () => {
    const index = identified();
    if (index >= 0) return index;
    const pendingIndexes = next
      .map((item, itemIndex) => ({ item, itemIndex }))
      .filter(({ item }) => item.role === turn.role && item.pending)
      .map(({ itemIndex }) => itemIndex);
    if (!pendingIndexes.length) return -1;

    if (turn.message) {
      const matching = pendingIndexes.findLast((itemIndex) =>
        sameMessageFamily(next[itemIndex].message, turn.message),
      );
      if (matching != null) return matching;
    }

    // A tentative reply may arrive before the previous tentative reply's
    // final callback. Do not let it overwrite a placeholder that sits before
    // a newer caller turn.
    const lastOtherSpeaker = lastIndexWhere(next, (item) => item.role !== turn.role);
    const latest = pendingIndexes.at(-1);
    if (latest > lastOtherSpeaker) return latest;
    return -1;
  };

  if (turn.append) {
    let index = compatiblePending();
    if (index >= 0) {
      next[index] = {
        ...next[index],
        message: `${next[index].message || ""}${turn.message || ""}`,
        eventId: turn.eventId ?? next[index].eventId,
        pending: true,
      };
      return next;
    }
    if (!turn.message) return transcript;
    return insertSpokenTurn(next, { ...turn, pending: true });
  }

  if (turn.replace) {
    let index = compatiblePending();
    const replacement = { role: turn.role, message: turn.message || "", eventId: turn.eventId ?? null, pending: true };
    if (index >= 0 && !turn.message && next[index].message) {
      next.push(replacement);
      return next;
    }
    if (index >= 0) {
      next[index] = { ...next[index], ...replacement, eventId: turn.eventId ?? next[index].eventId };
      return next;
    }
    if (!turn.message) return transcript;
    return insertSpokenTurn(next, replacement);
  }

  if (turn.finalize) {
    const index = compatiblePending();
    if (index < 0) return transcript;
    next[index] = { ...next[index], pending: false, eventId: turn.eventId ?? next[index].eventId };
    return next;
  }

  if (!turn.message) return transcript;

  if (turn.pending) {
    let index = compatiblePending();
    const replacement = { role: turn.role, message: turn.message, eventId: turn.eventId ?? null, pending: true };
    if (index >= 0) {
      next[index] = { ...next[index], ...replacement, eventId: turn.eventId ?? next[index].eventId };
      return next;
    }
    return insertSpokenTurn(next, replacement);
  }

  const byId = identified();
  if (byId >= 0) {
    next[byId] = { role: turn.role, message: turn.message, eventId: turn.eventId ?? next[byId].eventId, pending: false };
    return next;
  }
  const pending = compatiblePending();
  if (pending >= 0) {
    next[pending] = { role: turn.role, message: turn.message, eventId: turn.eventId ?? next[pending].eventId, pending: false };
    return next;
  }
  const duplicate = lastIndexWhere(next, (item) => item.role === turn.role && item.message === turn.message);
  if (duplicate >= 0) {
    if (!next[duplicate].pending) return transcript;
    next[duplicate] = {
      role: turn.role,
      message: turn.message,
      eventId: turn.eventId ?? next[duplicate].eventId,
      pending: false,
    };
    return next;
  }
  const last = next.at(-1);
  if (last && last.role === turn.role && last.message === turn.message && !last.pending) return transcript;
  return insertSpokenTurn(next, { role: turn.role, message: turn.message, eventId: turn.eventId ?? null, pending: false });
}

export function handoffSidecarTranscript(monitor = {}, live = {}) {
  const fromLive = (live.transcript || [])
    .map((turn) => ({
      role: turn.role,
      text:
        turn.role === "agent"
          ? callerFacingSpeech(turn.message || turn.text)
          : String(turn.message || turn.text || "").trim(),
      pending: Boolean(turn.pending),
    }))
    .filter((turn) => turn.text);
  const screening = cleanScreeningTurns(
    fromLive.filter((turn) => turn.role === "agent" || turn.role === "user"),
  );
  const liveHuman = fromLive.filter((turn) => turn.role === "staff" || turn.role === "caller");
  const monitorHuman = (monitor.transcript || [])
    .map((turn) => ({
      role: turn.role === "staff" ? "staff" : "caller",
      text: String(turn.text || turn.message || "").trim(),
      pending: Boolean(turn.pending),
    }))
    .filter((turn) => turn.text);
  const human = monitorHuman.length ? monitorHuman : liveHuman;
  const turns = [...screening, ...human];
  if (!turns.length) return { source: "empty", turns: [] };
  return { source: human.length ? "full" : "screening", turns };
}

export function conversationListFingerprint(conversations = []) {
  return conversations
    .map((call) =>
      [call.id, call.status, call.duration, call.title, call.summary, call.data?.outcome, call.policy?.risk_level].join("\u001f"),
    )
    .join("\n");
}

export function shouldAutoselectConversation({
  selectedId = null,
  liveSessionId = null,
  liveActive = false,
  conversations = [],
  userPinned = false,
} = {}) {
  if (userPinned) return null;
  if (liveActive && liveSessionId) {
    const live = conversations.find((call) => call.id === liveSessionId);
    if (live && live.id !== selectedId) return live.id;
  }
  if (!selectedId && conversations[0]) return conversations[0].id;
  return null;
}

export function staffCallPresentation(monitor = {}, localSessionId = null) {
  const sessionId = monitor?.sessionId || "";
  const isLocalCaller = Boolean(localSessionId && sessionId && sessionId === localSessionId);
  const status = String(monitor?.handoffStatus || "");
  if (!sessionId) return { phase: "idle", isLocalCaller: false, sessionId: "" };
  if (status === "ringing") return { phase: isLocalCaller ? "waiting" : "ringing", isLocalCaller, sessionId };
  if (status === "connected") return { phase: isLocalCaller ? "handed-off" : "connected", isLocalCaller, sessionId };
  if (status === "ended") return { phase: "ended", isLocalCaller, sessionId };
  return { phase: "monitoring", isLocalCaller, sessionId };
}

export function operatorStaffPhase(monitor = {}, { answered = false } = {}) {
  if (answered) return "connected";
  const status = String(monitor?.handoffStatus || "");
  if (status === "ringing") return "ringing";
  if (monitor.source === "verified-webhook" && status === "connected") return "monitoring";
  if (monitor.source === "verified-webhook" && status === "ended") return "ended";
  return "idle";
}
