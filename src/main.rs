use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

mod ai;
mod config;
mod fix;
mod logs;
mod metrics;
mod notifications;
mod report;
mod scheduler;
mod watch;

use config::Config;
use notifications::webhook::DiscordWebhook;
use notifications::Notifier;

#[derive(Parser)]
#[command(name = "health", about = "System health monitor with AI analysis")]
struct Cli {
    /// Run report once immediately
    #[arg(long)]
    run_now: bool,

    /// Fix note file to process
    #[arg(long)]
    fix_note: Option<PathBuf>,

    /// Execution mode for fix-note (e.g., "auto")
    #[arg(long)]
    mode: Option<String>,

    /// Watch mode: stream bash commands to Discord
    #[arg(long)]
    watch: bool,

    /// Config file path
    #[arg(long, default_value = "/etc/system-health-report/config.yml")]
    config: PathBuf,
}

async fn run_report(config: Arc<Config>) -> Result<()> {
    info!("Starting health report");

    // Create AI provider
    let provider = ai::create_provider(&config)?;

    // Collect metrics and logs concurrently
    let metrics_config = config.system_metrics.clone();
    let metrics_future = metrics::collect_metrics(&metrics_config);
    let logs_future = logs::collect_logs(&config);

    let (metrics, logs) = tokio::join!(metrics_future, logs_future);
    let metrics = metrics.context("Failed to collect system metrics")?;
    let logs = logs.context("Failed to collect logs")?;

    // Generate AI report
    let report = report::generate_report(config.clone(), provider.as_ref(), metrics, logs).await?;

    info!("AI analysis complete. Sending notifications.");

    // Send notification based on provider
    match config.notifications.provider.as_str() {
        "discord" | "webhook" => {
            let webhook = DiscordWebhook::new(config.notifications.clone());
            webhook
                .send_report(&report)
                .await
                .context("Failed to send webhook notification")?;
        }
        "bot" | "discord_bot" => {
            if let Some(bot_config) = &config.notifications_discord_bot {
                let bot = notifications::bot::DiscordBot::new(config.clone(), bot_config.clone());
                bot.send_report(&report)
                    .await
                    .context("Failed to send bot notification")?;
            } else {
                error!("Discord bot config missing, falling back to webhook");
                let webhook = DiscordWebhook::new(config.notifications.clone());
                webhook.send_report(&report).await?;
            }
        }
        provider => {
            error!("Unknown notification provider: {}. Using webhook.", provider);
            let webhook = DiscordWebhook::new(config.notifications.clone());
            webhook.send_report(&report).await?;
        }
    }

    info!("Health report complete");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args = Cli::parse();

    // Load config with fallback
    let config_path = if args.config.exists() {
        args.config.clone()
    } else {
        let fallback = PathBuf::from("./config.yml");
        if fallback.exists() {
            info!(
                "Config not found at {}, using ./config.yml",
                args.config.display()
            );
            fallback
        } else {
            args.config.clone()
        }
    };

    info!("Loading config from {}", config_path.display());
    let config = Arc::new(Config::load(&config_path).context("Failed to load configuration")?);

    // Dispatch based on CLI flags
    if args.watch {
        info!("Starting watch mode");
        watch::run_watch_mode(config).await?;
    } else if let Some(fix_note_path) = args.fix_note {
        let auto_execute = args.mode.as_deref() == Some("auto");
        info!(
            "Running fix mode for: {} (auto_execute: {})",
            fix_note_path.display(),
            auto_execute
        );

        let provider = ai::create_provider(&config)?;
        let fix_report =
            fix::run_fix(config, provider.as_ref(), &fix_note_path, auto_execute).await?;

        println!("\n=== FIX REPORT ===");
        println!("Fix Note:\n{}", fix_report.fix_note);
        println!("\nAI Response:\n{}", fix_report.ai_response);
        println!("\nSuggested Commands:");
        for cmd in &fix_report.commands {
            println!("  $ {}", cmd);
        }

        if fix_report.executed {
            println!("\nExecution Results:");
            for result in &fix_report.results {
                println!("\n$ {}", result.command);
                println!("Exit code: {}", result.exit_code);
                if !result.stdout.is_empty() {
                    println!("STDOUT:\n{}", result.stdout);
                }
                if !result.stderr.is_empty() {
                    println!("STDERR:\n{}", result.stderr);
                }
            }
        } else {
            println!("\n[Execution disabled - set allow_execution: true in config to run automatically]");
        }
    } else if args.run_now {
        info!("Running report immediately");
        run_report(config).await?;
    } else {
        // Default: daemon/scheduler mode
        info!("Starting in scheduler daemon mode");

        // Optionally start Discord bot in background
        if let Some(bot_config) = config.notifications_discord_bot.clone() {
            if config.notifications.provider == "bot" || config.notifications.provider == "discord_bot" {
                let config_for_bot = config.clone();
                let bot_config_clone = bot_config.clone();
                tokio::spawn(async move {
                    let bot = notifications::bot::DiscordBot::new(config_for_bot, bot_config_clone);
                    if let Err(e) = bot.start().await {
                        error!("Discord bot error: {}", e);
                    }
                });
            }
        }

        let config_clone = config.clone();
        scheduler::run_scheduler(config, move || {
            let config = config_clone.clone();
            async move { run_report(config).await }
        })
        .await?;
    }

    Ok(())
}
