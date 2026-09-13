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

Browser demos use a second path: after the assistant gives the disclosure and calls `transfer_to_human`, the caller tab posts a WebRTC offer to `/api/handoff/{line}/ring`. The operator console for that business line rings like a staff line. Answering connects caller audio to the human, and live caller/staff speech writes into the same transcript panel. Handoff state and events are isolated by line, so the dental and barbershop consoles cannot answer one another's calls.

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

- `POST /api/handoff/{line}/ring`: SDP offer plus caller preview and a per-call caption capability
- `POST /api/handoff/{line}/signal`: SDP answer or ICE
- `POST /api/handoff/{line}/transcript`: live caller/staff speech; requires the caption capability and a connected or just-ended room
- `POST /api/handoff/{line}/transcribe`: bounded speech audio; requires the caption capability and a connected or just-ended room
- `POST /api/handoff/{line}/hangup`
- `GET /api/monitor/{line}/events`: line-scoped operator event stream
- `GET /api/handoff/{line}/{id}`

Open the operator console on one tab or machine, and the demo caller at `/phone/` on another. When Sol hands off, pick up on the home page.

## Safety behavior

- `risk-core` can raise a post-handoff alert from transcript chunks; it does not re-classify the original screening route.
- The AI is marked silent after handoff.
- Alerts carry the triggering evidence and a fixed recommended action.
- Monitoring failure sets `monitorStatus` to `degraded` and preserves `handoffStatus: connected`.

## Infrastructure boundary

A real sidecar still needs a Twilio or SIP conference media stream. No phone number, carrier credentials, or human destination is included in this repository. Until those exist, the browser staff line on the home page is the verifiable demo surface: call from `/phone/`, then pick up when Sol hands off.
