import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { spawn } from "node:child_process";
import test from "node:test";
import { assessTranscriptChunk } from "../risk-engine.mjs";

const PORT = 41973;
const BASE = `http://127.0.0.1:${PORT}`;
const SECRET = "local-monitored-handoff-test-secret";

function signedHeaders(body, secret = SECRET) {
  const timestamp = String(Math.floor(Date.now() / 1000));
  const signature = createHmac("sha256", secret).update(`${timestamp}.${body}`).digest("hex");
  return {
    "content-type": "application/json",
    "x-jellyphish-timestamp": timestamp,
    "x-jellyphish-signature": `sha256=${signature}`,
  };
}

async function post(path, payload, secret) {
  const body = JSON.stringify(payload);
  return fetch(`${BASE}${path}`, { method: "POST", headers: signedHeaders(body, secret), body });
}

async function waitForServer(child) {
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("server did not start")), 5_000);
    child.stdout.on("data", (chunk) => {
      if (chunk.toString().includes("owner console running")) {
        clearTimeout(timer);
        resolve();
      }
    });
    child.once("exit", (code) => reject(new Error(`server exited early with ${code}`)));
  });
}

test("streaming risk engine recognizes every monitored high-risk class", () => {
  const cases = [
    ["Please update the bank account on our invoice.", "payment_account_change"],
    ["Read me the MFA verification code.", "credential_mfa_request"],
    ["Install TeamViewer so I can get remote access.", "remote_access"],
    ["You must do this immediately or your account will be suspended.", "coercive_urgency"],
  ];
  for (const [text, expectedSignal] of cases) {
    const result = assessTranscriptChunk({ sessionId: `class-${expectedSignal}`, role: "caller", text });
    assert.equal(result.alerts.length, 1);
    assert.ok(result.alerts[0].signals.some(({ id }) => id === expectedSignal));
  }
});

test("verified handoff transcript creates the required live payment alert and fails open", async (t) => {
  const child = spawn(process.execPath, ["server.mjs"], {
    cwd: new URL("..", import.meta.url),
    env: { ...process.env, PORT: String(PORT), MONITOR_WEBHOOK_SECRET: SECRET, ELEVENLABS_API_KEY: "test-only" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  t.after(() => child.kill());
  await waitForServer(child);

  const eventsResponse = await fetch(`${BASE}/api/monitor/events`);
  assert.equal(eventsResponse.status, 200);
  const eventReader = eventsResponse.body.getReader();
  const decoder = new TextDecoder();
  let streamedEvents = "";

  const rejected = await post("/api/webhooks/handoff", { session_id: "bad-signature" }, "wrong-secret");
  assert.equal(rejected.status, 401);

  const handoff = await post("/api/webhooks/handoff", {
    session_id: "supplier-demo-1",
    disclosure_given: true,
  });
  assert.equal(handoff.status, 202);

  await post("/api/webhooks/transcript", {
    session_id: "supplier-demo-1",
    sequence: 1,
    role: "caller",
    text: "This is Morgan from Harbour Coffee Roasters about tomorrow's expected coffee-bean delivery, reference HCR-204. Could I speak with the café owner?",
  });
  await post("/api/webhooks/transcript", {
    session_id: "supplier-demo-1",
    sequence: 2,
    role: "staff",
    text: "Thanks Morgan, what did you need to confirm?",
  });
  const riskResponse = await post("/api/webhooks/transcript", {
    session_id: "supplier-demo-1",
    sequence: 3,
    role: "caller",
    text: "Our payment details have changed, so update the bank account today or the delivery may be cancelled.",
  });
  assert.equal(riskResponse.status, 202);

  const state = await fetch(`${BASE}/api/monitor/state`).then((response) => response.json());
  assert.equal(state.handoffStatus, "connected");
  assert.equal(state.monitorStatus, "monitoring");
  assert.equal(state.disclosureGiven, true);
  assert.equal(state.aiMuted, true);
  assert.equal(state.transcript.length, 3);
  assert.equal(state.alerts.length, 1);
  assert.equal(state.alerts[0].severity, "high");
  assert.equal(state.alerts[0].recommendedAction, "Do not change payment details; verify using the registered supplier contact.");
  assert.match(state.alerts[0].evidence, /payment details have changed/i);

  while (!streamedEvents.includes("event: risk-alert")) {
    const { value, done } = await eventReader.read();
    if (done) break;
    streamedEvents += decoder.decode(value, { stream: true });
  }
  assert.match(streamedEvents, /event: risk-alert/);
  assert.match(streamedEvents, /Do not change payment details; verify using the registered supplier contact\./);
  await eventReader.cancel();

  const failedMonitor = await post("/api/webhooks/monitor-failure", {
    session_id: "supplier-demo-1",
    reason: "carrier transcription stream unavailable",
  });
  assert.equal(failedMonitor.status, 202);
  const degraded = await failedMonitor.json();
  assert.equal(degraded.handoffStatus, "connected");
  assert.equal(degraded.monitorStatus, "degraded");
});
