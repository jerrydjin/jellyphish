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

## Safety behavior

- `risk-core` assesses caller transcript chunks with the same rules used by `assess_call`.
- The AI is marked silent after handoff.
- Alerts carry the triggering evidence and a fixed recommended action.
- Monitoring failure sets `monitorStatus` to `degraded` and preserves `handoffStatus: connected`.
- The local **Run monitored-handoff demo** button still exercises the full SSE path.

## Infrastructure boundary

A real sidecar still needs a Twilio or SIP conference media stream. No phone number, carrier credentials, or human destination is included in this repository. Until those exist, the browser voice widget and timed handoff simulation are the verifiable demo surfaces.
