mod auth;
mod config;
mod db;
mod elevenlabs;
mod live;
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
use live::{LiveHub, LiveSnapshot, LiveUpdate};
use monitor::{HandoffRoom, MonitorHub, MonitorSnapshot, RingParams};
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
    live: Arc<RwLock<LiveHub>>,
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
        live: Arc::new(RwLock::new(LiveHub::default())),
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
        .route("/api/monitor/{line}/events", get(monitor_events))
        .route("/api/monitor/{line}/state", get(monitor_state))
        .route("/api/monitor/{line}/demo", post(monitor_demo))
        .route("/api/handoff/{line}/ring", post(handoff_ring))
        .route("/api/handoff/{line}/signal", post(handoff_signal))
        .route("/api/handoff/{line}/hangup", post(handoff_hangup))
        .route(
            "/api/handoff/{line}/transcript",
            post(handoff_local_transcript),
        )
        .route(
            "/api/handoff/{line}/transcribe",
            post(handoff_transcribe_audio),
        )
        .route("/api/handoff/{line}/{id}", get(handoff_room))
        .route("/api/live/{line}", get(live_state).post(live_update))
        .route("/api/live/{line}/events", get(live_events))
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
        "lines": {
            "studio-sol": { "agentId": state.config.elevenlabs_agent_id },
            "dental-clinic": { "agentId": state.config.elevenlabs_dental_agent_id },
        },
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
struct ConversationListQuery {
    agent_id: Option<String>,
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

async fn conversations(
    State(state): State<AppState>,
    Query(query): Query<ConversationListQuery>,
) -> Result<Json<Value>, ApiError> {
    let agent_id = query.agent_id.as_deref();
    if let Some(id) = agent_id {
        let dental = state.config.elevenlabs_dental_agent_id.as_deref();
        if id != state.config.elevenlabs_agent_id && dental != Some(id) {
            return Err(ApiError::bad_request("unknown agent_id"));
        }
    }
    let mut value = state.elevenlabs.conversations(agent_id).await?;
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
                    "transfer_allowed": matches!(
                        summary.risk_level,
                        RiskLevel::Green | RiskLevel::Amber
                    ),
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
    let risk_level = parse_risk_level(&value["data"]["route"]);
    let decision = evaluate(&CallerContext {
        call_id: id.clone(),
        requested_action: value["data"]["requested_action"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        claimed_company: value["data"]["claimed_company"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        transcript,
        risk_level,
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
            risk_level: parse_risk_level(&json!(collection_value(
                &event["data"]["analysis"]["data_collection_results"],
                "route",
            ))),
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
    #[serde(default = "default_line")]
    line: String,
    session_id: String,
    #[serde(default)]
    disclosure_given: bool,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct TranscriptPayload {
    #[serde(default = "default_line")]
    line: String,
    session_id: String,
    sequence: Option<i64>,
    role: String,
    text: String,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct FailurePayload {
    #[serde(default = "default_line")]
    line: String,
    session_id: String,
    #[serde(default = "default_failure")]
    reason: String,
}

fn default_failure() -> String {
    "Monitoring sidecar unavailable".to_owned()
}

fn default_line() -> String {
    "studio-sol".to_owned()
}

async fn monitor_handoff(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<MonitorSnapshot>), ApiError> {
    verify_monitor_request(&state, &headers, &body)?;
    let payload: HandoffPayload = serde_json::from_slice(&body)?;
    known_line(&payload.line)?;
    require_session(&payload.session_id)?;
    let session = state.monitor.write().await.begin(
        payload.line.clone(),
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
    Ok((
        StatusCode::ACCEPTED,
        Json(MonitorSnapshot::Active(Box::new(session))),
    ))
}

async fn monitor_transcript(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<MonitorSnapshot>), ApiError> {
    verify_monitor_request(&state, &headers, &body)?;
    let payload: TranscriptPayload = serde_json::from_slice(&body)?;
    known_line(&payload.line)?;
    require_session(&payload.session_id)?;
    let session = state.monitor.write().await.ingest(
        payload.line.clone(),
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
    Ok((
        StatusCode::ACCEPTED,
        Json(MonitorSnapshot::Active(Box::new(session))),
    ))
}

async fn monitor_failure(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<MonitorSnapshot>), ApiError> {
    verify_monitor_request(&state, &headers, &body)?;
    let payload: FailurePayload = serde_json::from_slice(&body)?;
    known_line(&payload.line)?;
    require_session(&payload.session_id)?;
    let session = state.monitor.write().await.fail(
        payload.line.clone(),
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
    Ok((
        StatusCode::ACCEPTED,
        Json(MonitorSnapshot::Active(Box::new(session))),
    ))
}

async fn monitor_state(
    State(state): State<AppState>,
    Path(line): Path<String>,
) -> Result<Json<MonitorSnapshot>, ApiError> {
    known_line(&line)?;
    Ok(Json(state.monitor.read().await.snapshot(&line)))
}

async fn monitor_demo(
    State(state): State<AppState>,
    Path(line): Path<String>,
) -> Result<(StatusCode, Json<MonitorSnapshot>), ApiError> {
    known_line(&line)?;
    let session_id = format!("demo_{}", now_millis());
    let session =
        state
            .monitor
            .write()
            .await
            .begin(line.clone(), session_id.clone(), true, "local-demo");
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
                line.clone(),
                session_id.clone(),
                Some(sequence as i64),
                role.to_owned(),
                text.to_owned(),
                "local-demo",
            );
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(MonitorSnapshot::Active(Box::new(session))),
    ))
}

#[derive(Debug, Deserialize)]
struct RingRequest {
    session_id: String,
    #[serde(default)]
    disclosure_given: bool,
    caller_name: Option<String>,
    requested_action: Option<String>,
    caption_key: String,
    offer: Value,
}

#[derive(Debug, Deserialize)]
struct SignalRequest {
    session_id: String,
    from: String,
    kind: String,
    payload: Value,
}

#[derive(Debug, Deserialize)]
struct HangupRequest {
    session_id: String,
    #[serde(default = "default_from")]
    from: String,
}

fn default_from() -> String {
    "staff".to_owned()
}

async fn handoff_ring(
    State(state): State<AppState>,
    Path(line): Path<String>,
    Json(request): Json<RingRequest>,
) -> Result<(StatusCode, Json<HandoffRoom>), ApiError> {
    known_line(&line)?;
    require_session(&request.session_id)?;
    let room = state
        .monitor
        .write()
        .await
        .ring(RingParams {
            line,
            session_id: request.session_id.clone(),
            disclosure_given: request.disclosure_given,
            caller_name: request.caller_name.clone(),
            requested_action: request.requested_action.clone(),
            caption_key: request.caption_key,
            offer: request.offer,
            source: "browser-handoff",
        })
        .map_err(handoff_error)?;
    state
        .store
        .append_event(
            &request.session_id,
            "handoff_ring",
            &json!({
                "disclosure_given": request.disclosure_given,
                "caller_name": request.caller_name,
                "requested_action": request.requested_action,
            }),
            None,
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(room)))
}

async fn handoff_signal(
    State(state): State<AppState>,
    Path(line): Path<String>,
    Json(request): Json<SignalRequest>,
) -> Result<(StatusCode, Json<HandoffRoom>), ApiError> {
    known_line(&line)?;
    require_session(&request.session_id)?;
    let room = state
        .monitor
        .write()
        .await
        .signal(
            line,
            request.session_id,
            request.from,
            request.kind,
            request.payload,
        )
        .map_err(handoff_error)?;
    Ok((StatusCode::ACCEPTED, Json(room)))
}

async fn handoff_hangup(
    State(state): State<AppState>,
    Path(line): Path<String>,
    Json(request): Json<HangupRequest>,
) -> Result<(StatusCode, Json<HandoffRoom>), ApiError> {
    known_line(&line)?;
    require_session(&request.session_id)?;
    let room = state
        .monitor
        .write()
        .await
        .hangup(line, request.session_id.clone(), &request.from)
        .map_err(handoff_error)?;
    state
        .store
        .append_event(
            &request.session_id,
            "handoff_hangup",
            &json!({ "from": request.from }),
            None,
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(room)))
}

async fn handoff_local_transcript(
    State(state): State<AppState>,
    Path(line): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<TranscriptPayload>,
) -> Result<(StatusCode, Json<MonitorSnapshot>), ApiError> {
    known_line(&line)?;
    require_session(&payload.session_id)?;
    if !state.monitor.read().await.accepts_captions(
        &line,
        &payload.session_id,
        header(&headers, "x-caption-key").unwrap_or_default(),
    ) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "handoff is not connected".to_owned(),
        ));
    }
    let session = state
        .monitor
        .write()
        .await
        .ingest(
            line,
            payload.session_id.clone(),
            payload.sequence,
            payload.role.clone(),
            payload.text.clone(),
            "browser-handoff",
        )
        .map_err(handoff_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(MonitorSnapshot::Active(Box::new(session))),
    ))
}

async fn handoff_transcribe_audio(
    State(state): State<AppState>,
    Path(line): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    known_line(&line)?;
    let session_id = header(&headers, "x-session-id").unwrap_or_default();
    let role = header(&headers, "x-role").unwrap_or_default();
    require_session(session_id)?;
    if !matches!(role, "caller" | "staff") {
        return Err(ApiError::bad_request("role must be caller or staff"));
    }
    if !state.monitor.read().await.accepts_captions(
        &line,
        session_id,
        header(&headers, "x-caption-key").unwrap_or_default(),
    ) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "handoff is not connected".to_owned(),
        ));
    }
    if body.len() < 200 {
        return Ok((StatusCode::OK, Json(json!({ "text": "" }))));
    }
    if !state.elevenlabs.configured() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "speech-to-text is not configured".to_owned(),
        ));
    }
    let content_type = header(&headers, "content-type").unwrap_or("application/octet-stream");
    let extension = if content_type.contains("webm") {
        "webm"
    } else if content_type.contains("mp4") {
        "mp4"
    } else {
        "audio"
    };
    let text = state
        .elevenlabs
        .transcribe_audio(body.to_vec(), content_type, &format!("speech.{extension}"))
        .await
        .map_err(|error| ApiError(StatusCode::BAD_GATEWAY, error.to_string()))?;
    if text.is_empty() {
        return Ok((StatusCode::OK, Json(json!({ "text": "" }))));
    }
    let session = state
        .monitor
        .write()
        .await
        .ingest(
            line,
            session_id.to_owned(),
            None,
            role.to_owned(),
            text.clone(),
            "browser-handoff",
        )
        .map_err(handoff_error)?;
    Ok((
        StatusCode::OK,
        Json(json!({ "text": text, "snapshot": session })),
    ))
}

async fn handoff_room(
    State(state): State<AppState>,
    Path((line, id)): Path<(String, String)>,
) -> Result<Json<HandoffRoom>, ApiError> {
    known_line(&line)?;
    require_session(&id)?;
    state
        .monitor
        .read()
        .await
        .room(&line, &id)
        .map(Json)
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                "no handoff for this session".to_owned(),
            )
        })
}

fn handoff_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("already answered") {
        ApiError(StatusCode::CONFLICT, message)
    } else {
        ApiError::bad_request(message)
    }
}

fn known_line(line: &str) -> Result<(), ApiError> {
    if LiveHub::is_line(line) {
        Ok(())
    } else {
        Err(ApiError(StatusCode::NOT_FOUND, "unknown line".to_owned()))
    }
}

async fn live_state(
    State(state): State<AppState>,
    Path(line): Path<String>,
) -> Result<Json<LiveSnapshot>, ApiError> {
    known_line(&line)?;
    Ok(Json(state.live.write().await.snapshot(&line)))
}

async fn live_update(
    State(state): State<AppState>,
    Path(line): Path<String>,
    Json(update): Json<LiveUpdate>,
) -> Result<StatusCode, ApiError> {
    known_line(&line)?;
    let applied = state
        .live
        .write()
        .await
        .apply(&line, update)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(if applied {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    })
}

async fn live_events(
    State(state): State<AppState>,
    Path(line): Path<String>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    known_line(&line)?;
    let (snapshot, receiver) = {
        let mut hub = state.live.write().await;
        (
            serde_json::to_string(&hub.snapshot(&line)).unwrap_or_default(),
            hub.subscribe(),
        )
    };
    let initial = stream::once(async move { Ok(Event::default().event("live").data(snapshot)) });
    let updates = BroadcastStream::new(receiver).filter_map(move |message| {
        let line = line.clone();
        async move {
            message
                .ok()
                .filter(|(event_line, _)| *event_line == line)
                .map(|(_, data)| Ok(Event::default().event("live").data(data)))
        }
    });
    Ok(Sse::new(initial.chain(updates))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(20))))
}

async fn monitor_events(
    State(state): State<AppState>,
    Path(line): Path<String>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    known_line(&line)?;
    let (snapshot, receiver) = {
        let hub = state.monitor.read().await;
        (
            serde_json::to_string(&hub.snapshot(&line))
                .unwrap_or_else(|_| "{\"status\":\"idle\"}".to_owned()),
            hub.subscribe(),
        )
    };
    let initial = stream::once(async move { Ok(Event::default().event("monitor").data(snapshot)) });
    let updates = BroadcastStream::new(receiver).filter_map(move |message| {
        let line = line.clone();
        async move {
            message
                .ok()
                .filter(|(event_line, _, _)| *event_line == line)
                .map(|(_, event, data)| Ok(Event::default().event(event).data(data)))
        }
    });
    Ok(Sse::new(initial.chain(updates))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(20))))
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

fn parse_risk_level(value: &Value) -> Option<RiskLevel> {
    serde_json::from_value(value.clone()).ok().or_else(|| {
        value
            .as_str()
            .or_else(|| value.get("value").and_then(Value::as_str))
            .and_then(|raw| {
                serde_json::from_str(&format!("\"{}\"", raw.trim().to_ascii_lowercase())).ok()
            })
    })
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
            elevenlabs_dental_agent_id: None,
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
                .body(Body::from(r#"{"call_id":"test-1","risk_level":"red","requested_action":"Install TeamViewer for remote access","proposed_action":"transfer"}"#))
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
                .body(Body::from(r#"{"call_id":"test-2","risk_level":"amber","caller_name":"Morgan","claimed_company":"North Star Hair Supply","reference":"NS-204","business_caller":true,"known_contact":true,"verified_reference":true,"requested_action":"speak to the owner about a delivery","proposed_action":"transfer"}"#))
                .unwrap(),
        ).await.unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(body["risk_level"], "amber");
        assert_eq!(body["transfer_allowed"], true);
        assert_eq!(body["proposed_action_allowed"], true);
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
                Request::get("/api/monitor/studio-sol/state")
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

    fn live_update(call_id: &str, seq: u64, status: &str) -> Request<Body> {
        let body = json!({
            "callId": call_id,
            "seq": seq,
            "status": status,
            "callerNumber": "+61 400 000 000",
            "transcript": [
                { "role": "user", "message": "Can I book a haircut tomorrow?" },
                { "role": "system", "message": "not a caller or agent turn" }
            ]
        });
        Request::post("/api/live/studio-sol")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn live_snapshot(app: &Router) -> Value {
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/live/studio-sol")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn live_call_relays_to_dashboard_and_ignores_stale_updates() {
        let app = test_app().await;
        let status = |request: Request<Body>| {
            let app = app.clone();
            async move { app.oneshot(request).await.unwrap().status() }
        };

        assert_eq!(
            status(live_update("call-1", 1, "connected")).await,
            StatusCode::ACCEPTED
        );
        let body = live_snapshot(&app).await;
        assert_eq!(body["call"]["status"], "connected");
        assert_eq!(body["call"]["callerNumber"], "+61 400 000 000");
        assert!(body["call"]["startedAt"].is_i64());
        assert_eq!(body["call"]["transcript"].as_array().unwrap().len(), 1);
        assert!(body["call"]["riskLevel"].is_null());

        // Out-of-order snapshots are dropped.
        assert_eq!(
            status(live_update("call-1", 1, "connected")).await,
            StatusCode::OK
        );
        assert_eq!(
            status(live_update("call-1", 2, "ended")).await,
            StatusCode::ACCEPTED
        );
        // A heartbeat that raced the hang-up must not revive the call.
        assert_eq!(
            status(live_update("call-1", 3, "connected")).await,
            StatusCode::OK
        );
        let body = live_snapshot(&app).await;
        assert_eq!(body["call"]["status"], "ended");
        assert!(body["call"]["endedAt"].is_i64());

        // The next call on the line replaces the finished one.
        assert_eq!(
            status(live_update("call-2", 1, "connecting")).await,
            StatusCode::ACCEPTED
        );
        let body = live_snapshot(&app).await;
        assert_eq!(body["call"]["callId"], "call-2");
        assert!(body["call"]["startedAt"].is_null());
    }

    #[tokio::test]
    async fn live_call_relays_pending_transcript_turns() {
        let app = test_app().await;
        let body = json!({
            "callId": "call-stream",
            "seq": 1,
            "status": "connected",
            "transcript": [
                { "role": "agent", "message": "Thanks for calling", "pending": true },
                { "role": "user", "message": "Hi I need a", "pending": true }
            ]
        });
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/live/studio-sol")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let snapshot = live_snapshot(&app).await;
        assert_eq!(snapshot["call"]["transcript"][0]["pending"], true);
        assert_eq!(snapshot["call"]["transcript"][1]["message"], "Hi I need a");
        assert_eq!(snapshot["call"]["transcript"][1]["pending"], true);
        assert!(snapshot["call"]["riskLevel"].is_null());
    }

    #[tokio::test]
    async fn live_snapshot_relays_the_agents_route() {
        let app = test_app().await;
        let body = json!({
            "callId": "scam-1",
            "seq": 1,
            "status": "connected",
            "riskLevel": "red",
            "transcript": [{
                "role": "user",
                "message": "I'm calling from Microsoft tech support. Please pay them now."
            }]
        });
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/live/studio-sol")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let snapshot = live_snapshot(&app).await;
        assert_eq!(snapshot["call"]["riskLevel"], "red");
    }

    #[tokio::test]
    async fn live_relay_rejects_unknown_lines_and_statuses() {
        let app = test_app().await;
        let response = app
            .clone()
            .oneshot(Request::get("/api/live/nope").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .oneshot(live_update("call-1", 1, "transferred"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn browser_handoff_rings_then_connects() {
        let app = test_app().await;
        let ring = serde_json::json!({
            "session_id": "conv_staff_1",
            "disclosure_given": true,
            "caller_name": "Jamie",
            "requested_action": "haircut tomorrow",
            "caption_key": "caption-secret-1",
            "offer": { "type": "offer", "sdp": "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n" }
        });
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/handoff/studio-sol/ring")
                    .header("content-type", "application/json")
                    .body(Body::from(ring.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let state = app
            .clone()
            .oneshot(
                Request::get("/api/monitor/studio-sol/state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(state.into_body(), 64 * 1024).await.unwrap()).unwrap();
        assert_eq!(body["handoffStatus"], "ringing");
        assert_eq!(body["aiMuted"], true);

        let dental = app
            .clone()
            .oneshot(
                Request::get("/api/monitor/dental-clinic/state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let dental: Value =
            serde_json::from_slice(&to_bytes(dental.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(dental["status"], "idle");

        let wrong_line = app
            .clone()
            .oneshot(
                Request::get("/api/handoff/dental-clinic/conv_staff_1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong_line.status(), StatusCode::NOT_FOUND);

        let answer = serde_json::json!({
            "session_id": "conv_staff_1",
            "from": "staff",
            "kind": "answer",
            "payload": { "type": "answer", "sdp": "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n" }
        });
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/handoff/studio-sol/signal")
                    .header("content-type", "application/json")
                    .body(Body::from(answer.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let connected = app
            .oneshot(
                Request::get("/api/monitor/studio-sol/state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(connected.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(body["handoffStatus"], "connected");
    }

    #[tokio::test]
    async fn browser_handoff_keeps_staff_and_caller_captions() {
        let app = test_app().await;
        let ring = serde_json::json!({
            "session_id": "conv_staff_2",
            "disclosure_given": true,
            "caption_key": "caption-secret-2",
            "offer": { "type": "offer", "sdp": "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n" }
        });
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/handoff/studio-sol/ring")
                    .header("content-type", "application/json")
                    .body(Body::from(ring.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let answer = serde_json::json!({
            "session_id": "conv_staff_2",
            "from": "staff",
            "kind": "answer",
            "payload": { "type": "answer", "sdp": "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n" }
        });
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/handoff/studio-sol/signal")
                    .header("content-type", "application/json")
                    .body(Body::from(answer.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let first = serde_json::json!({
            "session_id": "conv_staff_2",
            "role": "staff",
            "text": "Studio Sol, this is Alex."
        });
        let rejected = app
            .clone()
            .oneshot(
                Request::post("/api/handoff/studio-sol/transcript")
                    .header("content-type", "application/json")
                    .header("x-caption-key", "wrong-secret")
                    .body(Body::from(first.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/api/handoff/studio-sol/transcript")
                        .header("content-type", "application/json")
                        .header("x-caption-key", "caption-secret-2")
                        .body(Body::from(first.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::ACCEPTED);
        }
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/handoff/studio-sol/transcript")
                    .header("content-type", "application/json")
                    .header("x-caption-key", "caption-secret-2")
                    .body(Body::from(
                        serde_json::json!({
                            "session_id": "conv_staff_2",
                            "role": "caller",
                            "text": "Hi, I booked a haircut."
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let state = app
            .clone()
            .oneshot(
                Request::get("/api/monitor/studio-sol/state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(state.into_body(), 64 * 1024).await.unwrap()).unwrap();
        assert_eq!(body["transcript"].as_array().unwrap().len(), 2);
        assert_eq!(body["transcript"][0]["role"], "staff");
        assert_eq!(body["transcript"][1]["role"], "caller");

        let silent = app
            .clone()
            .oneshot(
                Request::post("/api/handoff/studio-sol/transcribe")
                    .header("x-session-id", "conv_staff_2")
                    .header("x-role", "staff")
                    .header("x-caption-key", "caption-secret-2")
                    .header("content-type", "audio/webm")
                    .body(Body::from(vec![1, 2, 3, 4]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(silent.status(), StatusCode::OK);

        let hangup = app
            .clone()
            .oneshot(
                Request::post("/api/handoff/studio-sol/hangup")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "session_id": "conv_staff_2",
                            "from": "caller"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(hangup.status(), StatusCode::ACCEPTED);

        let final_caption = app
            .oneshot(
                Request::post("/api/handoff/studio-sol/transcript")
                    .header("content-type", "application/json")
                    .header("x-caption-key", "caption-secret-2")
                    .body(Body::from(
                        serde_json::json!({
                            "session_id": "conv_staff_2",
                            "role": "caller",
                            "text": "Thanks, goodbye."
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(final_caption.status(), StatusCode::ACCEPTED);
    }
}
