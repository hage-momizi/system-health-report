use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

use crate::ai::AiProvider;
use crate::config::Config;
use crate::logs::LogCollection;
use crate::metrics::SystemMetrics;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub ai_analysis: String,
    pub metrics: SystemMetrics,
    pub logs: LogCollection,
    pub generated_at: String,
}

pub fn build_prompt(metrics: &SystemMetrics, logs: &LogCollection, language: &str) -> String {
    let mut prompt = String::new();

    let lang_instruction = language_instruction(language);
    prompt.push_str(&format!(
        "You are a Linux server monitoring AI. Analyze the following metrics and logs.{}\n\n",
        lang_instruction
    ));
    prompt.push_str("Report the following:\n");
    prompt.push_str("1. Overall system health status (OK / Warning / Critical)\n");
    prompt.push_str("2. Any anomalies or issues detected\n");
    prompt.push_str("3. Recommended actions if needed\n\n");

    prompt.push_str("=== SYSTEM METRICS ===\n");
    prompt.push_str(&format!("Collected at: {}\n\n", metrics.collected_at));

    if let Some(cpu) = &metrics.cpu {
        prompt.push_str(&format!(
            "CPU Usage (avg over {}s): {:.1}%\n",
            cpu.interval_seconds, cpu.average_usage
        ));
    }

    if let Some(mem) = &metrics.memory {
        prompt.push_str(&format!(
            "Memory: {}/{} MB ({:.1}%)\n",
            mem.used_mb, mem.total_mb, mem.usage_percent
        ));
        prompt.push_str(&format!("Memory Available: {} MB\n", mem.available_mb));
        prompt.push_str(&format!(
            "Swap: {}/{} MB\n",
            mem.swap_used_mb, mem.swap_total_mb
        ));
    }

    if let Some(load) = &metrics.load_average {
        prompt.push_str(&format!(
            "Load Average: {:.2} (1min), {:.2} (5min), {:.2} (15min)\n",
            load.one, load.five, load.fifteen
        ));
    }

    if !metrics.disks.is_empty() {
        prompt.push_str("\nDisk Usage:\n");
        for disk in &metrics.disks {
            prompt.push_str(&format!(
                "  {}: {:.1}/{:.1} GB used ({:.1}%), {:.1} GB available\n",
                disk.mount_point,
                disk.used_gb,
                disk.total_gb,
                disk.usage_percent,
                disk.available_gb
            ));
        }
    }

    prompt.push_str("\n=== LOGS ===\n");

    let mut sorted_logs: Vec<(&String, &String)> = logs.logs.iter().collect();
    sorted_logs.sort_by_key(|(k, _)| k.as_str());

    for (path, content) in sorted_logs {
        prompt.push_str(&format!("\n[{}] (last 100 lines):\n", path));
        prompt.push_str("---\n");

        // Truncate very long log sections to keep prompt manageable
        if content.chars().count() > 5000 {
            let tail: String = content.chars().rev().take(5000).collect::<String>().chars().rev().collect();
            if let Some(nl_pos) = tail.find('\n') {
                prompt.push_str(&tail[nl_pos + 1..]);
                prompt.push_str("\n[... truncated ...]\n");
            } else {
                prompt.push_str(&tail);
            }
        } else {
            prompt.push_str(content);
        }
        prompt.push_str("\n---\n");
    }

    prompt
}

pub fn language_instruction(language: &str) -> String {
    match language.to_lowercase().as_str() {
        "english" | "en" => String::new(),
        lang => format!(" Respond entirely in {}.", lang),
    }
}

pub async fn generate_report(
    config: Arc<Config>,
    provider: &dyn AiProvider,
    metrics: SystemMetrics,
    logs: LogCollection,
) -> Result<Report> {
    info!("Building AI prompt from metrics and logs");
    let prompt = build_prompt(&metrics, &logs, &config.ai_assistant.language);

    info!("Sending prompt to AI provider for analysis");
    let ai_analysis = provider.analyze(&prompt).await?;

    let report = Report {
        ai_analysis,
        metrics,
        logs,
        generated_at: chrono::Utc::now().to_rfc3339(),
    };

    Ok(report)
}
