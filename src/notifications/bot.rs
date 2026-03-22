use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use serenity::all::{
    ButtonStyle, ChannelId, Command, CommandDataOptionValue, CommandInteraction, CommandOptionType,
    ComponentInteraction, CreateActionRow, CreateButton, CreateCommand, CreateCommandOption,
    CreateEmbed, CreateEmbedFooter, CreateMessage, EventHandler, GatewayIntents, Http, Interaction,
    Ready,
};
use serenity::client::{Client, Context as SerenityContext};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info};
use uuid::Uuid;

use crate::ai;
use crate::config::{Config, DiscordBotNotifications};
use crate::fix;
use crate::notifications::Notifier;
use crate::report::Report;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingFix {
    pub id: String,
    pub instruction: String,
    pub report_summary: String,
}

pub type PendingFixes = Arc<Mutex<HashMap<String, PendingFix>>>;

pub struct BotState {
    pub config: Arc<Config>,
    pub pending_fixes: PendingFixes,
    pub channel_id: u64,
    pub http_client: reqwest::Client,
}

struct Handler {
    state: Arc<BotState>,
}

// Send interaction response directly via reqwest (bypasses serenity's HTTP client)
async fn respond_to_interaction(
    client: &reqwest::Client,
    interaction_id: u64,
    interaction_token: &str,
    body: Value,
) -> Result<()> {
    let url = format!(
        "https://discord.com/api/v10/interactions/{}/{}/callback",
        interaction_id, interaction_token
    );
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .context("Failed to POST interaction callback")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Interaction callback failed {}: {}", status, text);
    }
    Ok(())
}

// Edit the deferred message (progress updates)
async fn update_deferred_message(
    client: &reqwest::Client,
    application_id: u64,
    interaction_token: &str,
    content: &str,
) -> Result<()> {
    let url = format!(
        "https://discord.com/api/v10/webhooks/{}/{}/messages/@original",
        application_id, interaction_token
    );
    let resp = client
        .patch(&url)
        .header("Content-Type", "application/json")
        .json(&json!({ "content": content, "components": [] }))
        .send()
        .await
        .context("Failed to PATCH deferred message")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Update deferred failed {}: {}", status, text);
    }
    Ok(())
}

// Send a new followup message as file attachment
async fn send_file_followup(
    client: &reqwest::Client,
    application_id: u64,
    interaction_token: &str,
    summary: &str,
    filename: &str,
    file_content: &str,
) -> Result<()> {
    let url = format!(
        "https://discord.com/api/v10/webhooks/{}/{}",
        application_id, interaction_token
    );
    let payload = serde_json::to_string(&json!({
        "content": truncate_utf8(summary, 1900)
    }))?;
    let form = reqwest::multipart::Form::new()
        .text("payload_json", payload)
        .part(
            "files[0]",
            reqwest::multipart::Part::bytes(file_content.as_bytes().to_vec())
                .file_name(filename.to_string())
                .mime_str("text/plain")?,
        );
    let resp = client.post(&url).multipart(form).send().await
        .context("Failed to send file followup")?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("File followup failed {}: {}", status, text);
    }
    Ok(())
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: SerenityContext, ready: Ready) {
        info!("Discord bot connected as {}", ready.user.name);

        let command = CreateCommand::new("health-fix")
            .description("Request an AI-powered system fix")
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "instruction",
                    "Describe what needs to be fixed",
                )
                .required(true),
            );

        match Command::create_global_command(&ctx.http, command).await {
            Ok(_) => info!("Registered /health-fix slash command"),
            Err(e) => error!("Failed to register slash command: {}", e),
        }
    }

    async fn interaction_create(&self, ctx: SerenityContext, interaction: Interaction) {
        info!("Received interaction: {:?}", interaction.kind());
        match interaction {
            Interaction::Command(command_interaction) => {
                info!("Slash command: {}", command_interaction.data.name);
                self.handle_slash_command(&ctx, command_interaction).await;
            }
            Interaction::Component(component_interaction) => {
                info!(
                    "Component: {}",
                    component_interaction.data.custom_id
                );
                self.handle_component(&ctx, component_interaction).await;
            }
            _ => {}
        }
    }
}

impl Handler {
    async fn handle_slash_command(&self, ctx: &SerenityContext, command: CommandInteraction) {
        if command.data.name != "health-fix" {
            return;
        }

        let instruction = command
            .data
            .options
            .first()
            .and_then(|opt| {
                if let CommandDataOptionValue::String(s) = &opt.value {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("No instruction provided")
            .to_string();

        let fix_id = Uuid::new_v4().to_string();
        let pending_fix = PendingFix {
            id: fix_id.clone(),
            instruction: instruction.clone(),
            report_summary: "Manual fix request via slash command".to_string(),
        };

        {
            let mut fixes = self.state.pending_fixes.lock().await;
            fixes.insert(fix_id.clone(), pending_fix);
        }

        let body = json!({
            "type": 4,
            "data": {
                "content": format!("Fix request received: **{}**\n\nReview and confirm execution?", instruction),
                "components": [{
                    "type": 1,
                    "components": [
                        {
                            "type": 2,
                            "label": "Confirm Fix",
                            "style": 4,
                            "custom_id": format!("fix_confirm_{}", fix_id)
                        },
                        {
                            "type": 2,
                            "label": "Cancel",
                            "style": 2,
                            "custom_id": format!("fix_cancel_{}", fix_id)
                        }
                    ]
                }]
            }
        });

        let interaction_id = command.id.get();
        let token = command.token.clone();
        if let Err(e) = respond_to_interaction(
            &self.state.http_client,
            interaction_id,
            &token,
            body,
        )
        .await
        {
            error!("Failed to respond to slash command: {}", e);
            // Fallback: send to channel directly
            let _ = ChannelId::new(self.state.channel_id)
                .send_message(
                    &ctx.http,
                    CreateMessage::new().content(format!(
                        "Fix request: **{}** — use buttons above or retry `/health-fix`",
                        instruction
                    )),
                )
                .await;
        } else {
            info!("Slash command response sent successfully");
        }
    }

    async fn handle_component(&self, ctx: &SerenityContext, component: ComponentInteraction) {
        let custom_id = component.data.custom_id.clone();
        let application_id = component.application_id.get();
        let token = component.token.clone();
        let interaction_id = component.id.get();

        if custom_id.starts_with("fix_confirm_") {
            let fix_id = custom_id.trim_start_matches("fix_confirm_").to_string();

            // Defer first
            let defer_body = json!({ "type": 6 });
            if let Err(e) = respond_to_interaction(
                &self.state.http_client,
                interaction_id,
                &token,
                defer_body,
            )
            .await
            {
                error!("Failed to defer: {}", e);
                return;
            }

            let fix = {
                let mut fixes = self.state.pending_fixes.lock().await;
                fixes.remove(&fix_id)
            };

            let Some(pending_fix) = fix else {
                let _ = update_deferred_message(&self.state.http_client, application_id, &token, "Fix request not found or already processed.").await;
                return;
            };

            let allow_execution = self.state.config.auto_fix.allow_execution;
            let provider = match ai::create_provider(&self.state.config) {
                Ok(p) => p,
                Err(e) => {
                    let _ = update_deferred_message(&self.state.http_client, application_id, &token, &format!("❌ AI provider error: {}", e)).await;
                    return;
                }
            };

            // Step 1: AI分析中
            let _ = update_deferred_message(&self.state.http_client, application_id, &token,
                "```\n🔄 AIがコマンドを分析中...\n```").await;

            let (commands, _) = match fix::generate_fix_commands(
                self.state.config.clone(), provider.as_ref(), &pending_fix.instruction
            ).await {
                Ok(r) => r,
                Err(e) => {
                    let _ = update_deferred_message(&self.state.http_client, application_id, &token, &format!("❌ AI分析失敗: {}", e)).await;
                    return;
                }
            };

            if commands.is_empty() {
                let _ = update_deferred_message(&self.state.http_client, application_id, &token, "⚠️ AIはコマンドを生成しませんでした。").await;
                return;
            }

            // Step 2: コマンド一覧を表示
            let cmd_list = commands.iter().enumerate()
                .map(|(i, c)| format!("  {}. {}", i + 1, c))
                .collect::<Vec<_>>().join("\n");
            let _ = update_deferred_message(&self.state.http_client, application_id, &token,
                &format!("```\n✅ コマンド生成完了 ({} 件)\n{}\n\n▶️  実行開始...\n```", commands.len(), cmd_list)).await;

            if !allow_execution {
                let summary = format!("**Fix提案: {}**\n\n実行するには `allow_execution: true` に設定してください。", pending_fix.instruction);
                let file_content = format!("# Fix提案\n## 指示\n{}\n\n## 生成されたコマンド\n```sh\n{}\n```\n", pending_fix.instruction, commands.join("\n"));
                let _ = send_file_followup(&self.state.http_client, application_id, &token, &summary, "fix_proposal.md", &file_content).await;
                let _ = update_deferred_message(&self.state.http_client, application_id, &token, &truncate_utf8(&summary, 500)).await;
                return;
            }

            // Step 3: コマンドをリアルタイムで実行
            let mut file_log = format!("# Fix実行ログ\n## 指示\n{}\n\n## 実行結果\n\n", pending_fix.instruction);
            let mut discord_lines: Vec<String> = Vec::new(); // Discordに表示する直近の出力
            let mut results = Vec::new();

            for (i, cmd) in commands.iter().enumerate() {
                // 実行前: コマンドをDiscordに表示
                discord_lines.push(format!("$ {}", cmd));
                let preview = build_discord_log_preview(&discord_lines, i + 1, commands.len(), false);
                let _ = update_deferred_message(&self.state.http_client, application_id, &token, &preview).await;

                let result = fix::run_single_command(cmd);

                // 出力をDiscordログに追記
                let output = if !result.stdout.is_empty() {
                    result.stdout.trim().to_string()
                } else if !result.stderr.is_empty() {
                    format!("(stderr) {}", result.stderr.trim())
                } else {
                    format!("(exit: {})", result.exit_code)
                };
                // 出力が長い場合は末尾を表示
                let short_output: String = output.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                discord_lines.push(short_output.clone());
                discord_lines.push(String::new()); // 空行

                // ファイルには全出力を記録
                file_log.push_str(&format!("$ {}\n", cmd));
                if !result.stdout.is_empty() { file_log.push_str(&result.stdout); }
                if !result.stderr.is_empty() { file_log.push_str(&format!("(stderr)\n{}", result.stderr)); }
                file_log.push_str(&format!("(exit: {})\n\n", result.exit_code));

                // 実行後: 結果込みでDiscordを更新
                let preview = build_discord_log_preview(&discord_lines, i + 1, commands.len(), i + 1 == commands.len());
                let _ = update_deferred_message(&self.state.http_client, application_id, &token, &preview).await;

                results.push(result);
            }

            // Step 4: AIがまとめ
            let _ = update_deferred_message(&self.state.http_client, application_id, &token,
                "```\n✅ 全コマンド完了\n🔄 AIがまとめを生成中...\n```").await;

            let tmp_report = fix::FixReport {
                fix_note: pending_fix.instruction.clone(),
                ai_response: String::new(),
                commands: commands.clone(),
                results,
                executed: true,
            };
            let summary = fix::summarize_results(provider.as_ref(), &pending_fix.instruction, &tmp_report, &self.state.config.ai_assistant.language)
                .await.unwrap_or_else(|e| format!("Summary failed: {}", e));

            file_log.push_str(&format!("\n## AIまとめ\n{}\n", summary));

            // Step 5: ファイルのみ送信（本文はシンプルに）
            if let Err(e) = send_file_followup(
                &self.state.http_client, application_id, &token,
                "✅ Fix完了 — 詳細は添付ファイルを参照", "fix_log.md", &file_log
            ).await {
                error!("Failed to send file: {}", e);
                let _ = update_deferred_message(&self.state.http_client, application_id, &token,
                    &truncate_utf8(&format!("✅ Fix完了\n\n{}", summary), 1900)).await;
            } else {
                let _ = update_deferred_message(&self.state.http_client, application_id, &token,
                    "```\n✅ 全コマンド完了\n```").await;
                info!("Fix complete, file sent");
            }
        } else if custom_id.starts_with("fix_cancel_") {
            let fix_id = custom_id.trim_start_matches("fix_cancel_").to_string();
            {
                let mut fixes = self.state.pending_fixes.lock().await;
                fixes.remove(&fix_id);
            }
            let body = json!({
                "type": 4,
                "data": { "content": "Fix cancelled.", "components": [] }
            });
            if let Err(e) =
                respond_to_interaction(&self.state.http_client, interaction_id, &token, body).await
            {
                error!("Failed to respond to cancel: {}", e);
            }
        } else if custom_id == "autofix_button" {
            let body = json!({
                "type": 4,
                "data": {
                    "content": "Use `/health-fix` with a specific instruction.",
                    "flags": 64
                }
            });
            if let Err(e) =
                respond_to_interaction(&self.state.http_client, interaction_id, &token, body).await
            {
                error!("Failed to respond to autofix: {}", e);
            }
        }
    }
}

pub struct DiscordBot {
    config: Arc<Config>,
    bot_config: DiscordBotNotifications,
    pending_fixes: PendingFixes,
}

impl DiscordBot {
    pub fn new(config: Arc<Config>, bot_config: DiscordBotNotifications) -> Self {
        DiscordBot {
            config,
            bot_config,
            pending_fixes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn start(&self) -> Result<()> {
        let channel_id: u64 = self
            .bot_config
            .channel_id
            .parse()
            .context("Invalid channel ID")?;

        let state = Arc::new(BotState {
            config: self.config.clone(),
            pending_fixes: self.pending_fixes.clone(),
            channel_id,
            http_client: reqwest::Client::new(),
        });

        let handler = Handler { state };
        let intents = GatewayIntents::GUILDS;

        let mut client = Client::builder(&self.bot_config.discord_bot_token, intents)
            .event_handler(handler)
            .await
            .context("Failed to create Discord bot client")?;

        client.start().await.context("Discord bot error")?;
        Ok(())
    }

    pub async fn send_to_channel(&self, report: &Report) -> Result<()> {
        let channel_id: u64 = self
            .bot_config
            .channel_id
            .parse()
            .context("Invalid channel ID")?;

        let http = Http::new(&self.bot_config.discord_bot_token);
        let channel = ChannelId::new(channel_id);

        let embed = build_report_embed(report, &self.config);
        let components = vec![CreateActionRow::Buttons(vec![
            CreateButton::new("autofix_button")
                .label("Auto Fix")
                .style(ButtonStyle::Primary),
        ])];

        channel
            .send_message(&http, CreateMessage::new().embed(embed).components(components))
            .await
            .context("Failed to send message to Discord channel")?;

        info!("Successfully sent report to Discord channel");
        Ok(())
    }
}

// Discordのコードブロックに収まるよう直近のログ行を組み立てる
fn build_discord_log_preview(lines: &[String], current: usize, total: usize, done: bool) -> String {
    let status = if done {
        format!("✅ [{}/{}] 完了", current, total)
    } else {
        format!("▶️  [{}/{}] 実行中...", current, total)
    };
    // 直近の行のみ取り出してコードブロックに収める（1800文字以内）
    let mut block = String::new();
    for line in lines.iter().rev() {
        let candidate = format!("{}\n{}", line, block);
        if candidate.len() > 1700 { break; }
        block = candidate;
    }
    format!("```\n{}\n\n{}\n```", status, block.trim_start())
}

fn truncate_utf8(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() { truncated + "..." } else { truncated }
}

fn build_report_embed(report: &Report, config: &Config) -> CreateEmbed {
    let embed_config = &config.notifications.embed;
    let mut embed = CreateEmbed::new()
        .title(&embed_config.title)
        .color(embed_config.color);

    embed = embed.description(truncate_utf8(&report.ai_analysis, 2000));

    if let Some(cpu) = &report.metrics.cpu {
        embed = embed.field(
            "CPU Usage",
            format!("{:.1}% (avg over {}s)", cpu.average_usage, cpu.interval_seconds),
            true,
        );
    }

    if let Some(mem) = &report.metrics.memory {
        embed = embed.field(
            "Memory",
            format!(
                "{}/{} MB ({:.1}%)\nSwap: {}/{} MB",
                mem.used_mb, mem.total_mb, mem.usage_percent, mem.swap_used_mb, mem.swap_total_mb
            ),
            true,
        );
    }

    if let Some(load) = &report.metrics.load_average {
        embed = embed.field(
            "Load Average",
            format!("{:.2} / {:.2} / {:.2}", load.one, load.five, load.fifteen),
            true,
        );
    }

    for disk in &report.metrics.disks {
        embed = embed.field(
            format!("Disk {}", disk.mount_point),
            format!("{:.1}/{:.1} GB ({:.1}%)", disk.used_gb, disk.total_gb, disk.usage_percent),
            true,
        );
    }

    embed = embed.footer(CreateEmbedFooter::new(format!("Generated at {}", report.generated_at)));
    embed
}

#[async_trait]
impl Notifier for DiscordBot {
    async fn send_report(&self, report: &Report) -> Result<()> {
        info!("Sending report via Discord bot");
        self.send_to_channel(report).await
    }
}
