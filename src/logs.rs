use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};

use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogCollection {
    pub logs: HashMap<String, String>,
}

impl LogCollection {
    pub fn new() -> Self {
        LogCollection {
            logs: HashMap::new(),
        }
    }
}

impl Default for LogCollection {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn collect_logs(config: &Config) -> Result<LogCollection> {
    info!("Starting log collection");
    let mut collection = LogCollection::new();

    // Collect standard logs
    for log_path in &config.standard_logs {
        match read_last_lines(log_path, 100) {
            Ok(content) => {
                collection.logs.insert(log_path.clone(), content);
            }
            Err(e) => {
                warn!("Failed to read log {}: {}", log_path, e);
                collection
                    .logs
                    .insert(log_path.clone(), format!("[Error reading log: {}]", e));
            }
        }
    }

    // Collect application logs
    for (name, app_log) in &config.application_logs {
        if !app_log.enabled {
            continue;
        }
        match read_last_lines(&app_log.path, 100) {
            Ok(content) => {
                collection.logs.insert(app_log.path.clone(), content);
            }
            Err(e) => {
                warn!("Failed to read {} log at {}: {}", name, app_log.path, e);
                collection.logs.insert(
                    app_log.path.clone(),
                    format!("[Error reading {} log: {}]", name, e),
                );
            }
        }
    }

    info!(
        "Log collection complete. Collected {} log files",
        collection.logs.len()
    );
    Ok(collection)
}

fn read_last_lines(path: &str, n: usize) -> Result<String> {
    let path = Path::new(path);

    if !path.exists() {
        return Err(anyhow::anyhow!("File does not exist: {}", path.display()));
    }

    let bytes = std::fs::read(path)?;

    // Check if file is likely binary by looking for null bytes
    if bytes.contains(&0u8) {
        return Err(anyhow::anyhow!(
            "File appears to be binary: {}",
            path.display()
        ));
    }

    let content = String::from_utf8_lossy(&bytes).to_string();
    let lines: Vec<&str> = content.lines().collect();

    let start = if lines.len() > n {
        lines.len() - n
    } else {
        0
    };

    let result = lines[start..].join("\n");
    Ok(result)
}
