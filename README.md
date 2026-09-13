<p align="center"><img src="jellyphis-icon-white.png" alt="Jellyphish icon" width="160" /></p>

# Jellyphish — managed voice, deterministic policy

Jellyphish is a screened AI front desk for small businesses. ElevenLabs owns realtime voice and call controls; a Rust service makes the safety decisions, records events, and serves a small operator console.

```text
ElevenLabs voice agent
        ↓ webhook tools
Rust Axum API
        ↓
deterministic risk/policy engine
        ↓
SQLite event store + post-call summaries
        ↓
small operator dashboard
```

The language model may describe what the caller wants, but it cannot authorize an action. Before a transfer, it calls `assess_call`; the Rust response provides `allowed_actions` and the decisive `transfer_allowed` value. If the policy service is unavailable, the prompt fails safely to taking a message.

## Repository map

- `risk-core/` — pure Rust types, signal detection, and `evaluate()`; no network or database dependency.
- `server/` — Axum routes, SQLite event store, ElevenLabs client, signed webhooks, SSE monitoring, and static dashboard hosting.
- `web/` — intentionally plain HTML/CSS/JavaScript operator console.
- `tools/` — ElevenLabs webhook-tool templates for `assess_call` and `finish_call`.
- `elevenlabs-agent.json` and `agent-prompt.md` — voice-agent configuration and source prompt.
- `tests/*.json` — ElevenLabs conversation simulation scenarios.

## Run locally

Copy `.env.example` to `.env`, add a restricted ElevenLabs API key, then run:

```sh
cargo +stable run -p jellyphish-server
```

Open <http://localhost:4173>, allow microphone access, and start a demo call. The database is created at `data/jellyphish.db`.

The browser widget remains the fastest demo path. A phone number is required only for a real `transfer_to_number` call.

## Core API

### `GET /healthz`

Reports service and integration readiness without exposing secrets.

### `POST /tool/assess`

Accepts a typed `CallerContext`. The response is a deterministic `RiskDecision`:

```json
{
  "call_id": "conv_demo",
  "requested_action": "Install TeamViewer so support can access the till",
  "proposed_action": "transfer"
}
```

```json
{
  "risk_level": "red",
  "signals": ["remote_access_request"],
  "allowed_actions": ["safe_refusal", "bounded_distraction", "end_call"],
  "transfer_allowed": false,
  "proposed_action_allowed": false,
  "reasons": ["A high-risk request makes human transfer unsafe."]
}
```

### `POST /tool/finish`

Re-evaluates the supplied context and upserts a concise summary. It never accepts a model-supplied risk level as truth.

### `POST /webhooks/elevenlabs/post-call`

Validates `ElevenLabs-Signature: t=<unix>,v0=<hmac>`, stores an idempotent event metadata record, re-evaluates the transcript, and writes a summary. Full audio/transcripts remain in ElevenLabs rather than being copied into the event log.

The existing monitored-handoff contract remains at `/api/webhooks/*`; see [MONITORED_HANDOFF.md](./MONITORED_HANDOFF.md).

## Tests

```sh
cargo +stable fmt --all -- --check
cargo +stable test --workspace
cargo +stable clippy --workspace --all-targets -- -D warnings
```

The tests cover ordinary, unverified supplier, and high-risk transfer decisions; every high-risk class; HMAC validation; and the HTTP assessment contract.

## ElevenLabs setup

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

This repository does not include a Rust/WASM frontend, microservices, Kubernetes, a custom WebRTC stack, or Postgres. SQLite is enough for the single-business demo. A later multi-tenant deployment can move persistence behind the same `EventStore` boundary.

The browser widget cannot perform a real phone transfer. Conference transfer requires an ElevenLabs-compatible Twilio or SIP number and a real destination. The current prompt and transfer template require a fresh Rust approval before transfer, but ElevenLabs still executes the system tool; production assurance should also use platform guardrails and live integration testing.
