const SIGNALS = [
  {
    id: "payment_account_change",
    label: "payment or account change",
    pattern: /\b(?:bank|payment|account|invoice|refund|settlement)\b.{0,70}\b(?:change|changed|changing|update|updated|new|replace|redirect|different)\b|\b(?:change|changed|changing|update|updated|replace|redirect)\b.{0,70}\b(?:bank|payment|account|invoice|refund|settlement)\b/i,
    recommendation: "Do not change payment details; verify using the registered supplier contact.",
  },
  {
    id: "credential_mfa_request",
    label: "credential or MFA request",
    pattern: /\b(?:password|passcode|verification code|security code|one[- ]time code|otp|mfa|multi[- ]factor|login code)\b/i,
    recommendation: "Do not share credentials or codes; end the request and verify through an approved channel.",
  },
  {
    id: "remote_access",
    label: "remote-access request",
    pattern: /\b(?:remote access|remote support|screen share|anydesk|teamviewer|logmein|install.{0,35}(?:support|software|tool|app))\b/i,
    recommendation: "Do not install software or grant access; verify through the provider’s registered support channel.",
  },
  {
    id: "coercive_urgency",
    label: "coercive urgency",
    pattern: /\b(?:must|need(?:s)? to|have to).{0,35}\b(?:today|now|immediately|right away)\b|\b(?:urgent|immediately|right now|today only|final warning|act now)\b|\b(?:or|otherwise).{0,45}\b(?:suspend|close|cancel|penalty|fee|lose access)\b/i,
    recommendation: "Pause the request and independently verify the caller through a registered contact.",
  },
];

function cleanEvidence(text) {
  return String(text || "").replace(/\s+/g, " ").trim().slice(0, 280);
}

export function assessTranscriptChunk({ sessionId, role, text, seenSignals = new Set() }) {
  if (!sessionId || role !== "caller") return { alerts: [], seenSignals };
  const evidence = cleanEvidence(text);
  if (!evidence) return { alerts: [], seenSignals };

  const matches = SIGNALS.filter(({ id, pattern }) => !seenSignals.has(id) && pattern.test(evidence));
  for (const match of matches) seenSignals.add(match.id);
  if (!matches.length) return { alerts: [], seenSignals };

  const primary = matches.find(({ id }) => id === "payment_account_change") || matches[0];
  return {
    seenSignals,
    alerts: [
      {
        id: `${sessionId}:${Date.now()}:${primary.id}`,
        sessionId,
        severity: "high",
        signals: matches.map(({ id, label }) => ({ id, label })),
        evidence,
        recommendedAction: primary.recommendation,
        createdAt: Date.now(),
      },
    ],
  };
}

export function signalCatalog() {
  return SIGNALS.map(({ id, label }) => ({ id, label }));
}
