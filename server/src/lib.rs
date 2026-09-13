mod auth;
mod config;
mod db;
mod elevenlabs;
mod monitor;

pub use config::Config;
pub use db::{CallSummary, EventStore};

use auth::{SignatureError, verify_elevenlabs, verify_monitor};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use db::now_millis;
use elevenlabs::ElevenLabsClient;
use futures_util::{StreamExt, stream};
use monitor::{MonitorHub, MonitorSnapshot};
use risk_core::{CallerContext, RiskDecision, RiskLevel, TranscriptTurn, evaluate};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::{services::ServeDir, trace::TraceLayer};

#[derive(Clone)]
pub struct AppState {
    config: Config,
    store: EventStore,
    elevenlabs: ElevenLabsClient,
    monitor: Arc<RwLock<MonitorHub>>,
}

pub async fn build_state(config: Config) -> anyhow::Result<AppState> {
    let store = EventStore::connect(&config.database_url).await?;
    let elevenlabs = ElevenLabsClient::new(
        config.elevenlabs_api_key.clone(),
        config.elevenlabs_agent_id.clone(),
    )?;
    Ok(AppState {
        config,
        store,
        elevenlabs,
        monitor: Arc::new(RwLock::new(MonitorHub::default())),
    })
}

pub fn router(state: AppState) -> Router {
    let web_root = state.config.web_root.clone();
    Router::new()
        .route("/healthz", get(health))
        .route("/api/health", get(health))
        .route("/tool/assess", post(assess))
        .route("/tool/finish", post(finish))
        .route("/webhooks/elevenlabs/post-call", post(elevenlabs_post_call))
        .route("/api/summaries", get(summaries))
        .route("/api/conversations", get(conversations))
        .route("/api/conversations/{id}", get(conversation))
        .route("/api/monitor/events", get(monitor_events))
        .route("/api/monitor/state", get(monitor_state))
        .route("/api/monitor/demo", post(monitor_demo))
        .route("/api/webhooks/handoff", post(monitor_handoff))
        .route("/api/webhooks/transcript", post(monitor_transcript))
        .route("/api/webhooks/monitor-failure", post(monitor_failure))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .fallback_service(ServeDir::new(web_root).append_index_html_on_directories(true))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": "jellyphish-rust",
        "agentId": state.config.elevenlabs_agent_id,
        "apiConfigured": state.elevenlabs.configured(),
        "elevenLabsWebhookConfigured": state.config.elevenlabs_webhook_secret.is_some(),
        "monitorWebhookConfigured": state.config.monitor_webhook_secret.is_some(),
        "toolAuthConfigured": state.config.tool_api_key.is_some(),
        "monitoredTransferReady": state.config.monitor_webhook_secret.is_some() && state.config.human_transfer_number.is_some(),
    }))
}

async fn assess(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut context): Json<CallerContext>,
) -> Result<Json<RiskDecision>, ApiError> {
    verify_tool_key(&headers, state.config.tool_api_key.as_deref())?;
    if context.call_id.trim().is_empty() {
        context.call_id = format!("tool_{}", now_millis());
    }
    context.call_id = context.call_id.chars().take(200).collect();
    // The voice model cannot assert that a contact or reference is trusted.
    // Those flags remain false until a future server-side lookup supplies them.
    context.known_contact = false;
    context.verified_reference = false;
    let decision = evaluate(&context);
    state
        .store
        .append_event(
            &context.call_id,
            "risk_assessed",
            &json!({ "proposed_action": context.proposed_action, "decision": decision }),
            None,
        )
        .await?;
    Ok(Json(decision))
}

#[derive(Deserialize)]
struct FinishRequest {
    context: CallerContext,
    outcome: String,
    #[serde(default)]
    summary: String,
}

async fn finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<FinishRequest>,
) -> Result<Json<CallSummary>, ApiError> {
    verify_tool_key(&headers, state.config.tool_api_key.as_deref())?;
    if request.context.call_id.trim().is_empty() {
        return Err(ApiError::bad_request("context.call_id is required"));
    }
    let mut context = request.context;
    context.call_id = context.call_id.chars().take(200).collect();
    context.known_contact = false;
    context.verified_reference = false;
    let decision = evaluate(&context);
    let summary = state
        .store
        .finish_call(&context, &decision, request.outcome, request.summary)
        .await?;
    state
        .store
        .append_event(
            &summary.call_id,
            "call_finished",
            &serde_json::to_value(&summary)?,
            Some(&format!("finish:{}", summary.call_id)),
        )
        .await?;
    Ok(Json(summary))
}

#[derive(Deserialize)]
struct SummaryQuery {
    limit: Option<i64>,
}

async fn summaries(
    State(state): State<AppState>,
    Query(query): Query<SummaryQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        json!({ "summaries": state.store.summaries(query.limit.unwrap_or(30)).await? }),
    ))
}

async fn conversations(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let mut value = state.elevenlabs.conversations().await?;
    let summaries = state.store.summaries(100).await?;
    if let Some(items) = value["conversations"].as_array_mut() {
        for item in items {
            let Some(id) = item["id"].as_str() else {
                continue;
            };
            if let Some(summary) = summaries.iter().find(|summary| summary.call_id == id) {
                item["policy"] = json!({
                    "risk_level": summary.risk_level,
                    "signals": summary.signals,
                    "transfer_allowed": summary.risk_level == RiskLevel::Green,
                });
            }
        }
    }
    Ok(Json(value))
}

async fn conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut value = state.elevenlabs.conversation(&id).await?;
    let transcript = value["transcript"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|turn| {
            Some(TranscriptTurn {
                role: turn["role"].as_str()?.to_owned(),
                message: turn["message"].as_str()?.to_owned(),
            })
        })
        .collect();
    let decision = evaluate(&CallerContext {
        call_id: id,
        transcript,
        ..Default::default()
    });
    value["policy"] = serde_json::to_value(decision)?;
    Ok(Json(value))
}

async fn elevenlabs_post_call(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    verify_elevenlabs(
        &body,
        header(&headers, "elevenlabs-signature"),
        state.config.elevenlabs_webhook_secret.as_deref(),
    )
    .map_err(ApiError::signature)?;
    let event: Value = serde_json::from_slice(&body)?;
    let kind = event["type"].as_str().unwrap_or("unknown");
    let call_id = event["data"]["conversation_id"]
        .as_str()
        .ok_or_else(|| ApiError::bad_request("data.conversation_id is required"))?;
    let dedupe_key = format!("elevenlabs:{kind}:{call_id}:{}", event["event_timestamp"]);
    let event_metadata = json!({
        "type": kind,
        "event_timestamp": event["event_timestamp"],
        "agent_id": event["data"]["agent_id"],
    });
    state
        .store
        .append_event(call_id, kind, &event_metadata, Some(&dedupe_key))
        .await?;

    if kind == "post_call_transcription" {
        let transcript = event["data"]["transcript"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|turn| {
                Some(TranscriptTurn {
                    role: turn["role"].as_str()?.to_owned(),
                    message: turn["message"].as_str()?.to_owned(),
                })
            })
            .collect();
        let context = CallerContext {
            call_id: call_id.to_owned(),
            requested_action: collection_value(
                &event["data"]["analysis"]["data_collection_results"],
                "requested_action",
            ),
            caller_name: non_empty_value(collection_value(
                &event["data"]["analysis"]["data_collection_results"],
                "caller_name",
            )),
            claimed_company: non_empty_value(collection_value(
                &event["data"]["analysis"]["data_collection_results"],
                "claimed_company",
            )),
            transcript,
            ..Default::default()
        };
        let decision = evaluate(&context);
        state
            .store
            .finish_call(
                &context,
                &decision,
                collection_value(
                    &event["data"]["analysis"]["data_collection_results"],
                    "outcome",
                ),
                event["data"]["analysis"]["transcript_summary"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
            .await?;
    }
    Ok(Json(json!({ "status": "received" })))
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct HandoffPayload {
    session_id: String,
    #[serde(default)]
    disclosure_given: bool,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct TranscriptPayload {
    session_id: String,
    sequence: Option<i64>,
    role: String,
    text: String,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct FailurePayload {
    session_id: String,
    #[serde(default = "default_failure")]
    reason: String,
}

fn default_failure() -> String {
    "Monitoring sidecar unavailable".to_owned()
}

async fn monitor_handoff(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<MonitorSnapshot>), ApiError> {
    verify_monitor_request(&state, &headers, &body)?;
    let payload: HandoffPayload = serde_json::from_slice(&body)?;
    require_session(&payload.session_id)?;
    let session = state.monitor.write().await.begin(
        payload.session_id.clone(),
        payload.disclosure_given,
        "verified-webhook",
    );
    state
        .store
        .append_event(
            &payload.session_id,
            "handoff",
            &serde_json::to_value(&payload)?,
            None,
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(MonitorSnapshot::Active(session))))
}

async fn monitor_transcript(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<MonitorSnapshot>), ApiError> {
    verify_monitor_request(&state, &headers, &body)?;
    let payload: TranscriptPayload = serde_json::from_slice(&body)?;
    require_session(&payload.session_id)?;
    let session = state.monitor.write().await.ingest(
        payload.session_id.clone(),
        payload.sequence,
        payload.role.clone(),
        payload.text.clone(),
        "verified-webhook",
    )?;
    state
        .store
        .append_event(
            &payload.session_id,
            "transcript",
            &serde_json::to_value(&payload)?,
            None,
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(MonitorSnapshot::Active(session))))
}

async fn monitor_failure(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<MonitorSnapshot>), ApiError> {
    verify_monitor_request(&state, &headers, &body)?;
    let payload: FailurePayload = serde_json::from_slice(&body)?;
    require_session(&payload.session_id)?;
    let session = state.monitor.write().await.fail(
        payload.session_id.clone(),
        payload.reason.clone(),
        "verified-webhook",
    );
    state
        .store
        .append_event(
            &payload.session_id,
            "monitor_failure",
            &serde_json::to_value(&payload)?,
            None,
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(MonitorSnapshot::Active(session))))
}

async fn monitor_state(State(state): State<AppState>) -> Json<MonitorSnapshot> {
    Json(state.monitor.read().await.snapshot())
}

async fn monitor_demo(State(state): State<AppState>) -> (StatusCode, Json<MonitorSnapshot>) {
    let session_id = format!("demo_{}", now_millis());
    let session = state
        .monitor
        .write()
        .await
        .begin(session_id.clone(), true, "local-demo");
    let monitor = state.monitor.clone();
    tokio::spawn(async move {
        let turns = [
            (
                120,
                "caller",
                "Hi, this is Morgan from Harbour Coffee Roasters about tomorrow's delivery, reference HCR-204.",
            ),
            (400, "staff", "Hi Morgan, what did you need to confirm?"),
            (
                400,
                "caller",
                "Our payment details have changed, so update the bank account today.",
            ),
            (
                400,
                "caller",
                "It must happen now or the delivery may be cancelled.",
            ),
        ];
        for (sequence, (delay, role, text)) in turns.into_iter().enumerate() {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let _ = monitor.write().await.ingest(
                session_id.clone(),
                Some(sequence as i64),
                role.to_owned(),
                text.to_owned(),
                "local-demo",
            );
        }
    });
    (StatusCode::ACCEPTED, Json(MonitorSnapshot::Active(session)))
}

async fn monitor_events(
    State(state): State<AppState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let (snapshot, receiver) = {
        let hub = state.monitor.read().await;
        (
            serde_json::to_string(&hub.snapshot())
                .unwrap_or_else(|_| "{\"status\":\"idle\"}".to_owned()),
            hub.subscribe(),
        )
    };
    let initial = stream::once(async move { Ok(Event::default().event("monitor").data(snapshot)) });
    let updates = BroadcastStream::new(receiver).filter_map(|message| async move {
        message
            .ok()
            .map(|(event, data)| Ok(Event::default().event(event).data(data)))
    });
    Sse::new(initial.chain(updates)).keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
}

fn verify_monitor_request(
    state: &AppState,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), ApiError> {
    verify_monitor(
        body,
        header(headers, "x-jellyphish-timestamp"),
        header(headers, "x-jellyphish-signature"),
        state.config.monitor_webhook_secret.as_deref(),
    )
    .map_err(ApiError::signature)
}

fn verify_tool_key(headers: &HeaderMap, expected: Option<&str>) -> Result<(), ApiError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let supplied = header(headers, "authorization").and_then(|value| value.strip_prefix("Bearer "));
    if supplied == Some(expected) {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "tool authorization is invalid".to_owned(),
        ))
    }
}

fn require_session(session_id: &str) -> Result<(), ApiError> {
    if session_id.trim().is_empty() {
        Err(ApiError::bad_request("session_id is required"))
    } else {
        Ok(())
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn collection_value(raw: &Value, key: &str) -> String {
    raw.get(key)
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn non_empty_value(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[derive(Debug)]
struct ApiError(StatusCode, String);

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    fn signature(error: SignatureError) -> Self {
        let status = if matches!(error, SignatureError::MissingSecret) {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::UNAUTHORIZED
        };
        Self(status, error.to_string())
    }
}

impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, error.into().to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{HeaderValue, Request},
    };
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::ServiceExt;

    async fn test_app() -> Router {
        test_app_with(None).await
    }

    async fn test_app_with(monitor_secret: Option<&str>) -> Router {
        let config = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            database_url: "sqlite::memory:".to_owned(),
            web_root: std::path::PathBuf::from("../web"),
            elevenlabs_api_key: None,
            elevenlabs_agent_id: "test-agent".to_owned(),
            elevenlabs_webhook_secret: None,
            monitor_webhook_secret: monitor_secret.map(str::to_owned),
            tool_api_key: None,
            human_transfer_number: None,
        };
        router(build_state(config).await.unwrap())
    }

    fn signed_headers(body: &[u8], secret: &str) -> (HeaderValue, HeaderValue) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        (
            HeaderValue::from_str(&timestamp).unwrap(),
            HeaderValue::from_str(&signature).unwrap(),
        )
    }

    #[tokio::test]
    async fn assess_endpoint_denies_red_transfer() {
        let response = test_app().await.oneshot(
            Request::post("/tool/assess")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"call_id":"test-1","requested_action":"Install TeamViewer for remote access","proposed_action":"transfer"}"#))
                .unwrap(),
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(body["risk_level"], "red");
        assert_eq!(body["transfer_allowed"], false);
        assert_eq!(body["proposed_action_allowed"], false);
    }

    #[tokio::test]
    async fn tool_cannot_self_assert_a_verified_supplier() {
        let response = test_app().await.oneshot(
            Request::post("/tool/assess")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"call_id":"test-2","claimed_company":"North Star Hair Supply","business_caller":true,"known_contact":true,"verified_reference":true,"requested_action":"speak to the owner about a delivery","proposed_action":"transfer"}"#))
                .unwrap(),
        ).await.unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(body["risk_level"], "amber");
        assert_eq!(body["transfer_allowed"], false);
    }

    #[tokio::test]
    async fn signed_handoff_streams_alert_and_preserves_connection() {
        let app = test_app_with(Some("monitor-secret")).await;
        let body = br#"{"session_id":"supplier-1","disclosure_given":true}"#;
        let (timestamp, signature) = signed_headers(body, "monitor-secret");
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/webhooks/handoff")
                    .header("content-type", "application/json")
                    .header("x-jellyphish-timestamp", timestamp)
                    .header("x-jellyphish-signature", signature)
                    .body(Body::from(body.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let transcript = br#"{"session_id":"supplier-1","sequence":1,"role":"caller","text":"Our payment details changed, update the bank account today."}"#;
        let (timestamp, signature) = signed_headers(transcript, "monitor-secret");
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/webhooks/transcript")
                    .header("content-type", "application/json")
                    .header("x-jellyphish-timestamp", timestamp)
                    .header("x-jellyphish-signature", signature)
                    .body(Body::from(transcript.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let response = app
            .oneshot(
                Request::get("/api/monitor/state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(body["handoffStatus"], "connected");
        assert_eq!(body["monitorStatus"], "monitoring");
        assert_eq!(body["aiMuted"], true);
        assert_eq!(body["alerts"][0]["severity"], "high");
        assert_eq!(
            body["alerts"][0]["signals"][0]["id"],
            "payment_or_account_change"
        );
    }
}
