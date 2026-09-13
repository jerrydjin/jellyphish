//! Relay between a caller's browser (the phone simulator) and a business
//! dashboard. The caller owns the realtime voice session; it publishes the
//! whole call snapshot here and every dashboard for that line mirrors it.

use crate::db::now_millis;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::broadcast;

/// Business lines a caller can reach. Each line has its own dashboard.
pub const LINES: &[&str] = &["studio-sol", "dental-clinic"];

/// A call that stops sending heartbeats for this long is treated as dropped.
pub const STALE_AFTER_MS: i64 = 20_000;

const MAX_TURNS: usize = 200;
const MAX_MESSAGE_CHARS: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LiveTurn {
    pub role: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct LiveUpdate {
    #[serde(rename = "callId")]
    pub call_id: String,
    pub seq: u64,
    pub status: String,
    #[serde(rename = "conversationId", default)]
    pub conversation_id: Option<String>,
    #[serde(rename = "callerNumber", default)]
    pub caller_number: Option<String>,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub transcript: Vec<LiveTurn>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LiveCall {
    #[serde(rename = "callId")]
    pub call_id: String,
    /// `connecting`, `connected`, `ended`, or `lost` (heartbeat stopped).
    pub status: String,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "callerNumber")]
    pub caller_number: Option<String>,
    pub muted: bool,
    pub mode: Option<String>,
    pub transcript: Vec<LiveTurn>,
    #[serde(rename = "startedAt")]
    pub started_at: Option<i64>,
    #[serde(rename = "endedAt")]
    pub ended_at: Option<i64>,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    #[serde(skip)]
    seq: u64,
}

impl LiveCall {
    fn finished(&self) -> bool {
        matches!(self.status.as_str(), "ended" | "lost")
    }
}

#[derive(Debug, Serialize)]
pub struct LiveSnapshot {
    pub line: String,
    pub call: Option<LiveCall>,
    #[serde(rename = "serverNow")]
    pub server_now: i64,
}

pub struct LiveHub {
    calls: HashMap<String, LiveCall>,
    events: broadcast::Sender<(String, String)>,
}

impl Default for LiveHub {
    fn default() -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            calls: HashMap::new(),
            events,
        }
    }
}

impl LiveHub {
    pub fn is_line(line: &str) -> bool {
        LINES.contains(&line)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, String)> {
        self.events.subscribe()
    }

    pub fn snapshot(&mut self, line: &str) -> LiveSnapshot {
        let now = now_millis();
        if let Some(call) = self.calls.get_mut(line)
            && !call.finished()
            && now - call.updated_at > STALE_AFTER_MS
        {
            call.status = "lost".to_owned();
            call.ended_at = Some(call.updated_at);
        }
        LiveSnapshot {
            line: line.to_owned(),
            call: self.calls.get(line).cloned(),
            server_now: now,
        }
    }

    /// Applies a caller snapshot. Returns `false` when the update is stale:
    /// an older sequence number, or a heartbeat arriving after the call ended.
    pub fn apply(&mut self, line: &str, update: LiveUpdate) -> anyhow::Result<bool> {
        if !matches!(update.status.as_str(), "connecting" | "connected" | "ended") {
            anyhow::bail!("status must be connecting, connected, or ended");
        }
        let call_id = clip(&update.call_id, 80);
        if call_id.trim().is_empty() {
            anyhow::bail!("callId is required");
        }
        let now = now_millis();
        let previous = self.calls.get(line).filter(|call| call.call_id == call_id);
        if let Some(call) = previous
            && (update.seq <= call.seq || call.finished())
        {
            return Ok(false);
        }

        let mut transcript: Vec<LiveTurn> = update
            .transcript
            .into_iter()
            .filter(|turn| matches!(turn.role.as_str(), "user" | "agent"))
            .map(|turn| LiveTurn {
                role: turn.role,
                message: clip(turn.message.trim(), MAX_MESSAGE_CHARS),
            })
            .filter(|turn| !turn.message.is_empty())
            .collect();
        if transcript.len() > MAX_TURNS {
            transcript.drain(..transcript.len() - MAX_TURNS);
        }

        let connected = update.status == "connected";
        let ended = update.status == "ended";
        let started_at = previous
            .and_then(|call| call.started_at)
            .or(connected.then_some(now));
        let call = LiveCall {
            call_id,
            status: update.status,
            conversation_id: update
                .conversation_id
                .map(|id| clip(&id, 200))
                .or_else(|| previous.and_then(|call| call.conversation_id.clone())),
            caller_number: update.caller_number.map(|number| clip(&number, 32)),
            muted: update.muted,
            mode: update
                .mode
                .filter(|mode| matches!(mode.as_str(), "listening" | "speaking")),
            transcript,
            started_at,
            ended_at: ended.then_some(now),
            updated_at: now,
            seq: update.seq,
        };
        self.calls.insert(line.to_owned(), call);
        let snapshot = self.snapshot(line);
        if let Ok(json) = serde_json::to_string(&snapshot) {
            let _ = self.events.send((line.to_owned(), json));
        }
        Ok(true)
    }
}

fn clip(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}
