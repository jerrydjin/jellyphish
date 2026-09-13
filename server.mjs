import { createServer } from "node:http";
import { createHmac, timingSafeEqual } from "node:crypto";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { assessTranscriptChunk, signalCatalog } from "./risk-engine.mjs";

const ROOT = fileURLToPath(new URL(".", import.meta.url));
const WEB_ROOT = join(ROOT, "web");
const AGENT_ID = process.env.ELEVENLABS_AGENT_ID || "agent_1101m2b4acdnedz9y2yky3w9vwfm";
const PORT = Number(process.env.PORT || 4173);

const envText = await readFile(join(ROOT, ".env"), "utf8").catch(() => "");
const env = Object.fromEntries(
  envText
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#") && line.includes("="))
    .map((line) => {
      const index = line.indexOf("=");
      return [line.slice(0, index), line.slice(index + 1).replace(/^['"]|['"]$/g, "")];
    }),
);
const API_KEY = process.env.ELEVENLABS_API_KEY || env.ELEVENLABS_API_KEY;
const WEBHOOK_SECRET = process.env.MONITOR_WEBHOOK_SECRET || env.MONITOR_WEBHOOK_SECRET;

const monitorSessions = new Map();
const monitorClients = new Set();
let activeMonitorId = null;

function newMonitorSession(sessionId, source = "telephony-sidecar") {
  const session = {
    sessionId,
    source,
    handoffStatus: "connected",
    monitorStatus: "monitoring",
    disclosureGiven: false,
    aiMuted: true,
    startedAt: Date.now(),
    updatedAt: Date.now(),
    transcript: [],
    alerts: [],
    seenSignals: new Set(),
    error: null,
  };
  monitorSessions.set(sessionId, session);
  activeMonitorId = sessionId;
  return session;
}

function getMonitorSession(sessionId, source) {
  return monitorSessions.get(sessionId) || newMonitorSession(sessionId, source);
}

function serializeMonitor(session = monitorSessions.get(activeMonitorId)) {
  if (!session) return { status: "idle", signalCatalog: signalCatalog() };
  return {
    sessionId: session.sessionId,
    source: session.source,
    handoffStatus: session.handoffStatus,
    monitorStatus: session.monitorStatus,
    disclosureGiven: session.disclosureGiven,
    aiMuted: session.aiMuted,
    startedAt: session.startedAt,
    updatedAt: session.updatedAt,
    transcript: session.transcript,
    alerts: session.alerts,
    error: session.error,
    signalCatalog: signalCatalog(),
  };
}

function publishMonitor(session, event = "monitor") {
  const payload = `event: ${event}\ndata: ${JSON.stringify(serializeMonitor(session))}\n\n`;
  for (const client of monitorClients) client.write(payload);
}

function beginHandoff(payload, source = "verified-webhook") {
  const sessionId = String(payload.session_id || "").trim();
  if (!sessionId) throw new Error("session_id is required");
  const session = getMonitorSession(sessionId, source);
  session.source = source;
  session.handoffStatus = "connected";
  session.monitorStatus = "monitoring";
  session.disclosureGiven = payload.disclosure_given === true;
  session.aiMuted = true;
  session.updatedAt = Date.now();
  session.error = null;
  publishMonitor(session, "handoff");
  return session;
}

function ingestTranscript(payload, source = "verified-webhook") {
  const sessionId = String(payload.session_id || "").trim();
  const role = String(payload.role || "").toLowerCase();
  const text = String(payload.text || "").replace(/\s+/g, " ").trim();
  if (!sessionId || !["caller", "staff"].includes(role) || !text) throw new Error("session_id, caller/staff role, and text are required");
  const session = getMonitorSession(sessionId, source);
  session.source = source;
  session.updatedAt = Date.now();
  session.aiMuted = true;
  session.transcript.push({ id: `${sessionId}:${payload.sequence ?? session.transcript.length}`, role, text, receivedAt: session.updatedAt });
  if (session.transcript.length > 160) session.transcript.splice(0, session.transcript.length - 160);
  const assessment = assessTranscriptChunk({ sessionId, role, text, seenSignals: session.seenSignals });
  session.seenSignals = assessment.seenSignals;
  session.alerts.push(...assessment.alerts);
  publishMonitor(session, assessment.alerts.length ? "risk-alert" : "transcript");
  return session;
}

function markMonitorFailure(payload, source = "verified-webhook") {
  const sessionId = String(payload.session_id || "").trim();
  if (!sessionId) throw new Error("session_id is required");
  const session = getMonitorSession(sessionId, source);
  session.handoffStatus = "connected";
  session.monitorStatus = "degraded";
  session.aiMuted = true;
  session.error = String(payload.reason || "Monitoring sidecar unavailable").slice(0, 240);
  session.updatedAt = Date.now();
  publishMonitor(session, "monitor-failure");
  return session;
}

async function readBody(req) {
  const chunks = [];
  let bytes = 0;
  for await (const chunk of req) {
    bytes += chunk.length;
    if (bytes > 64 * 1024) throw new Error("Webhook body exceeds 64 KB");
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

function verifyWebhook(rawBody, timestamp, signature) {
  if (!WEBHOOK_SECRET) return { ok: false, status: 503, error: "MONITOR_WEBHOOK_SECRET is not configured" };
  const unix = Number(timestamp);
  if (!Number.isFinite(unix) || Math.abs(Date.now() / 1000 - unix) > 300) return { ok: false, status: 401, error: "Webhook timestamp is invalid or stale" };
  const suppliedHex = String(signature || "").replace(/^sha256=/, "");
  if (!/^[a-f0-9]{64}$/i.test(suppliedHex)) return { ok: false, status: 401, error: "Webhook signature is invalid" };
  const expected = createHmac("sha256", WEBHOOK_SECRET).update(`${timestamp}.${rawBody}`).digest();
  const supplied = Buffer.from(suppliedHex, "hex");
  return timingSafeEqual(expected, supplied) ? { ok: true } : { ok: false, status: 401, error: "Webhook signature is invalid" };
}

function runDemoHandoff() {
  const sessionId = `demo_${Date.now()}`;
  const session = beginHandoff({ session_id: sessionId, disclosure_given: true }, "local-demo");
  const chunks = [
    [120, "caller", "Hi, this is Morgan from Harbour Coffee Roasters about tomorrow’s expected coffee-bean delivery, reference HCR-204. Could I speak with the café owner?"],
    [520, "staff", "Hi Morgan, I’m the café owner. What did you need to confirm?"],
    [920, "caller", "Our payment details have changed, so please update the bank account on file."],
    [1320, "caller", "It needs to be done today or your delivery may be cancelled."],
  ];
  for (const [delay, role, text] of chunks) setTimeout(() => ingestTranscript({ session_id: sessionId, role, text }, "local-demo"), delay);
  return session;
}

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".json": "application/json; charset=utf-8",
};

function json(res, status, body) {
  res.writeHead(status, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" });
  res.end(JSON.stringify(body));
}

function valueOf(field) {
  if (field && typeof field === "object" && "value" in field) return field.value;
  return field ?? null;
}

function dataCollection(raw = {}) {
  return Object.fromEntries(Object.entries(raw).map(([key, value]) => [key, valueOf(value)]));
}

async function elevenLabs(path) {
  if (!API_KEY) throw new Error("ELEVENLABS_API_KEY is missing from .env");
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 12_000);
  try {
    const response = await fetch(`https://api.elevenlabs.io${path}`, {
      headers: { "xi-api-key": API_KEY },
      signal: controller.signal,
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(body.detail?.message || body.detail || `ElevenLabs returned ${response.status}`);
    return body;
  } finally {
    clearTimeout(timeout);
  }
}

async function listConversations() {
  const fields = ["route", "caller_name", "claimed_company", "reference", "requested_action", "risk_signals", "outcome", "distraction_turns"];
  const params = new URLSearchParams({ agent_id: AGENT_ID, page_size: "30", summary_mode: "include", sort_direction: "desc" });
  fields.forEach((field) => params.append("data_collection_ids", field));
  const result = await elevenLabs(`/v1/convai/conversations?${params}`);
  return (result.conversations || []).map((item) => ({
    id: item.conversation_id,
    agentName: item.agent_name,
    startedAt: item.start_time_unix_secs,
    duration: item.call_duration_secs || 0,
    messageCount: item.message_count || 0,
    status: item.status,
    success: item.call_successful,
    title: item.call_summary_title,
    summary: item.transcript_summary,
    source: item.conversation_initiation_source,
    direction: item.direction,
    data: dataCollection(item.data_collection_results),
  }));
}

async function getConversation(id) {
  if (!/^conv_[a-zA-Z0-9]+$/.test(id)) throw new Error("Invalid conversation id");
  const item = await elevenLabs(`/v1/convai/conversations/${id}`);
  const analysis = item.analysis || {};
  return {
    id: item.conversation_id,
    agentName: item.agent_name,
    status: item.status,
    startedAt: item.metadata?.start_time_unix_secs || 0,
    duration: item.metadata?.call_duration_secs || 0,
    source: item.metadata?.conversation_initiation_source,
    terminationReason: item.metadata?.termination_reason,
    success: analysis.call_successful,
    score: analysis.call_success_score,
    title: analysis.call_summary_title,
    summary: analysis.transcript_summary,
    sentiment: analysis.sentiment_analysis?.overall_label,
    data: dataCollection(analysis.data_collection_results),
    transcript: (item.transcript || [])
      .filter((turn) => (turn.role === "agent" || turn.role === "user") && typeof turn.message === "string" && turn.message.trim())
      .map((turn) => ({ role: turn.role, message: turn.message, time: turn.time_in_call_secs || 0, interrupted: Boolean(turn.interrupted) })),
  };
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url || "/", `http://${req.headers.host || "localhost"}`);
  try {
    if (req.method === "GET" && url.pathname === "/api/monitor/events") {
      res.writeHead(200, {
        "content-type": "text/event-stream; charset=utf-8",
        "cache-control": "no-store",
        connection: "keep-alive",
        "x-accel-buffering": "no",
      });
      monitorClients.add(res);
      res.write(`event: monitor\ndata: ${JSON.stringify(serializeMonitor())}\n\n`);
      const heartbeat = setInterval(() => res.write(": heartbeat\n\n"), 20_000);
      req.on("close", () => { clearInterval(heartbeat); monitorClients.delete(res); });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/monitor/state") {
      return json(res, 200, serializeMonitor());
    }
    if (req.method === "POST" && url.pathname === "/api/monitor/demo") {
      return json(res, 202, serializeMonitor(runDemoHandoff()));
    }
    if (req.method === "POST" && url.pathname.startsWith("/api/webhooks/")) {
      const rawBody = await readBody(req);
      const verified = verifyWebhook(rawBody, req.headers["x-jellyphish-timestamp"], req.headers["x-jellyphish-signature"]);
      if (!verified.ok) return json(res, verified.status, { error: verified.error });
      let payload;
      try { payload = JSON.parse(rawBody); } catch { return json(res, 400, { error: "Webhook body must be JSON" }); }
      if (url.pathname === "/api/webhooks/handoff") return json(res, 202, serializeMonitor(beginHandoff(payload)));
      if (url.pathname === "/api/webhooks/transcript") return json(res, 202, serializeMonitor(ingestTranscript(payload)));
      if (url.pathname === "/api/webhooks/monitor-failure") return json(res, 202, serializeMonitor(markMonitorFailure(payload)));
      return json(res, 404, { error: "Webhook not found" });
    }
    if (req.method === "GET" && url.pathname === "/api/health") {
      return json(res, 200, {
        ok: true,
        agentId: AGENT_ID,
        apiConfigured: Boolean(API_KEY),
        monitorWebhookConfigured: Boolean(WEBHOOK_SECRET),
        monitoredTransferReady: Boolean(WEBHOOK_SECRET && (process.env.HUMAN_TRANSFER_NUMBER || env.HUMAN_TRANSFER_NUMBER)),
      });
    }
    if (req.method === "GET" && url.pathname === "/api/conversations") {
      return json(res, 200, { conversations: await listConversations(), refreshedAt: Date.now() });
    }
    const detailMatch = req.method === "GET" && url.pathname.match(/^\/api\/conversations\/(conv_[a-zA-Z0-9]+)$/);
    if (detailMatch) return json(res, 200, await getConversation(detailMatch[1]));
    if (req.method !== "GET") return json(res, 405, { error: "Method not allowed" });

    const relative = url.pathname === "/" ? "index.html" : normalize(url.pathname).replace(/^[/\\]+/, "");
    const filePath = join(WEB_ROOT, relative);
    if (!filePath.startsWith(`${WEB_ROOT}/`)) return json(res, 404, { error: "Not found" });
    const content = await readFile(filePath);
    res.writeHead(200, { "content-type": MIME[extname(filePath)] || "application/octet-stream" });
    res.end(content);
  } catch (error) {
    if (url.pathname.startsWith("/api/")) return json(res, 502, { error: error.message || "Upstream request failed" });
    json(res, 404, { error: "Not found" });
  }
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`Studio Sol owner console running at http://localhost:${PORT}`);
});
