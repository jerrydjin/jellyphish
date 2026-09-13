# Rust rewrite verification

Date: 2026-09-13

## Automated checks

```sh
cargo +stable test --workspace
cargo +stable clippy --workspace --all-targets -- -D warnings
```

Result: all policy, HMAC, persistence-contract, and HTTP tests pass; Clippy reports no warnings.

The covered decisions are:

- ordinary haircut enquiry → green → transfer allowed;
- unverified supplier delivery → amber → transfer denied;
- payment/account change, credentials/MFA, remote access, and coercive urgency → red → transfer denied;
- a model-supplied claim that a supplier is already trusted is ignored by the HTTP boundary;
- agent-authored safety wording is not mistaken for caller risk;
- a changed ElevenLabs webhook body fails HMAC verification;
- `POST /tool/assess` returns the Rust decision rather than a model classification.

## Running-service check

The compiled Axum service was launched on a temporary localhost port and verified directly:

1. `/healthz` returned `service: jellyphish-rust`.
2. Green, amber, and red assessment requests returned their expected allowed actions.
3. `/tool/finish` persisted a Rust-derived red summary to SQLite.
4. `/api/summaries` returned the stored result.
5. `/` served the operator dashboard with HTTP 200.
6. `/api/monitor/demo` streamed the caller/staff transcript and emitted the exact payment-change warning while retaining `handoffStatus: connected`.

## Live infrastructure boundary

The Rust dashboard, SQLite store, signed webhook contracts, risk engine, and fail-open monitor path are working. An actual post-transfer phone-audio test still needs a Twilio/SIP number, a configured human E.164 destination, and a carrier conference-media stream. The browser widget cannot provide that phone conference.
