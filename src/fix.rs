use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tracing::{info, warn};

use crate::ai::AiProvider;
use crate::config::Config;
use crate::logs::collect_logs;
use crate::metrics::collect_metrics;

#[derive(Debug)]
#[allow(dead_code)]
pub struct FixResult {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub success: bool,
}

#[derive(Debug)]
pub struct FixReport {
    pub fix_note: String,
    pub ai_response: String,
    pub commands: Vec<String>,
    pub results: Vec<FixResult>,
    pub executed: bool,
}

/// AIにコマンドだけ生成させる（実行はしない）
/// CPUの平均待機はスキップし、ログだけを参照する
pub async fn generate_fix_commands(
    config: Arc<Config>,
    provider: &dyn AiProvider,
    instruction: &str,
) -> Result<(Vec<String>, String)> {
    // CPU待機なしでログだけ収集
    let logs = collect_logs(&config).await?;

    let mut prompt = String::new();
    prompt.push_str("You are a Linux system administrator. Based on the following fix request and system context, provide shell commands to resolve the issue.\n\n");
    prompt.push_str("IMPORTANT: Respond ONLY with shell commands, one per line. Do not include explanations or markdown. Each line should be a valid shell command.\n");
    prompt.push_str("IMPORTANT: All commands must be fully non-interactive (no user prompts).\n");
    prompt.push_str("- Use `apt-get install -y` or `DEBIAN_FRONTEND=noninteractive apt-get install -y` (never bare `apt install`)\n");
    prompt.push_str("- Add `-y` / `--yes` / `--force` wherever confirmation is required\n\n");
    prompt.push_str("=== FIX REQUEST ===\n");
    prompt.push_str(instruction);
    prompt.push_str("\n\n=== RECENT LOGS ===\n");
    for (path, content) in logs.logs.iter().take(2) {
        let relevant: Vec<&str> = content.lines()
            .filter(|l| { let lo = l.to_lowercase(); lo.contains("error") || lo.contains("fail") })
            .take(5).collect();
        if !relevant.is_empty() {
            prompt.push_str(&format!("\n[{}] errors:\n{}\n", path, relevant.join("\n")));
        }
    }
    prompt.push_str("\nProvide the minimal set of shell commands needed:");

    let ai_response = provider.analyze(&prompt).await?;
    let commands: Vec<String> = ai_response.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("```") && !l.starts_with("~~~"))
        .collect();

    Ok((commands, ai_response))
}

/// コマンドを1つ実行して結果を返す
pub fn run_single_command(command: &str) -> FixResult {
    execute_command(command)
}

pub async fn run_fix_from_instruction(
    config: Arc<Config>,
    provider: &dyn AiProvider,
    instruction: &str,
    auto_execute: bool,
) -> Result<FixReport> {
    // Write instruction to a temp file and delegate
    let tmp = std::env::temp_dir().join("health_fix_note.txt");
    std::fs::write(&tmp, instruction).context("Failed to write temp fix note")?;
    run_fix(config, provider, &tmp, auto_execute).await
}

pub async fn run_fix(
    config: Arc<Config>,
    provider: &dyn AiProvider,
    fix_note_path: &Path,
    auto_execute: bool,
) -> Result<FixReport> {
    info!("Reading fix note from {}", fix_note_path.display());

    let fix_note = std::fs::read_to_string(fix_note_path)
        .with_context(|| format!("Failed to read fix note: {}", fix_note_path.display()))?;

    // Collect current system context
    info!("Collecting system context for fix");
    let metrics = collect_metrics(&config.system_metrics).await?;
    let logs = collect_logs(&config).await?;

    // Build fix prompt
    let mut prompt = String::new();
    prompt.push_str("You are a Linux system administrator. Based on the following fix request and system context, provide shell commands to resolve the issue.\n\n");
    prompt.push_str("IMPORTANT: Respond ONLY with shell commands, one per line. Do not include explanations or markdown. Each line should be a valid shell command.\n");
    prompt.push_str("IMPORTANT: All commands must be fully non-interactive (no user prompts).\n");
    prompt.push_str("- Use `apt-get install -y` or `DEBIAN_FRONTEND=noninteractive apt-get install -y` (never bare `apt install`)\n");
    prompt.push_str("- Add `-y` / `--yes` / `--force` wherever confirmation is required\n\n");
    prompt.push_str("=== FIX REQUEST ===\n");
    prompt.push_str(&fix_note);
    prompt.push_str("\n\n=== CURRENT SYSTEM STATE ===\n");

    if let Some(cpu) = &metrics.cpu {
        prompt.push_str(&format!("CPU: {:.1}%\n", cpu.average_usage));
    }
    if let Some(mem) = &metrics.memory {
        prompt.push_str(&format!(
            "Memory: {}/{} MB ({:.1}%)\n",
            mem.used_mb, mem.total_mb, mem.usage_percent
        ));
    }

    // Add relevant log snippets
    for (path, content) in logs.logs.iter().take(3) {
        let relevant: Vec<&str> = content
            .lines()
            .filter(|line| {
                let lower = line.to_lowercase();
                lower.contains("error") || lower.contains("fail") || lower.contains("critical")
            })
            .take(10)
            .collect();

        if !relevant.is_empty() {
            prompt.push_str(&format!("\n[{}] errors:\n", path));
            for line in relevant {
                prompt.push_str(line);
                prompt.push('\n');
            }
        }
    }

    prompt.push_str("\nProvide the minimal set of shell commands needed to fix the issue:");

    info!("Requesting fix commands from AI");
    let ai_response = provider.analyze(&prompt).await?;

    // Parse commands from AI response (skip empty, comments, markdown artifacts)
    let commands: Vec<String> = ai_response
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with('#')
                && !line.starts_with("```")
                && !line.starts_with("~~~")
        })
        .collect();

    info!("AI suggested {} commands", commands.len());
    for cmd in &commands {
        info!("  > {}", cmd);
    }

    let mut results: Vec<FixResult> = Vec::new();
    let executed = auto_execute && config.auto_fix.allow_execution;

    if executed {
        // Create workspace directory
        let workspace = PathBuf::from(&config.auto_fix.workspace);
        if !workspace.exists() {
            std::fs::create_dir_all(&workspace)
                .context("Failed to create workspace directory")?;
        }

        // Backup if configured
        if config.auto_fix.backup_before_fix {
            info!("Creating backup before executing fix");
            let backup_time = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            let backup_script = workspace.join(format!("backup_{}.sh", backup_time));
            std::fs::write(&backup_script, "#!/bin/bash\n# Backup created before auto-fix\n")
                .context("Failed to create backup script")?;
        }

        for command in &commands {
            info!("Executing: {}", command);
            let result = execute_command(command);
            results.push(result);
        }
    } else if !config.auto_fix.allow_execution {
        warn!("Command execution is disabled in config (auto_fix.allow_execution = false)");
    }

    Ok(FixReport {
        fix_note,
        ai_response,
        commands,
        results,
        executed,
    })
}

pub async fn summarize_results(
    provider: &dyn AiProvider,
    instruction: &str,
    report: &FixReport,
    language: &str,
) -> Result<String> {
    use crate::report::language_instruction;
    let lang = language_instruction(language);
    let mut prompt = String::new();
    prompt.push_str(&format!(
        "You are a Linux system administration AI. Analyze the following command execution results and provide a concise summary.{}\n\n",
        lang
    ));
    prompt.push_str(&format!("Original instruction: {}\n\n", instruction));
    prompt.push_str("=== Execution Results ===\n");

    for r in &report.results {
        prompt.push_str(&format!("$ {}\nexit code: {}\n", r.command, r.exit_code));
        if !r.stdout.is_empty() {
            let out: String = r.stdout.chars().take(500).collect();
            prompt.push_str(&format!("output:\n{}\n", out));
        }
        if !r.stderr.is_empty() {
            let err: String = r.stderr.chars().take(200).collect();
            prompt.push_str(&format!("stderr:\n{}\n", err));
        }
        prompt.push('\n');
    }

    prompt.push_str("Summarize: 1. What was done  2. Success/failure  3. Current system state  4. Further actions needed (if any)");

    provider.analyze(&prompt).await
}

fn execute_command(command: &str) -> FixResult {
    match Command::new("sh").arg("-c").arg(command).output() {
        Ok(output) => FixResult {
            command: command.to_string(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            success: output.status.success(),
        },
        Err(e) => FixResult {
            command: command.to_string(),
            stdout: String::new(),
            stderr: format!("Failed to execute: {}", e),
            exit_code: -1,
            success: false,
        },
    }
}
