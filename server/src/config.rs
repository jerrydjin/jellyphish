use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    pub web_root: PathBuf,
    pub elevenlabs_api_key: Option<String>,
    pub elevenlabs_agent_id: String,
    pub elevenlabs_dental_agent_id: Option<String>,
    pub elevenlabs_webhook_secret: Option<String>,
    pub monitor_webhook_secret: Option<String>,
    pub tool_api_key: Option<String>,
    pub human_transfer_number: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();
        Ok(Self {
            bind: listen_addr()?,
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://data/jellyphish.db".to_owned()),
            web_root: web_root(),
            elevenlabs_api_key: non_empty("ELEVENLABS_API_KEY"),
            elevenlabs_agent_id: std::env::var("ELEVENLABS_AGENT_ID")
                .unwrap_or_else(|_| "agent_1101m2b4acdnedz9y2yky3w9vwfm".to_owned()),
            elevenlabs_dental_agent_id: non_empty("ELEVENLABS_DENTAL_AGENT_ID"),
            elevenlabs_webhook_secret: non_empty("ELEVENLABS_WEBHOOK_SECRET"),
            monitor_webhook_secret: non_empty("MONITOR_WEBHOOK_SECRET"),
            tool_api_key: non_empty("TOOL_API_KEY"),
            human_transfer_number: non_empty("HUMAN_TRANSFER_NUMBER"),
        })
    }
}

fn listen_addr() -> anyhow::Result<SocketAddr> {
    if let Some(bind) = non_empty("BIND_ADDR") {
        return Ok(bind.parse()?);
    }
    if let Some(port) = non_empty("PORT") {
        return Ok(format!("0.0.0.0:{port}").parse()?);
    }
    Ok("127.0.0.1:4173".parse()?)
}

fn web_root() -> PathBuf {
    non_empty("WEB_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web"))
}

fn non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}
