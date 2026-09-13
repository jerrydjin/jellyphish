const UNFINISHED_REMOTE_STATUSES = new Set(["initiated", "in-progress", "processing"]);

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
    ? { role, message: message.trim(), eventId: payload?.event_id ?? null }
    : null;
}
