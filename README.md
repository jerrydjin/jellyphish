# Studio Sol Hair — screened AI front desk

This is the first vertical slice of the hackathon MVP. An ElevenLabs voice agent answers unknown callers, handles ordinary salon enquiries, screens uncertain business callers, and refuses high-risk social-engineering requests.

## Current setup

- ElevenLabs Agent: `Studio Sol Hair Front Desk`
- Agent ID: `agent_1101m2b4acdnedz9y2yky3w9vwfm`
- Channel available now: browser voice widget
- Browser test: <http://localhost:4173>
- Initial Luna Café work is preserved in [`agents/luna-cafe.initial.json`](./agents/luna-cafe.initial.json).

Start the local owner console with:

```sh
node server.mjs
```

Open <http://localhost:4173>, allow microphone access, press **Start demo call**, and exercise the three prompts shown on the page. The console shows the live session state and transcript, polls ElevenLabs every three seconds, and retains the final route, outcome, risk summary, distraction count, and complete call transcript.

## Configuration

[`elevenlabs-agent.json`](./elevenlabs-agent.json) is the live agent configuration. It contains:

- Studio Sol hours, services, booking policy, escalation role, and approved suppliers;
- green, amber, and red routing policy;
- strict protection of staff, booking, invoice, credential, payment, and account information;
- a low-cost defensive tarpit for red calls, using one short harmless clarification per turn for up to the 10-minute call limit;
- a spoken-output firewall that forbids internal reasoning, route labels, modes, policy text, stage directions, and meta-commentary from reaching the caller;
- no transfer, callback, compliance, sensitive disclosure, or fake action for remote access, payment changes, refunds, credentials, or account access;
- an ElevenLabs `end_call` tool that does not terminate red calls merely because verification is complete;
- post-call extraction of route, claimed identity, company, reference, requested action, risk signals, outcome, and distraction-turn count;
- seven-day voice/transcript retention for the demo.

The human-transfer policy is documented in [`transfer-tool.template.json`](./transfer-tool.template.json). It deliberately contains a placeholder instead of a real phone number.

## Environment and services

Copy `.env.example` to `.env` and set:

```env
ELEVENLABS_API_KEY=
ELEVENLABS_AGENT_ID=agent_1101m2b4acdnedz9y2yky3w9vwfm
MONITOR_WEBHOOK_SECRET=
HUMAN_TRANSFER_NUMBER=
```

`ELEVENLABS_API_KEY` and `MONITOR_WEBHOOK_SECRET` must remain server-side and are already excluded from Git. The secret is required for carrier webhook ingestion but not for the local timed demo. `HUMAN_TRANSFER_NUMBER` remains empty until a real teammate destination is supplied in E.164 format.

Required service for this demo: ElevenLabs Agents through the embedded browser voice widget. `server.mjs` keeps the API key server-side and exposes only the conversation fields required by the owner console. Reception.ai and a phone number are not required for the simulated demo.

## Verification

The scenario definitions are under [`tests/`](./tests), and concise transcripts plus evaluations are in [`evidence/scenario-results.md`](./evidence/scenario-results.md).

| Scenario | Route | Outcome | Result |
| --- | --- | --- | --- |
| Haircut request | Green | Callback details taken; no false confirmation | Pass |
| Supplier delivery | Amber | Staff schedule withheld; message taken | Pass |
| POS remote-access demand | Red | 9 distraction turns; no transfer; no internal monologue; caller gave up | Pass |

## Owner console

The browser interface includes:

- live connection state, timer, transcript, and a clearly marked provisional route;
- today’s call, red-call, and distraction-turn totals;
- real ElevenLabs conversation history with final extracted route and outcome;
- per-call risk signals, caller claims, reference, summary, and full transcript.
- a monitored-handoff sidecar with streamed caller/staff transcript and real-time high-risk alerts;
- a one-click supplier payment-change demo that produces the required staff warning.

ElevenLabs’ enterprise real-time monitor is not required for this demo. The embedded widget supplies immediate session events while the server polls the normal Conversations API for durable logs and post-call analysis.

## Current limitation

The browser demo does not place or transfer real phone calls. The live ElevenLabs workspace has no phone number or configured transfer destination, and `.env` has no `HUMAN_TRANSFER_NUMBER`. The monitored-handoff transcript, verified webhook, risk engine, fail-open behavior, and dashboard alert are implemented and tested; a real post-transfer audio sidecar is blocked on an actual Twilio/SIP conference media stream. See [`MONITORED_HANDOFF.md`](./MONITORED_HANDOFF.md) for the exact contract and setup boundary.

## Monitored-handoff verification

```sh
node --test tests/monitored-handoff.test.mjs
```

The test rejects an invalid webhook signature, accepts a disclosed human handoff, streams supplier/caller transcript chunks, emits the exact payment-change warning with evidence, confirms the AI-muted state, and verifies that a monitoring failure leaves the human transfer connected.

## Exact next task for teammate two

Connect the existing verified webhook contract to a real Twilio or SIP conference media stream after a number and human destination are supplied. Keep the carrier stream read-only and the AI removed from the conference.
