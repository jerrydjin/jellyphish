use crate::db::now_millis;
use risk_core::{CallerContext, RiskLevel, RiskSignal, TranscriptTurn, evaluate, recommendation};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorTurn {
    pub id: String,
    pub role: String,
    pub text: String,
    #[serde(rename = "receivedAt")]
    pub received_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorSignal {
    pub id: RiskSignal,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorAlert {
    pub id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub severity: String,
    pub signals: Vec<MonitorSignal>,
    pub evidence: String,
    #[serde(rename = "recommendedAction")]
    pub recommended_action: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorSession {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub source: String,
    #[serde(rename = "handoffStatus")]
    pub handoff_status: String,
    #[serde(rename = "monitorStatus")]
    pub monitor_status: String,
    #[serde(rename = "disclosureGiven")]
    pub disclosure_given: bool,
    #[serde(rename = "aiMuted")]
    pub ai_muted: bool,
    #[serde(rename = "startedAt")]
    pub started_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    pub transcript: Vec<MonitorTurn>,
    pub alerts: Vec<MonitorAlert>,
    pub error: Option<String>,
    #[serde(skip)]
    seen_signals: HashSet<RiskSignal>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum MonitorSnapshot {
    Idle {
        status: &'static str,
        #[serde(rename = "signalCatalog")]
        signal_catalog: Vec<MonitorSignal>,
    },
    Active(MonitorSession),
}

pub struct MonitorHub {
    sessions: HashMap<String, MonitorSession>,
    active_id: Option<String>,
    events: broadcast::Sender<(String, String)>,
}

impl Default for MonitorHub {
    fn default() -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            sessions: HashMap::new(),
            active_id: None,
            events,
        }
    }
}

impl MonitorHub {
    pub fn subscribe(&self) -> broadcast::Receiver<(String, String)> {
        self.events.subscribe()
    }

    pub fn snapshot(&self) -> MonitorSnapshot {
        self.active_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
            .cloned()
            .map(MonitorSnapshot::Active)
            .unwrap_or_else(|| MonitorSnapshot::Idle {
                status: "idle",
                signal_catalog: signal_catalog(),
            })
    }

    pub fn begin(
        &mut self,
        session_id: String,
        disclosure_given: bool,
        source: &str,
    ) -> MonitorSession {
        let now = now_millis();
        let session = self
            .sessions
            .entry(session_id.clone())
            .or_insert_with(|| MonitorSession {
                session_id: session_id.clone(),
                source: source.to_owned(),
                handoff_status: "connected".to_owned(),
                monitor_status: "monitoring".to_owned(),
                disclosure_given,
                ai_muted: true,
                started_at: now,
                updated_at: now,
                transcript: Vec::new(),
                alerts: Vec::new(),
                error: None,
                seen_signals: HashSet::new(),
            });
        session.source = source.to_owned();
        session.handoff_status = "connected".to_owned();
        session.monitor_status = "monitoring".to_owned();
        session.disclosure_given = disclosure_given;
        session.ai_muted = true;
        session.updated_at = now;
        session.error = None;
        self.active_id = Some(session_id);
        let result = session.clone();
        self.publish("handoff", &result);
        result
    }

    pub fn ingest(
        &mut self,
        session_id: String,
        sequence: Option<i64>,
        role: String,
        text: String,
        source: &str,
    ) -> anyhow::Result<MonitorSession> {
        if !matches!(role.as_str(), "caller" | "staff") || text.trim().is_empty() {
            anyhow::bail!("caller/staff role and text are required");
        }
        if !self.sessions.contains_key(&session_id) {
            self.begin(session_id.clone(), false, source);
        }
        let session = self.sessions.get_mut(&session_id).expect("session exists");
        let now = now_millis();
        let cleaned: String = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(4_000)
            .collect();
        session.updated_at = now;
        session.ai_muted = true;
        session.transcript.push(MonitorTurn {
            id: format!(
                "{}:{}",
                session_id,
                sequence.unwrap_or(session.transcript.len() as i64)
            ),
            role: role.clone(),
            text: cleaned.clone(),
            received_at: now,
        });
        if session.transcript.len() > 160 {
            session.transcript.drain(..session.transcript.len() - 160);
        }

        let mut emitted_alert = false;
        if role == "caller" {
            let context = CallerContext {
                call_id: session_id.clone(),
                transcript: vec![TranscriptTurn {
                    role,
                    message: cleaned.clone(),
                }],
                ..Default::default()
            };
            let decision = evaluate(&context);
            let new_signals: Vec<_> = decision
                .signals
                .into_iter()
                .filter(|signal| session.seen_signals.insert(*signal))
                .collect();
            if decision.risk_level == RiskLevel::Red && !new_signals.is_empty() {
                let primary = new_signals
                    .iter()
                    .copied()
                    .find(|signal| *signal == RiskSignal::PaymentOrAccountChange)
                    .unwrap_or(new_signals[0]);
                session.alerts.push(MonitorAlert {
                    id: format!("{}:{}:{:?}", session_id, now, primary),
                    session_id: session_id.clone(),
                    severity: "high".to_owned(),
                    signals: new_signals
                        .into_iter()
                        .map(|id| MonitorSignal {
                            id,
                            label: signal_label(id).to_owned(),
                        })
                        .collect(),
                    evidence: cleaned.chars().take(280).collect(),
                    recommended_action: recommendation(primary).to_owned(),
                    created_at: now,
                });
                emitted_alert = true;
            }
        }
        let result = session.clone();
        self.publish(
            if emitted_alert {
                "risk-alert"
            } else {
                "transcript"
            },
            &result,
        );
        Ok(result)
    }

    pub fn fail(&mut self, session_id: String, reason: String, source: &str) -> MonitorSession {
        if !self.sessions.contains_key(&session_id) {
            self.begin(session_id.clone(), false, source);
        }
        let session = self.sessions.get_mut(&session_id).expect("session exists");
        session.handoff_status = "connected".to_owned();
        session.monitor_status = "degraded".to_owned();
        session.ai_muted = true;
        session.error = Some(reason.chars().take(240).collect());
        session.updated_at = now_millis();
        let result = session.clone();
        self.publish("monitor-failure", &result);
        result
    }

    fn publish(&self, event: &str, session: &MonitorSession) {
        if let Ok(json) = serde_json::to_string(session) {
            let _ = self.events.send((event.to_owned(), json));
        }
    }
}

fn signal_label(signal: RiskSignal) -> &'static str {
    match signal {
        RiskSignal::PaymentOrAccountChange => "payment or account change",
        RiskSignal::CredentialOrMfaRequest => "credential or MFA request",
        RiskSignal::RemoteAccessRequest => "remote-access request",
        RiskSignal::CoerciveUrgency => "coercive urgency",
        RiskSignal::ProtectedInformationRequest => "protected information request",
        RiskSignal::UnverifiedBusinessCaller => "unverified business caller",
    }
}

fn signal_catalog() -> Vec<MonitorSignal> {
    [
        RiskSignal::PaymentOrAccountChange,
        RiskSignal::CredentialOrMfaRequest,
        RiskSignal::RemoteAccessRequest,
        RiskSignal::CoerciveUrgency,
        RiskSignal::ProtectedInformationRequest,
        RiskSignal::UnverifiedBusinessCaller,
    ]
    .into_iter()
    .map(|id| MonitorSignal {
        id,
        label: signal_label(id).to_owned(),
    })
    .collect()
}
