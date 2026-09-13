const UNFINISHED_REMOTE_STATUSES = new Set(["initiated", "in-progress", "processing"]);

export const SOL_OPENING_LINE =
  "Thanks for calling Studio Sol Hair. I’m Sol, the virtual front desk. How can I help today?";

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
  const message = payload?.message;
  const source = String(payload?.role || payload?.source || "").toLowerCase();
  const role = source === "user" ? "user" : source === "agent" || source === "ai" ? "agent" : null;
  return typeof message === "string" && message.trim() && role
    ? { role, message: message.trim(), eventId: payload?.event_id ?? null, pending: false }
    : null;
}

export function routePresentation(route) {
  const normalized = String(route || "").toLowerCase();
  if (normalized === "green") return { route: "green", label: "Green · handoff" };
  if (normalized === "amber") return { route: "amber", label: "Amber · verify" };
  if (normalized === "red") return { route: "red", label: "Red · hold" };
  return { route: "unclassified", label: "Not classified" };
}

function trimmed(value) {
  return typeof value === "string" ? value.trim() : "";
}

export function normalizeIncomingSocketEvent(event) {
  if (!event || typeof event !== "object") return null;
  switch (event.type) {
    case "tentative_user_transcript": {
      const data = event.tentative_user_transcription_event || {};
      const message = trimmed(data.user_transcript);
      return message ? { role: "user", message, eventId: data.event_id ?? "user-partial", pending: true } : null;
    }
    case "user_transcript": {
      const data = event.user_transcription_event || {};
      const message = trimmed(data.user_transcript);
      return message ? { role: "user", message, eventId: data.event_id ?? null, pending: false } : null;
    }
    case "internal_tentative_agent_response": {
      const message = trimmed(event.tentative_agent_response_internal_event?.tentative_agent_response);
      return message ? { role: "agent", message, eventId: "tentative-agent", pending: true } : null;
    }
    case "agent_response": {
      const data = event.agent_response_event || {};
      const message = trimmed(data.agent_response);
      return message ? { role: "agent", message, eventId: data.event_id ?? null, pending: false } : null;
    }
    case "agent_chat_response_part": {
      const part = event.text_response_part || {};
      const eventId = part.response_id || part.event_id || "agent-stream";
      if (part.type === "start") return { role: "agent", message: "", eventId, pending: true, replace: true };
      if (part.type === "delta") return { role: "agent", message: String(part.text || ""), eventId, pending: true, append: true };
      if (part.type === "stop") return { role: "agent", message: "", eventId, pending: false, finalize: true };
      return null;
    }
    default:
      return null;
  }
}

export function normalizeDebugEvent(info) {
  if (info?.type === "tentative_agent_response") {
    const message = trimmed(info.response);
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
  return item.role === turn.role && String(item.eventId) === String(turn.eventId);
}

export function applyLiveTurn(transcript = [], turn) {
  if (!turn?.role) return transcript;
  const next = transcript.slice();
  const identified = () => next.findIndex((item) => turnIdentity(item, turn));
  const lastPending = () => lastIndexWhere(next, (item) => item.role === turn.role && item.pending);

  if (turn.append) {
    let index = identified();
    if (index < 0) index = lastPending();
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
    next.push({ role: turn.role, message: turn.message, eventId: turn.eventId ?? null, pending: true });
    return next;
  }

  if (turn.replace) {
    let index = identified();
    if (index < 0) index = lastPending();
    const replacement = { role: turn.role, message: turn.message || "", eventId: turn.eventId ?? null, pending: true };
    if (index >= 0) {
      next[index] = { ...next[index], ...replacement, eventId: turn.eventId ?? next[index].eventId };
      return next;
    }
    next.push(replacement);
    return next;
  }

  if (turn.finalize) {
    let index = identified();
    if (index < 0) index = lastPending();
    if (index < 0) return transcript;
    next[index] = { ...next[index], pending: false, eventId: turn.eventId ?? next[index].eventId };
    return next;
  }

  if (!turn.message) return transcript;

  if (turn.pending) {
    let index = identified();
    if (index < 0) index = lastPending();
    const replacement = { role: turn.role, message: turn.message, eventId: turn.eventId ?? null, pending: true };
    if (index >= 0) {
      next[index] = { ...next[index], ...replacement, eventId: turn.eventId ?? next[index].eventId };
      return next;
    }
    next.push(replacement);
    return next;
  }

  const byId = identified();
  if (byId >= 0) {
    next[byId] = { role: turn.role, message: turn.message, eventId: turn.eventId ?? next[byId].eventId, pending: false };
    return next;
  }
  const pending = lastPending();
  if (pending >= 0) {
    next[pending] = { role: turn.role, message: turn.message, eventId: turn.eventId ?? next[pending].eventId, pending: false };
    return next;
  }
  const last = next.at(-1);
  if (last && last.role === turn.role && last.message === turn.message && !last.pending) return transcript;
  next.push({ role: turn.role, message: turn.message, eventId: turn.eventId ?? null, pending: false });
  return next;
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
