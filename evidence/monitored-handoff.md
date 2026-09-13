# Monitored-handoff evidence

Date: 2026-09-13

## Automated end-to-end result

Command:

```sh
node --test tests/monitored-handoff.test.mjs
```

Result: **Pass** — two tests, zero failures. The first covers every configured high-risk signal class; the second covers the full webhook-to-live-dashboard path below.

The test starts the real local owner-console server, opens its Server-Sent Events stream, and verifies the full sidecar path:

1. An incorrectly signed handoff webhook is rejected with HTTP 401.
2. A correctly signed handoff states that disclosure was given and the AI is silent.
3. Caller and staff transcript chunks are accepted for an expected coffee-bean delivery.
4. The caller says their payment details changed and pressures the owner to act today.
5. The risk engine emits a `high` alert through the live SSE stream with the evidence and exact recommended action:

> Do not change payment details; verify using the registered supplier contact.

6. A simulated transcription-sidecar failure changes monitoring to `degraded` while the transfer remains `connected`.

## Live infrastructure boundary

The dashboard sidecar, signed webhook contract, streaming risk engine, and fail-open path are working. An actual post-transfer phone-audio test is blocked because the workspace has no Twilio/SIP number, no configured human E.164 destination, and no carrier conference-media stream. The browser widget cannot provide that phone conference.
