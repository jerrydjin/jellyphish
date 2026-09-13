use serde_json::{Value, json};

#[derive(Clone)]
pub struct ElevenLabsClient {
    client: reqwest::Client,
    api_key: Option<String>,
    agent_id: String,
}

impl ElevenLabsClient {
    pub fn new(api_key: Option<String>, agent_id: String) -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(12))
                .build()?,
            api_key,
            agent_id,
        })
    }

    pub fn configured(&self) -> bool {
        self.api_key.is_some()
    }

    pub async fn conversations(&self) -> anyhow::Result<Value> {
        let fields = [
            "route",
            "caller_name",
            "claimed_company",
            "reference",
            "requested_action",
            "risk_signals",
            "outcome",
            "distraction_turns",
        ];
        let mut url = reqwest::Url::parse("https://api.elevenlabs.io/v1/convai/conversations")?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("agent_id", &self.agent_id)
                .append_pair("page_size", "30")
                .append_pair("summary_mode", "include")
                .append_pair("sort_direction", "desc");
            for field in fields {
                query.append_pair("data_collection_ids", field);
            }
        }
        let raw = self.get(url).await?;
        let conversations = raw["conversations"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| {
                json!({
                    "id": item["conversation_id"],
                    "agentName": item["agent_name"],
                    "startedAt": item["start_time_unix_secs"],
                    "duration": item["call_duration_secs"].as_i64().unwrap_or_default(),
                    "messageCount": item["message_count"].as_i64().unwrap_or_default(),
                    "status": item["status"],
                    "success": item["call_successful"],
                    "title": item["call_summary_title"],
                    "summary": item["transcript_summary"],
                    "source": item["conversation_initiation_source"],
                    "direction": item["direction"],
                    "data": data_collection(&item["data_collection_results"]),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "conversations": conversations, "refreshedAt": crate::db::now_millis() }))
    }

    pub async fn conversation(&self, id: &str) -> anyhow::Result<Value> {
        if !id.starts_with("conv_") || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            anyhow::bail!("invalid conversation id");
        }
        let url = reqwest::Url::parse(&format!(
            "https://api.elevenlabs.io/v1/convai/conversations/{id}"
        ))?;
        let item = self.get(url).await?;
        let analysis = &item["analysis"];
        let transcript = item["transcript"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|turn| {
                let role = turn["role"].as_str()?;
                let message = turn["message"].as_str()?.trim();
                matches!(role, "agent" | "user").then(|| {
                    json!({
                        "role": role,
                        "message": message,
                        "time": turn["time_in_call_secs"].as_f64().unwrap_or_default(),
                        "interrupted": turn["interrupted"].as_bool().unwrap_or(false),
                    })
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "id": item["conversation_id"],
            "agentName": item["agent_name"],
            "status": item["status"],
            "startedAt": item["metadata"]["start_time_unix_secs"],
            "duration": item["metadata"]["call_duration_secs"].as_i64().unwrap_or_default(),
            "source": item["metadata"]["conversation_initiation_source"],
            "terminationReason": item["metadata"]["termination_reason"],
            "success": analysis["call_successful"],
            "score": analysis["call_success_score"],
            "title": analysis["call_summary_title"],
            "summary": analysis["transcript_summary"],
            "sentiment": analysis["sentiment_analysis"]["overall_label"],
            "data": data_collection(&analysis["data_collection_results"]),
            "transcript": transcript,
        }))
    }

    async fn get(&self, url: reqwest::Url) -> anyhow::Result<Value> {
        let key = self
            .api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("ELEVENLABS_API_KEY is missing"))?;
        let response = self
            .client
            .get(url)
            .header("xi-api-key", key)
            .send()
            .await?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("ElevenLabs returned {}: {}", status, body["detail"]);
        }
        Ok(body)
    }
}

fn data_collection(raw: &Value) -> Value {
    let Some(object) = raw.as_object() else {
        return json!({});
    };
    Value::Object(
        object
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value.get("value").cloned().unwrap_or_else(|| value.clone()),
                )
            })
            .collect(),
    )
}
