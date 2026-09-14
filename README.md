<p align="center"><img src="jellyphis-icon-white.png" alt="Jellyphish icon" width="160" /></p>

# Jellyphish

An AI front desk for small businesses that screens callers before they reach a human.

An ElevenLabs voice agent answers the call and rates the caller green, amber, or red. A Rust service records that rating, decides which actions are allowed (only safe callers get transferred), and serves a live operator dashboard.

```text
ElevenLabs voice agent (classifies the call)
        ↓ assessment tool records the route
Rust Axum API
        ↓
SQLite event store + post-call summaries
        ↓
small operator dashboard
```

If you're seeing this, our product should be on a live server.

## Quick start

```sh
cp .env.example .env        # add a restricted ElevenLabs API key
cargo +stable run -p jellyphish-server
```

| Page                  | URL                              |
| --------------------- | -------------------------------- |
| Barbershop dashboard  | <http://localhost:4173>          |
| Dental dashboard      | <http://localhost:4173/dental/>  |
| Caller phone          | <http://localhost:4173/phone/>   |

Open the phone in a second window, pick a business, and call. The call shows up live on that business's dashboard, and when the agent hands off, the staff line rings there. Using a real handset needs HTTPS (e.g. ngrok) for microphone access.

## Layout

| Path                    | What it is                                              |
| ----------------------- | ------------------------------------------------------- |
| `server/`               | Axum API, SQLite store, webhooks, live SSE, dashboard   |
| `risk-core/`            | Risk levels and the actions each one allows             |
| `web/`                  | Plain HTML/JS dashboards and phone simulator            |
| `tools/`                | ElevenLabs tool templates (`assess_call`, `finish_call`) |
| `agent-prompt*.md`, `elevenlabs-agent*.json` | Agent configs (Barbershop, Dental) |
| `scripts/`              | Deploy prompts and attach tools to live agents          |
| `tests/`                | Node tests and conversation scenarios                   |

## Key endpoints

- `GET /healthz`: service and integration status
- `POST /tool/assess`: the agent reports its risk level and gets back the allowed actions
- `POST /tool/finish`: stores the final rating and a call summary
- `POST /webhooks/elevenlabs/post-call`: signed post-call webhook

Human handoff webhooks are documented in [MONITORED_HANDOFF.md](./MONITORED_HANDOFF.md).

## Tests

```sh
cargo +stable test --workspace
cargo +stable clippy --workspace --all-targets -- -D warnings
node --test tests/call-lifecycle.test.mjs tests/agent-routing.test.mjs
```

## Deploying agents

```sh
node scripts/deploy-agent-safety.mjs
node scripts/deploy-dental-agent.mjs
PUBLIC_BASE_URL=https://your-origin.example node scripts/apply-policy-tools.mjs
node scripts/apply-transfer.mjs   # only once a real E.164 number exists
```

Point the ElevenLabs post-call webhook at `/webhooks/elevenlabs/post-call` and set `ELEVENLABS_WEBHOOK_SECRET`.

> **Note:** A browser session can't make a real phone transfer. That needs a Twilio or SIP number connected to ElevenLabs.
