use anyhow::{Context, Result};
use notify::{recommended_watcher, Event, EventKind, RecursiveMode, Watcher};
use reqwest::Client;
use serde_json::json;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use tracing::{error, info, warn};

use crate::config::Config;

pub async fn run_watch_mode(config: Arc<Config>) -> Result<()> {
    // Determine history file path
    let history_path = std::env::var("HISTFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            PathBuf::from(format!("{}/.bash_history", home))
        });

    println!("=== System Health Report - Watch Mode ===");
    println!("Watching bash history: {}", history_path.display());
    println!();
    println!("To ensure real-time command tracking, add this to your ~/.bashrc:");
    println!("  export PROMPT_COMMAND='history -a'");
    println!("  export HISTFILE=~/.bash_history");
    println!();
    println!("Commands will be streamed to Discord as they are executed.");
    println!("Press Ctrl+C to stop watching.");
    println!();

    if !history_path.exists() {
        return Err(anyhow::anyhow!(
            "History file not found: {}. Set HISTFILE env var or create ~/.bash_history",
            history_path.display()
        ));
    }

    info!("Starting watch mode for {}", history_path.display());

    let webhook_url = config.notifications.webhook_url.clone();
    if webhook_url.is_empty() {
        warn!("No webhook URL configured. Commands will only be logged locally.");
    }

    let client = Client::new();

    // Use std::sync::Mutex for the position so it can be shared with a std::thread
    let last_position: Arc<Mutex<u64>> = Arc::new(Mutex::new(0u64));

    // Get current file size to start watching from current position
    {
        let file = std::fs::File::open(&history_path)
            .context("Failed to open history file")?;
        let metadata = file.metadata()?;
        let mut pos = last_position.lock().unwrap();
        *pos = metadata.len();
        info!("Starting from file position: {} bytes", *pos);
    }

    let (notify_tx, notify_rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = recommended_watcher(notify_tx).context("Failed to create file watcher")?;
    watcher
        .watch(&history_path, RecursiveMode::NonRecursive)
        .context("Failed to watch history file")?;

    info!("File watcher started");

    let history_path_clone = history_path.clone();
    let last_position_clone = last_position.clone();

    let (async_tx, mut async_rx) = tokio::sync::mpsc::channel::<String>(100);

    // Spawn a blocking std thread to process inotify events synchronously
    std::thread::spawn(move || {
        for result in notify_rx {
            match result {
                Ok(event) => {
                    if matches!(
                        event.kind,
                        EventKind::Modify(_) | EventKind::Access(_) | EventKind::Create(_)
                    ) {
                        match read_new_lines(&history_path_clone, &last_position_clone) {
                            Ok(new_lines) => {
                                for line in new_lines {
                                    if !line.trim().is_empty() {
                                        if async_tx.blocking_send(line).is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Failed to read new history lines: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Watch error: {}", e);
                }
            }
        }
    });

    // Process commands and send to Discord
    while let Some(command) = async_rx.recv().await {
        info!("New command detected: {}", command);
        println!("[HISTORY] $ {}", command);

        if !webhook_url.is_empty() {
            let formatted = format!("```bash\n{}\n```", command);
            if let Err(e) = send_to_webhook(&client, &webhook_url, &formatted).await {
                error!("Failed to send command to Discord: {}", e);
            }
        }
    }

    Ok(())
}

fn read_new_lines(path: &PathBuf, last_position: &Arc<Mutex<u64>>) -> Result<Vec<String>> {
    let pos = {
        let guard = last_position.lock().unwrap();
        *guard
    };

    let mut file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    let current_size = metadata.len();

    if current_size <= pos {
        return Ok(vec![]);
    }

    file.seek(SeekFrom::Start(pos))?;
    let reader = BufReader::new(file);
    let mut new_lines = Vec::new();

    for line in reader.lines() {
        match line {
            Ok(l) => new_lines.push(l),
            Err(_) => break,
        }
    }

    // Update position
    {
        let mut guard = last_position.lock().unwrap();
        *guard = current_size;
    }

    Ok(new_lines)
}

async fn send_to_webhook(client: &Client, webhook_url: &str, content: &str) -> Result<()> {
    let payload = json!({
        "content": content
    });

    let response = client
        .post(webhook_url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .context("Failed to send to webhook")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Webhook error {}: {}", status, body));
    }

    Ok(())
}
