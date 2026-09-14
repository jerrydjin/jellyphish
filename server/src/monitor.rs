use crate::db::now_millis;
use risk_core::{CallerContext, RiskSignal, TranscriptTurn, detect_signals, recommendation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    pub line: String,
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
    Active(Box<MonitorSession>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffRoom {
    pub line: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub status: String,
    #[serde(rename = "disclosureGiven")]
    pub disclosure_given: bool,
    #[serde(rename = "callerName")]
    pub caller_name: Option<String>,
    #[serde(rename = "requestedAction")]
    pub requested_action: Option<String>,
    pub offer: Option<Value>,
    pub answer: Option<Value>,
    #[serde(rename = "callerIce")]
    pub caller_ice: Vec<Value>,
    #[serde(rename = "staffIce")]
    pub staff_ice: Vec<Value>,
    #[serde(skip)]
    caption_key: String,
    #[serde(skip)]
    staff_caption_key: Option<String>,
    #[serde(skip)]
    ended_at: Option<i64>,
}

pub struct RingParams {
    pub line: String,
    pub session_id: String,
    pub disclosure_given: bool,
    pub caller_name: Option<String>,
    pub requested_action: Option<String>,
    pub caption_key: String,
    pub offer: Value,
    pub source: &'static str,
}

pub struct MonitorHub {
    sessions: HashMap<String, MonitorSession>,
    rooms: HashMap<String, HandoffRoom>,
    active_ids: HashMap<String, String>,
    events: broadcast::Sender<(String, String, String)>,
}

impl Default for MonitorHub {
    fn default() -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            sessions: HashMap::new(),
            rooms: HashMap::new(),
            active_ids: HashMap::new(),
            events,
        }
    }
}

impl MonitorHub {
    pub fn subscribe(&self) -> broadcast::Receiver<(String, String, String)> {
        self.events.subscribe()
    }

    pub fn snapshot(&self, line: &str) -> MonitorSnapshot {
        self.active_ids
            .get(line)
            .and_then(|id| self.sessions.get(id))
            .cloned()
            .map(Box::new)
            .map(MonitorSnapshot::Active)
            .unwrap_or_else(|| MonitorSnapshot::Idle {
                status: "idle",
                signal_catalog: signal_catalog(),
            })
    }

    pub fn begin(
        &mut self,
        line: String,
        session_id: String,
        disclosure_given: bool,
        source: &str,
    ) -> MonitorSession {
        let now = now_millis();
        let key = room_key(&line, &session_id);
        let session = self
            .sessions
            .entry(key.clone())
            .or_insert_with(|| MonitorSession {
                line: line.clone(),
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
        session.line = line.clone();
        session.source = source.to_owned();
        session.handoff_status = "connected".to_owned();
        session.monitor_status = "monitoring".to_owned();
        session.disclosure_given = disclosure_given;
        session.ai_muted = true;
        session.updated_at = now;
        session.error = None;
        self.active_ids.insert(line, key);
        let result = session.clone();
        self.publish("handoff", &result);
        result
    }

    pub fn ingest(
        &mut self,
        line: String,
        session_id: String,
        sequence: Option<i64>,
        role: String,
        text: String,
        source: &str,
    ) -> anyhow::Result<MonitorSession> {
        if !matches!(role.as_str(), "caller" | "staff") || text.trim().is_empty() {
            anyhow::bail!("caller/staff role and text are required");
        }
        let key = room_key(&line, &session_id);
        if !self.sessions.contains_key(&key) {
            self.begin(line.clone(), session_id.clone(), false, source);
        }
        let session = self.sessions.get_mut(&key).expect("session exists");
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
        if session
            .transcript
            .iter()
            .rev()
            .take(6)
            .any(|turn| turn.role == role && similar_caption(&turn.text, &cleaned))
        {
            return Ok(session.clone());
        }
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
            let new_signals: Vec<_> = detect_signals(&context)
                .into_iter()
                .filter(|signal| session.seen_signals.insert(*signal))
                .collect();
            let high_risk = new_signals.iter().any(|signal| {
                matches!(
                    signal,
                    RiskSignal::PaymentOrAccountChange
                        | RiskSignal::CredentialOrMfaRequest
                        | RiskSignal::RemoteAccessRequest
                        | RiskSignal::CoerciveUrgency
                )
            });
            if high_risk {
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

    pub fn ring(&mut self, params: RingParams) -> anyhow::Result<HandoffRoom> {
        let RingParams {
            line,
            session_id,
            disclosure_given,
            caller_name,
            requested_action,
            caption_key,
            offer,
            source,
        } = params;
        validate_sdp(&offer, "offer")?;
        let key = room_key(&line, &session_id);
        if let Some(existing) = self.rooms.get(&key) {
            if existing.status != "ended" && existing.offer.is_some() {
                return Ok(existing.clone());
            }
        }
        let session = self.begin_with_status(
            line.clone(),
            session_id.clone(),
            disclosure_given,
            source,
            "ringing",
        );
        let room = HandoffRoom {
            line: line.clone(),
            session_id: session_id.clone(),
            status: "ringing".to_owned(),
            disclosure_given,
            caller_name,
            requested_action,
            offer: Some(offer),
            answer: None,
            caller_ice: Vec::new(),
            staff_ice: Vec::new(),
            caption_key,
            staff_caption_key: None,
            ended_at: None,
        };
        self.rooms.insert(key, room.clone());
        self.publish("handoff", &session);
        self.publish_json(&line, "incoming", &room);
        Ok(room)
    }

    pub fn signal(
        &mut self,
        line: String,
        session_id: String,
        from: String,
        kind: String,
        caption_key: Option<String>,
        payload: Value,
    ) -> anyhow::Result<HandoffRoom> {
        if !matches!(from.as_str(), "caller" | "staff") {
            anyhow::bail!("from must be caller or staff");
        }
        let key = room_key(&line, &session_id);
        if !self.rooms.contains_key(&key) {
            anyhow::bail!("no ringing handoff for this session");
        }
        match kind.as_str() {
            "answer" => {
                if from != "staff" {
                    anyhow::bail!("only staff can answer");
                }
                validate_sdp(&payload, "answer")?;
                let staff_caption_key = caption_key.unwrap_or_default();
                if staff_caption_key.is_empty() {
                    anyhow::bail!("staff caption key is required");
                }
                let room = self.rooms.get_mut(&key).expect("room exists");
                if room.answer.is_some() && room.status != "ended" {
                    anyhow::bail!("this handoff was already answered");
                }
                room.answer = Some(payload.clone());
                room.staff_caption_key = Some(staff_caption_key);
                room.status = "connected".to_owned();
                room.ended_at = None;
            }
            "ice" => {
                let room = self.rooms.get_mut(&key).expect("room exists");
                let bucket = if from == "caller" {
                    &mut room.caller_ice
                } else {
                    &mut room.staff_ice
                };
                if bucket.len() >= 40 {
                    bucket.remove(0);
                }
                bucket.push(payload.clone());
            }
            _ => anyhow::bail!("kind must be answer or ice"),
        }
        let room = self.rooms.get(&key).expect("room exists").clone();
        if kind == "answer" {
            let session = self.begin_with_status(
                line.clone(),
                session_id.clone(),
                room.disclosure_given,
                "browser-handoff",
                "connected",
            );
            self.publish("handoff", &session);
        }
        self.publish_json(
            &line,
            "signal",
            &serde_json::json!({
                "sessionId": session_id,
                "from": from,
                "kind": kind,
                "payload": payload,
                "room": room,
            }),
        );
        Ok(room)
    }

    pub fn hangup(
        &mut self,
        line: String,
        session_id: String,
        from: &str,
    ) -> anyhow::Result<HandoffRoom> {
        let key = room_key(&line, &session_id);
        let room = self
            .rooms
            .get_mut(&key)
            .ok_or_else(|| anyhow::anyhow!("no handoff for this session"))?;
        room.status = "ended".to_owned();
        room.ended_at = Some(now_millis());
        let result = room.clone();
        if let Some(session) = self.sessions.get_mut(&key) {
            session.handoff_status = "ended".to_owned();
            session.updated_at = now_millis();
            let session = session.clone();
            self.publish("handoff", &session);
        }
        self.publish_json(
            &line,
            "hangup",
            &serde_json::json!({
                "sessionId": session_id,
                "from": from,
                "room": result,
            }),
        );
        Ok(result)
    }

    pub fn room(&self, line: &str, session_id: &str) -> Option<HandoffRoom> {
        self.rooms.get(&room_key(line, session_id)).cloned()
    }

    pub fn accepts_captions(
        &self,
        line: &str,
        session_id: &str,
        role: &str,
        caption_key: &str,
    ) -> bool {
        self.room(line, session_id).is_some_and(|room| {
            let recently_ended = room
                .ended_at
                .is_some_and(|ended_at| now_millis().saturating_sub(ended_at) <= 30_000);
            (room.status == "connected" || recently_ended)
                && !caption_key.is_empty()
                && match role {
                    // The connected staff console owns same-device demo
                    // transcription and may label its single mic as either
                    // participant. The caller key remains valid for callers.
                    "caller" => {
                        room.caption_key == caption_key
                            || room.staff_caption_key.as_deref() == Some(caption_key)
                    }
                    "staff" => room.staff_caption_key.as_deref() == Some(caption_key),
                    _ => false,
                }
        })
    }

    fn begin_with_status(
        &mut self,
        line: String,
        session_id: String,
        disclosure_given: bool,
        source: &str,
        handoff_status: &str,
    ) -> MonitorSession {
        let now = now_millis();
        let key = room_key(&line, &session_id);
        let session = self
            .sessions
            .entry(key.clone())
            .or_insert_with(|| MonitorSession {
                line: line.clone(),
                session_id: session_id.clone(),
                source: source.to_owned(),
                handoff_status: handoff_status.to_owned(),
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
        session.line = line.clone();
        session.source = source.to_owned();
        session.handoff_status = handoff_status.to_owned();
        session.monitor_status = "monitoring".to_owned();
        session.disclosure_given = disclosure_given;
        session.ai_muted = true;
        session.updated_at = now;
        session.error = None;
        self.active_ids.insert(line, key);
        session.clone()
    }

    fn publish_json(&self, line: &str, event: &str, value: &impl Serialize) {
        if let Ok(json) = serde_json::to_string(value) {
            let _ = self.events.send((line.to_owned(), event.to_owned(), json));
        }
    }

    pub fn fail(
        &mut self,
        line: String,
        session_id: String,
        reason: String,
        source: &str,
    ) -> MonitorSession {
        let key = room_key(&line, &session_id);
        if !self.sessions.contains_key(&key) {
            self.begin(line.clone(), session_id.clone(), false, source);
        }
        let session = self.sessions.get_mut(&key).expect("session exists");
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
            let _ = self
                .events
                .send((session.line.clone(), event.to_owned(), json));
        }
    }
}

fn room_key(line: &str, session_id: &str) -> String {
    format!("{line}\u{1f}{session_id}")
}

fn validate_sdp(value: &Value, kind: &str) -> anyhow::Result<()> {
    let sdp = value.get("sdp").and_then(Value::as_str).unwrap_or_default();
    let sdp_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if sdp.is_empty() || sdp.len() > 32_000 {
        anyhow::bail!("a valid SDP {kind} is required");
    }
    if sdp_type != kind {
        anyhow::bail!("SDP type must be {kind}");
    }
    Ok(())
}

fn similar_caption(existing: &str, incoming: &str) -> bool {
    if existing == incoming {
        return true;
    }
    let (shorter, longer) = if existing.len() <= incoming.len() {
        (existing, incoming)
    } else {
        (incoming, existing)
    };
    !shorter.is_empty() && longer.starts_with(shorter) && longer.len() - shorter.len() < 24
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
