use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::info;

use crate::config::Notifications;
use crate::notifications::Notifier;
use crate::report::Report;

pub struct DiscordWebhook {
    client: Client,
    config: Notifications,
}

impl DiscordWebhook {
    pub fn new(config: Notifications) -> Self {
        DiscordWebhook {
            client: Client::new(),
            config,
        }
    }

    fn build_embed(&self, report: &Report) -> Value {
        let embed_config = &self.config.embed;
        let mut fields: Vec<Value> = Vec::new();

        // CPU field
        if let Some(cpu) = &report.metrics.cpu {
            fields.push(json!({
                "name": "CPU Usage",
                "value": format!("{:.1}% (avg over {}s)", cpu.average_usage, cpu.interval_seconds),
                "inline": true
            }));
        }

        // Memory field
        if let Some(mem) = &report.metrics.memory {
            fields.push(json!({
                "name": "Memory",
                "value": format!("{}/{} MB ({:.1}%)\nSwap: {}/{} MB",
                    mem.used_mb, mem.total_mb, mem.usage_percent,
                    mem.swap_used_mb, mem.swap_total_mb),
                "inline": true
            }));
        }

        // Load average field
        if let Some(load) = &report.metrics.load_average {
            fields.push(json!({
                "name": "Load Average",
                "value": format!("{:.2} / {:.2} / {:.2}", load.one, load.five, load.fifteen),
                "inline": true
            }));
        }

        // Disk fields
        for disk in &report.metrics.disks {
            fields.push(json!({
                "name": format!("Disk {}", disk.mount_point),
                "value": format!("{:.1}/{:.1} GB ({:.1}%)", disk.used_gb, disk.total_gb, disk.usage_percent),
                "inline": true
            }));
        }

        // Log errors field (if enabled)
        if embed_config.show_errors && !report.logs.logs.is_empty() {
            let mut error_lines: Vec<String> = Vec::new();
            for (path, content) in &report.logs.logs {
                let relevant: Vec<&str> = content
                    .lines()
                    .filter(|line| {
                        let lower = line.to_lowercase();
                        lower.contains("error")
                            || lower.contains("critical")
                            || lower.contains("fatal")
                            || lower.contains("warn")
                    })
                    .take(5)
                    .collect();

                if !relevant.is_empty() {
                    error_lines.push(format!("**{}**:", path));
                    for line in relevant {
                        error_lines.push(format!("`{}`", &line[..line.len().min(100)]));
                    }
                }
            }

            if !error_lines.is_empty() {
                let mut error_text = error_lines.join("\n");
                if error_text.len() > 1024 {
                    error_text.truncate(1021);
                    error_text.push_str("...");
                }
                fields.push(json!({
                    "name": "Recent Errors/Warnings",
                    "value": error_text,
                    "inline": false
                }));
            }
        }

        // Truncate AI analysis to 2048 chars for description
        let mut description = report.ai_analysis.clone();
        if description.len() > 2048 {
            description.truncate(2045);
            description.push_str("...");
        }

        json!({
            "title": embed_config.title,
            "description": description,
            "color": embed_config.color,
            "fields": fields,
            "footer": {
                "text": format!("Generated at {}", report.generated_at)
            }
        })
    }
}

#[async_trait]
impl Notifier for DiscordWebhook {
    async fn send_report(&self, report: &Report) -> Result<()> {
        info!("Sending report via Discord webhook");

        if self.config.webhook_url.is_empty() {
            return Err(anyhow::anyhow!("Discord webhook URL is not configured"));
        }

        let embed = self.build_embed(report);
        let payload = json!({
            "embeds": [embed]
        });

        let response = self
            .client
            .post(&self.config.webhook_url)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .context("Failed to send Discord webhook")?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow::anyhow!(
                "Discord webhook returned {}: {}",
                status,
                body
            ));
        }

        info!("Successfully sent report to Discord webhook");
        Ok(())
    }
}
