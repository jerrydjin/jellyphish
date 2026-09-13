use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    pub web_root: PathBuf,
    pub elevenlabs_api_key: Option<String>,
    pub elevenlabs_agent_id: String,
    pub elevenlabs_webhook_secret: Option<String>,
    pub monitor_webhook_secret: Option<String>,
    pub tool_api_key: Option<String>,
    pub human_transfer_number: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        Ok(Self {
            bind: std::env::var("BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:4173".to_owned())
                .parse()?,
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://data/jellyphish.db".to_owned()),
            web_root: repo_root.join("web"),
            elevenlabs_api_key: non_empty("ELEVENLABS_API_KEY"),
            elevenlabs_agent_id: std::env::var("ELEVENLABS_AGENT_ID")
                .unwrap_or_else(|_| "agent_1101m2b4acdnedz9y2yky3w9vwfm".to_owned()),
            elevenlabs_webhook_secret: non_empty("ELEVENLABS_WEBHOOK_SECRET"),
            monitor_webhook_secret: non_empty("MONITOR_WEBHOOK_SECRET"),
            tool_api_key: non_empty("TOOL_API_KEY"),
            human_transfer_number: non_empty("HUMAN_TRANSFER_NUMBER"),
        })
    }
}

fn non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}
