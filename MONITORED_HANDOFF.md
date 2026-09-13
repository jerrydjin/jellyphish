# Monitored human handoff

## What is implemented

The existing ElevenLabs browser voice agent, green/amber/red routing, conference-transfer template, owner console, and post-call analysis remain in place.

The new local sidecar adds:

- a deterministic streaming risk engine for payment/account changes, credentials or MFA, remote access, and coercive urgency;
- HMAC-verified handoff, transcript, and monitoring-failure webhooks;
- a Server-Sent Events stream that pushes transcript chunks and high-risk alerts to the existing dashboard;
- a mandatory pre-transfer disclosure in the agent prompt;
- an explicit post-handoff AI-silent state;
- fail-open monitoring state: sidecar failure marks monitoring as degraded while the human transfer stays connected;
- a local timed demo that reproduces the supplier payment-change scenario.

## Live-monitor architecture

```text
Caller -> ElevenLabs agent -> conference transfer -> human
                              |
                              +-> carrier/SIP read-only media sidecar
                                      |
                                      +-> speech-to-text adapter
                                              |
                                              +-> signed transcript webhook
                                                      |
                                                      +-> risk-engine.mjs
                                                              |
                                                              +-> SSE dashboard alert
```

The carrier adapter must treat monitoring as fire-and-forget. A sidecar connection or transcription failure must never cancel the conference transfer.

## Webhook contract

Send JSON to:

- `POST /api/webhooks/handoff` with `session_id` and `disclosure_given`;
- `POST /api/webhooks/transcript` with `session_id`, monotonically increasing `sequence`, `role` (`caller` or `staff`), and `text`;
- `POST /api/webhooks/monitor-failure` with `session_id` and a concise `reason`.

Every request must include:

```text
X-Jellyphish-Timestamp: <unix seconds>
X-Jellyphish-Signature: sha256=<HMAC-SHA256 of "<timestamp>.<raw JSON body>">
```

The server rejects missing, malformed, mismatched, or more-than-five-minute-old signatures. Credentials stay in environment variables.

## Environment

```env
ELEVENLABS_API_KEY=
ELEVENLABS_AGENT_ID=agent_1101m2b4acdnedz9y2yky3w9vwfm
HUMAN_TRANSFER_NUMBER=+61412345678
MONITOR_WEBHOOK_SECRET=
```

Run the dashboard with `node server.mjs`. Run the verified sidecar test with `node --test tests/monitored-handoff.test.mjs`.

## Infrastructure blocker

There is no live phone or SIP provider in this repository or ElevenLabs workspace: the live agent has zero phone numbers, no configured transfer tool, and `.env` has no human destination. The current channel is the browser widget, where ElevenLabs' `transfer_to_number` tool is unavailable.

ElevenLabs conference transfer removes the AI from the conference, which correctly guarantees that it cannot speak after handoff. However, ElevenLabs' own real-time monitor is enterprise-only, streams text and metadata rather than raw audio, and does not provide a post-transfer caller/human media bridge. A real read-only audio/transcript sidecar therefore requires the actual carrier or SIP conference to fork both participants' media to a transcription adapter. No such existing telephony stack is available to preserve or configure here.

To unblock the true phone test, provide an ElevenLabs-compatible Twilio or SIP number, a human destination in E.164 format, and carrier credentials/configuration for a read-only conference media stream. Then point that adapter at the verified webhooks above and apply `scripts/apply-transfer.mjs`.
