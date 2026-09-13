import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

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
    if (req.method === "GET" && url.pathname === "/api/health") {
      return json(res, 200, { ok: true, agentId: AGENT_ID, apiConfigured: Boolean(API_KEY) });
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
