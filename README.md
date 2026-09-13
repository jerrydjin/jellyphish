# Studio Sol Hair — screened AI front desk

This is the first vertical slice of the hackathon MVP. An ElevenLabs voice agent answers unknown callers, handles ordinary salon enquiries, screens uncertain business callers, and refuses high-risk social-engineering requests.

## Current setup

- ElevenLabs Agent: `Studio Sol Hair Front Desk`
- Agent ID: `agent_1101m2b4acdnedz9y2yky3w9vwfm`
- Channel available now: browser voice widget
- Browser test: <http://localhost:4173>
- Initial Luna Café work is preserved in [`agents/luna-cafe.initial.json`](./agents/luna-cafe.initial.json).

Start the local test page with:

```sh
python3 -m http.server 4173 --directory web
```

Allow microphone access, press **Start a call**, and exercise the three prompts shown on the page.

## Configuration

[`elevenlabs-agent.json`](./elevenlabs-agent.json) is the live agent configuration. It contains:

- Studio Sol hours, services, booking policy, escalation role, and approved suppliers;
- green, amber, and red routing policy;
- strict protection of staff, booking, invoice, credential, payment, and account information;
- a low-cost defensive tarpit for red calls, using one short harmless clarification per turn for up to the 10-minute call limit;
- no transfer, callback, compliance, sensitive disclosure, or fake action for remote access, payment changes, refunds, credentials, or account access;
- an ElevenLabs `end_call` tool that does not terminate red calls merely because verification is complete;
- post-call extraction of route, claimed identity, company, reference, requested action, risk signals, outcome, and distraction-turn count;
- seven-day voice/transcript retention for the demo.

The human-transfer policy is documented in [`transfer-tool.template.json`](./transfer-tool.template.json). It deliberately contains a placeholder instead of a real phone number.

## Environment and services

Copy `.env.example` to `.env` and set:

```env
ELEVENLABS_API_KEY=
HUMAN_TRANSFER_NUMBER=
```

`ELEVENLABS_API_KEY` must remain server-side and is already excluded from Git. `HUMAN_TRANSFER_NUMBER` is optional and reserved for a later real-phone version.

Required service for this demo: ElevenLabs Agents through the embedded browser voice widget. Reception.ai and a phone number are not required for the simulated demo.

## Verification

The scenario definitions are under [`tests/`](./tests), and concise transcripts plus evaluations are in [`evidence/scenario-results.md`](./evidence/scenario-results.md).

| Scenario | Route | Outcome | Result |
| --- | --- | --- | --- |
| Haircut request | Green | Callback details taken; no false confirmation | Pass |
| Supplier delivery | Amber | Staff schedule withheld; message taken | Pass |
| POS remote-access demand | Red | 10 distraction turns; no transfer; caller gave up | Pass |

## Current limitation

The browser demo does not place or transfer real phone calls. Green calls currently take a callback request; the safe human handoff is represented by that simulated outcome. The transfer template remains available for a later phone-enabled version.

## Exact next task for teammate two

Turn the post-call extraction into an on-page demo result card showing route, outcome, risk signals, and distraction-turn count after each browser conversation. Keep real telephony, authentication, and a full dashboard out of this slice.
