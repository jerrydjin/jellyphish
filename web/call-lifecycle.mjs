const LIVE_REMOTE_STATUSES = new Set(["initiated", "in-progress"]);
const ENDING_WIDGET_STATUSES = new Set(["disconnecting", "disconnected"]);

export function isRemoteConversationLive(call, locallyEndedAt = 0) {
  if (!LIVE_REMOTE_STATUSES.has(call?.status)) return false;
  const remotelyStartedAt = Number(call?.startedAt || 0) * 1000;
  return !locallyEndedAt || remotelyStartedAt > locallyEndedAt;
}

export function isEndingWidgetStatus(status) {
  return ENDING_WIDGET_STATUSES.has(status);
}
