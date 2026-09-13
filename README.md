<p align="center"><img src="jellyphis-icon-white.png" alt="Jellyphish icon" width="160" /></p>

# Jellyphish — managed voice, agent-judged routing

Jellyphish is a screened AI front desk for small businesses. ElevenLabs owns realtime voice and call controls; Sol silently classifies each caller as green, amber, or red. A Rust service records that judgment, stores events, and serves a small operator console.

```text
ElevenLabs voice agent (classifies the call)
        ↓ assessment tool records the route
Rust Axum API
        ↓
SQLite event store + post-call summaries
        ↓
small operator dashboard
```

Sol is the judge. Before a transfer it records its current `risk_level` with `assess_call`; the Rust response echoes allowed actions for that route. If the assessment tool is unavailable, the prompt fails safely to taking a message.

## Repository map

- `risk-core/` — route types and the action list for an agent-supplied green/amber/red judgment.
- `server/` — Axum routes, SQLite event store, ElevenLabs client, signed webhooks, SSE monitoring, and static dashboard hosting.
- `web/` — intentionally plain HTML/CSS/JavaScript operator console.
- `tools/` — ElevenLabs webhook-tool templates for `assess_call` and `finish_call`.
- `elevenlabs-agent.json` and `agent-prompt.md` — Barbershop (Konner) voice-agent configuration.
- `elevenlabs-agent-dental.json` and `agent-prompt-dental.md` — Evan & Kevin Dental (Net) voice-agent configuration.
- `tests/*.json` — ElevenLabs conversation simulation scenarios.

## Run locally

Copy `.env.example` to `.env`, add a restricted ElevenLabs API key, then run:

```sh
cargo +stable run -p jellyphish-server
```

Open the Barbershop inbound console at <http://localhost:4173>, or Evan & Kevin Dental at <http://localhost:4173/dental/>. Open the caller's phone at <http://localhost:4173/phone/> in a second window. Tap **Barbershop** or **Evan & Kevin Dental**, then **call**, and allow microphone access. The call appears live on that business's dashboard. When the assistant hands a screened caller to a human, the dashboard staff line rings; answer there and the assistant goes silent. The database is created at `data/jellyphish.db`.

The phone simulator owns the realtime ElevenLabs session, just like a real caller. It publishes each call snapshot (status, caller number, mute state, transcript) to `POST /api/live/{line}`, and every dashboard for that line mirrors it over `GET /api/live/{line}/events` (SSE). Human handoff rooms and monitor events are also isolated by line. The caller browser is the sole post-handoff caption producer: voice activity gates bounded Scribe uploads, and a per-call capability prevents unrelated clients from injecting captions. Barbershop answers on `studio-sol`; dental answers on `dental-clinic` once `ELEVENLABS_DENTAL_AGENT_ID` is set. Other contacts ring out with no answer. The microphone requires a secure origin, so a real handset needs HTTPS (for example an ngrok URL) rather than a LAN IP. A phone number is required only for a real `transfer_to_number` call.

## Core API

### `GET /healthz`

Reports service and integration readiness without exposing secrets.

### `POST /tool/assess`

Accepts the agent's current `CallerContext`, including its `risk_level`. The response is the action list for that route:

```json
{
  "call_id": "conv_demo",
  "risk_level": "red",
  "requested_action": "Install TeamViewer so support can access the till",
  "proposed_action": "transfer"
}
```

```json
{
  "risk_level": "red",
  "signals": [],
  "allowed_actions": ["safe_refusal", "bounded_distraction", "end_call"],
  "transfer_allowed": false,
  "proposed_action_allowed": false,
  "reasons": ["The agent classified this caller as high risk."]
}
```

### `POST /tool/finish`

Stores the agent's classification and a concise summary.

### `POST /webhooks/elevenlabs/post-call`

Validates `ElevenLabs-Signature: t=<unix>,v0=<hmac>`, stores an idempotent event metadata record, and writes a summary using the agent's collected `route`. Full audio/transcripts remain in ElevenLabs rather than being copied into the event log.

The existing monitored-handoff contract remains at `/api/webhooks/*`; see [MONITORED_HANDOFF.md](./MONITORED_HANDOFF.md).

## Tests

```sh
cargo +stable fmt --all -- --check
cargo +stable test --workspace
cargo +stable clippy --workspace --all-targets -- -D warnings
node --test tests/call-lifecycle.test.mjs tests/agent-routing.test.mjs
```

The tests cover ordinary, unverified supplier, and high-risk transfer decisions; every high-risk class; HMAC validation; and the HTTP assessment contract.

## ElevenLabs setup

Deploy prompt and spoken-output guardrail changes to the live agent with:

```sh
node scripts/deploy-agent-safety.mjs
node scripts/deploy-dental-agent.mjs
```

Webhook tools call external APIs, while `transfer_to_number` and `end_call` remain ElevenLabs system tools. To attach the two policy tools to the live agent, expose this server on a public HTTPS origin and run:

```sh
PUBLIC_BASE_URL=https://your-origin.example node scripts/apply-policy-tools.mjs
```

Then apply the transfer template only after a real E.164 teammate destination exists:

```sh
node scripts/apply-transfer.mjs
```

Configure the workspace post-call transcription webhook to send to `/webhooks/elevenlabs/post-call` and put its generated secret in `ELEVENLABS_WEBHOOK_SECRET`.

## Deliberate boundaries

This repository does not include a Rust/WASM frontend, microservices, Kubernetes, a custom WebRTC stack, or Postgres. SQLite is enough for the demo. A later multi-tenant deployment can move persistence behind the same `EventStore` boundary.

A browser demo session cannot perform a real phone transfer. Conference transfer requires an ElevenLabs-compatible Twilio or SIP number and a real destination. The current prompt and transfer template require a fresh Rust approval before transfer, but ElevenLabs still executes the system tool; production assurance should also use platform guardrails and live integration testing.
