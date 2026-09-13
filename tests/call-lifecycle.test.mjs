import assert from "node:assert/strict";
import test from "node:test";

import { isEndingWidgetStatus, isRemoteConversationLive } from "../web/call-lifecycle.mjs";

test("post-call processing does not appear as a live call", () => {
  assert.equal(isRemoteConversationLive({ status: "processing", startedAt: 20 }, 0), false);
  assert.equal(isRemoteConversationLive({ status: "done", startedAt: 20 }, 0), false);
});

test("a remote poll cannot resurrect the call that just ended locally", () => {
  assert.equal(isRemoteConversationLive({ status: "in-progress", startedAt: 20 }, 20_500), false);
  assert.equal(isRemoteConversationLive({ status: "in-progress", startedAt: 21 }, 20_500), true);
});

test("both widget teardown statuses end the live console session", () => {
  assert.equal(isEndingWidgetStatus("disconnecting"), true);
  assert.equal(isEndingWidgetStatus("disconnected"), true);
  assert.equal(isEndingWidgetStatus("connected"), false);
});
