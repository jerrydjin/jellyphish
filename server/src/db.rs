use risk_core::{CallerContext, RiskDecision, RiskLevel, RiskSignal};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use std::{
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct EventStore {
    pool: SqlitePool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallSummary {
    pub call_id: String,
    pub risk_level: RiskLevel,
    pub signals: Vec<RiskSignal>,
    pub caller_name: Option<String>,
    pub claimed_company: Option<String>,
    pub requested_action: String,
    pub outcome: String,
    pub summary: String,
    pub updated_at: i64,
}

impl EventStore {
    pub async fn connect(database_url: &str) -> anyhow::Result<Self> {
        if database_url.starts_with("sqlite://data/") {
            std::fs::create_dir_all("data")?;
        }
        let options = sqlx::sqlite::SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .foreign_keys(true);
        let connections = if database_url.contains(":memory:") {
            1
        } else {
            5
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(connections)
            .connect_with(options)
            .await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                call_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                dedupe_key TEXT UNIQUE,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS events_call_id_created_at ON events(call_id, created_at)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS call_summaries (
                call_id TEXT PRIMARY KEY,
                risk_level TEXT NOT NULL,
                summary_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn append_event(
        &self,
        call_id: &str,
        event_type: &str,
        payload: &Value,
        dedupe_key: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO events(call_id, event_type, payload_json, dedupe_key, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(call_id)
        .bind(event_type)
        .bind(serde_json::to_string(payload)?)
        .bind(dedupe_key)
        .bind(now_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_call(
        &self,
        context: &CallerContext,
        decision: &RiskDecision,
        outcome: String,
        summary: String,
    ) -> anyhow::Result<CallSummary> {
        let value = CallSummary {
            call_id: bounded(&context.call_id, 200),
            risk_level: decision.risk_level,
            signals: decision.signals.clone(),
            caller_name: context
                .caller_name
                .as_deref()
                .map(|value| bounded(value, 200)),
            claimed_company: context
                .claimed_company
                .as_deref()
                .map(|value| bounded(value, 200)),
            requested_action: bounded(&context.requested_action, 2_000),
            outcome: bounded(&outcome, 200),
            summary: bounded(&summary, 2_000),
            updated_at: now_millis(),
        };
        let json = serde_json::to_string(&value)?;
        sqlx::query(
            "INSERT INTO call_summaries(call_id, risk_level, summary_json, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(call_id) DO UPDATE SET
               risk_level = excluded.risk_level,
               summary_json = excluded.summary_json,
               updated_at = excluded.updated_at",
        )
        .bind(&value.call_id)
        .bind(format!("{:?}", value.risk_level).to_lowercase())
        .bind(json)
        .bind(value.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(value)
    }

    pub async fn summaries(&self, limit: i64) -> anyhow::Result<Vec<CallSummary>> {
        let rows =
            sqlx::query("SELECT summary_json FROM call_summaries ORDER BY updated_at DESC LIMIT ?")
                .bind(limit.clamp(1, 100))
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|row| serde_json::from_str(row.get::<&str, _>("summary_json")).map_err(Into::into))
            .collect()
    }
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn bounded(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
