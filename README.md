# system-health-report

AI-powered Linux system health monitoring tool that collects metrics and logs, analyzes them with an LLM, and reports to Discord — with automated fix suggestions you can execute directly from Discord slash commands.

## Features

- **System metrics collection** — CPU usage, memory, load average, disk usage via `sysinfo`
- **Log collection** — Reads standard system logs (`/var/log/syslog`, `kern.log`, `auth.log`, etc.) and application logs (Apache, Nginx, MySQL, mail)
- **AI analysis** — Sends collected data to OpenRouter, OpenAI, or Anthropic for intelligent health assessment
- **Discord notifications** — Posts formatted embed reports via webhook or Discord Bot
- **Slash command: `/health-fix`** — Describe an issue; AI generates shell commands to fix it. Confirm or cancel from Discord buttons with real-time progress updates and a summary file attachment
- **Scheduled reports** — Runs automatically at configured times (e.g. 09:00 and 21:00)
- **Multi-language** — AI responses in any language (configured in `config.yml`)
- **Debian package** — Ships as a `.deb` with systemd service for easy deployment

## Requirements

- Debian/Ubuntu Linux
- An AI provider API key (OpenRouter, OpenAI, or Anthropic)
- A Discord Bot token and channel ID (for bot mode), or a Discord webhook URL (for webhook mode)

## Installation

### From .deb package

```bash
sudo dpkg -i system-health-report_*.deb
```

Edit the configuration file:

```bash
sudo nano /etc/system-health-report/config.yml
```

Enable and start the service:

```bash
sudo systemctl enable --now system-health-report
```

### From source

```bash
git clone https://github.com/hage-momizi/system-health-report.git
cd system-health-report
cargo build --release
```

## Configuration

All configuration lives in `config.yml` (installed to `/etc/system-health-report/config.yml`):

```yaml
ai_assistant:
  active_provider: "openrouter"   # openrouter | openai | anthropic
  providers:
    openrouter:
      api_key: "sk-or-..."
      model: "google/gemini-2.0-flash-001"
    openai:
      api_key: "sk-..."
      model: "gpt-4o"
    anthropic:
      api_key: "sk-ant-..."
      model: "claude-3-5-sonnet-latest"
  schedule:
    enabled: true
    times: ["09:00", "21:00"]
    timezone: "UTC"
  language: "japanese"            # Language for AI responses

notifications:
  provider: "discord"
  webhook_url: "https://discord.com/api/webhooks/..."

"notifications(Discord BOT)":
  discord_bot_token: "Bot ..."
  channel_id: "123456789"
  guild_id: ""                    # Set for instant slash command registration

system_metrics:
  cpu:
    enabled: true
    collection_mode: "average"
    interval_seconds: 60
  memory:
    enabled: true
  load_average:
    enabled: true
  disk_status:
    enabled: true
    mount_points:
      - "/"
      - "/var"

standard_logs:
  - "/var/log/syslog"
  - "/var/log/kern.log"
  - "/var/log/auth.log"

application_logs:
  apache2:
    enabled: true
    path: "/var/log/apache2/error.log"
  nginx:
    enabled: false
    path: "/var/log/nginx/error.log"
  mysql:
    enabled: true
    path: "/var/log/mysql/error.log"

auto_fix:
  enabled: true
  workspace: "./fixes/"
  allow_execution: false          # Set true to allow bot to execute commands
  backup_before_fix: true
```

## Discord Bot Setup

1. Create a bot at [Discord Developer Portal](https://discord.com/developers/applications)
2. Enable **Server Members Intent** and **Message Content Intent** under Bot settings
3. Invite the bot with `applications.commands` and `bot` scopes (permissions: Send Messages, Attach Files, Use Slash Commands)
4. Set `discord_bot_token` and `channel_id` in `config.yml`
5. Optionally set `guild_id` for instant slash command registration (otherwise commands propagate globally in ~1 hour)

### Available Slash Commands

| Command | Description |
|---------|-------------|
| `/health-fix <instruction>` | Ask the AI to generate fix commands for a described issue |

The `/health-fix` command shows the instruction and two buttons:
- **Confirm Fix** — Execute the commands one by one with real-time Discord updates, then send a summary file as attachment
- **Cancel** — Dismiss without executing

When the scheduled health report is posted, it includes an **Auto Fix** button that opens a prompt to run `/health-fix` with a specific instruction.

## Building the .deb Package

```bash
cargo install cargo-deb
cargo deb
```

The package is output to `target/debian/system-health-report_*.deb`.

## Usage (CLI)

```bash
# Run a health check immediately
health --run-now

# Run with a custom config path
health --config /path/to/config.yml --run-now

# Generate fix commands from a fix-note file (dry-run, no execution)
echo "nginx is returning 502 errors" > /tmp/fix.txt
health --fix-note /tmp/fix.txt

# Generate and automatically execute fix commands
health --fix-note /tmp/fix.txt --mode auto

# Stream bash command output to Discord (watch mode)
health --watch

# Start the scheduler + Discord bot daemon
health
```

## Architecture

```
health (binary)
├── src/main.rs          CLI entry point, scheduler, bot launcher
├── src/config.rs        Config loading from YAML
├── src/metrics.rs       CPU, memory, disk, load average collection
├── src/logs.rs          Log file reading
├── src/report.rs        AI prompt building and report generation
├── src/fix.rs           Fix command generation and execution
├── src/ai/              AI provider clients (OpenRouter, OpenAI, Anthropic)
└── src/notifications/
    ├── webhook.rs       Discord webhook sender
    └── bot.rs           Discord bot (serenity + reqwest for interactions)
```

## License

MIT
