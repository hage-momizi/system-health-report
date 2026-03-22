use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub ai_assistant: AiAssistant,
    pub notifications: Notifications,
    #[serde(rename = "notifications(Discord BOT)")]
    pub notifications_discord_bot: Option<DiscordBotNotifications>,
    pub system_metrics: SystemMetrics,
    pub standard_logs: Vec<String>,
    pub application_logs: HashMap<String, ApplicationLog>,
    pub auto_fix: AutoFix,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAssistant {
    pub active_provider: String,
    pub providers: AiProviders,
    pub schedule: Schedule,
    #[serde(default = "default_language")]
    pub language: String,
}

fn default_language() -> String {
    "japanese".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProviders {
    pub openrouter: Option<AiProviderConfig>,
    pub openai: Option<AiProviderConfig>,
    pub anthropic: Option<AiProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProviderConfig {
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub enabled: bool,
    pub times: Vec<String>,
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notifications {
    pub provider: String,
    pub webhook_url: String,
    pub embed: EmbedConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedConfig {
    pub title: String,
    pub color: u32,
    pub show_metrics: bool,
    pub show_errors: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordBotNotifications {
    pub discord_bot_token: String,
    pub channel_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub cpu: CpuConfig,
    pub memory: MemoryConfig,
    pub load_average: LoadAverageConfig,
    pub disk_status: DiskConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuConfig {
    pub enabled: bool,
    pub collection_mode: String,
    pub interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadAverageConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskConfig {
    pub enabled: bool,
    pub mount_points: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationLog {
    pub enabled: bool,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoFix {
    pub enabled: bool,
    pub workspace: String,
    pub allow_execution: bool,
    pub backup_before_fix: bool,
}

impl Config {
    pub fn load(path: &PathBuf) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: Config = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
        Ok(config)
    }
}
