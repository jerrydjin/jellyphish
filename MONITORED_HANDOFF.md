# Monitored human handoff

The handoff sidecar is now part of the Rust Axum service. It remains read-only: a monitoring or transcription failure marks the dashboard degraded but never disconnects the human conference.

```text
Caller -> ElevenLabs -> conference transfer -> human
                       |
                       +-> carrier/SIP transcription sidecar
                               |
                               +-> signed Rust webhook
                                       |
                                       +-> risk-core::evaluate
                                               |
                                               +-> SSE dashboard alert
```

Browser demos use a second path: after Sol gives the disclosure and calls `transfer_to_human`, the caller tab posts a WebRTC offer to `/api/handoff/ring`. The operator console on the home page rings like a staff line. Answering connects caller audio to the human, and live caller/staff speech writes into the same transcript panel.

## Webhook contract

- `POST /api/webhooks/handoff`: `session_id`, `disclosure_given`
- `POST /api/webhooks/transcript`: `session_id`, increasing `sequence`, `role` (`caller` or `staff`), `text`
- `POST /api/webhooks/monitor-failure`: `session_id`, concise `reason`

Every request includes:

```text
X-Jellyphish-Timestamp: <unix seconds>
X-Jellyphish-Signature: sha256=<HMAC-SHA256 of "<timestamp>.<raw JSON body>">
```

The service rejects missing, malformed, mismatched, and more-than-five-minute-old signatures. Set the shared secret in `MONITOR_WEBHOOK_SECRET`.

## Browser staff line

Local demo endpoints do not require HMAC:

- `POST /api/handoff/ring`: SDP offer plus caller preview
- `POST /api/handoff/signal`: SDP answer or ICE
- `POST /api/handoff/transcript`: live caller/staff speech
- `POST /api/handoff/hangup`
- `GET /api/handoff/{id}`

Open the operator console on one tab or machine, and the demo caller on another. When Sol hands off, pick up on the home page. The simulated **Run monitored handoff** button still exercises the SSE alert path without audio.

## Safety behavior

- `risk-core` assesses caller transcript chunks with the same rules used by `assess_call`.
- The AI is marked silent after handoff.
- Alerts carry the triggering evidence and a fixed recommended action.
- Monitoring failure sets `monitorStatus` to `degraded` and preserves `handoffStatus: connected`.

## Infrastructure boundary

A real sidecar still needs a Twilio or SIP conference media stream. No phone number, carrier credentials, or human destination is included in this repository. Until those exist, the browser staff line and timed handoff simulation are the verifiable demo surfaces.
