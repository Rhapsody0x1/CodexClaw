# Getting Started

Installation, configuration, and first-run guide for CodexClaw.

*Read this guide in: [English](getting-started_en.md) | [中文](getting-started.md)*

## Prerequisites

- Rust toolchain (edition 2024)
- OpenAI Codex CLI installed and authenticated; confirm that `codex exec --json "reply with exactly: ok"` runs successfully
- The system `~/.codex` directory already exists and contains at least `config.toml` and `auth.json`
- QQ Open Platform account (personal verification required)

## Installation

```bash
mkdir -p ~/.codex-claw
cd ~/.codex-claw
git clone https://github.com/Rhapsody0x1/CodexClaw.git repo
cd repo
cargo build --release
```

If the directory already contains the repository:
```bash
cd ~/.codex-claw/repo
git pull --ff-only
cargo build --release
```

## Configuration

1. Copy the example configuration file:
```bash
cp config/codexclaw.example.toml ~/.codex-claw/codexclaw.toml
```

2. Edit `~/.codex-claw/codexclaw.toml` and fill in your QQ bot credentials:
```toml
[qq]
app_id = "your_app_id"
app_secret = "your_app_secret"
api_base_url = "https://sandbox.api.sgroup.qq.com"  # Change to https://api.sgroup.qq.com for production
```

For a full description of all configuration options, see [Configuration Reference](configuration.md).

## QQ Platform Setup

1. Go to the [QQ Open Platform](https://q.qq.com/), log in, and complete personal verification;

2. On the "Home" page, click "Bot", then click "Create QQ Bot". Record the AppID and AppSecret you receive;
   ![Create Bot](../assets/image-20260410150111815.png)

3. Return to the home page and enter the advanced settings page of the newly created bot;
   ![Advanced Settings](../assets/image-20260410150217404.png)

4. Scroll down the left sidebar, find "Callback Configuration", click "Select All" in the direct message events section, and save.
   ![Callback Configuration](../assets/image-20260410150539552.png)

## First Run

Start in foreground mode first to confirm the program can connect to the QQ Gateway:

```bash
cd ~/.codex-claw/repo
CODEX_CLAW_CONFIG=~/.codex-claw/codexclaw.toml ./target/release/codex-claw
```

**Log troubleshooting tips:**
- If access token retrieval fails -- check that `app_id` and `app_secret` are correct
- If the Gateway connection fails -- check your network connection and `api_base_url`
- If Codex fails to start -- confirm that `codex exec --json` runs successfully

On first launch, the system `~/.codex` directory's `config.toml`, `auth.json`, and `skills/` are automatically copied to `~/.codex-claw/.codex/` (existing files are not overwritten).

Once everything looks good, send a direct message to the bot from the QQ client to test the integration.

## Register as a System Service

### macOS (launchd)

Create `~/Library/LaunchAgents/com.codexclaw.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.codexclaw</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/your_username/.codex-claw/repo/target/release/codex-claw</string>
    </array>
    <key>WorkingDirectory</key>
    <string>/Users/your_username/.codex-claw/repo</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>CODEX_CLAW_CONFIG</key>
        <string>/Users/your_username/.codex-claw/codexclaw.toml</string>
    </dict>
    <key>KeepAlive</key>
    <true/>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/Users/your_username/.codex-claw/stdout.log</string>
    <key>StandardErrorPath</key>
    <string>/Users/your_username/.codex-claw/stderr.log</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/com.codexclaw.plist
launchctl start com.codexclaw
```

### Linux (systemd --user)

Create `~/.config/systemd/user/codexclaw.service`:

```ini
[Unit]
Description=CodexClaw QQ Bot
After=network-online.target

[Service]
Type=simple
WorkingDirectory=%h/.codex-claw/repo
Environment=CODEX_CLAW_CONFIG=%h/.codex-claw/codexclaw.toml
ExecStart=%h/.codex-claw/repo/target/release/codex-claw
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

```bash
systemctl --user daemon-reload
systemctl --user enable --now codexclaw
```

> **Tip:** Since the `/self-update` command replaces the running binary and exits the current process, it is recommended to let an external service manager handle automatic restarts.

## Sandbox Mode

CodexClaw starts Codex in workspace-write sandbox mode with network access enabled by default, which provides a degree of protection for your device. However, the sandbox may prevent some of Codex's capabilities from functioning properly:

- **macOS**: Due to the Seatbelt mechanism, Playwright cannot be used to operate a browser
- **Linux**: System-level commands such as `apt` cannot be called due to OS security mechanisms

~~If possible, consider running Codex in `danger-full-access` mode on an isolated VM/VPS to better leverage Codex's full capabilities.~~ The project has since migrated to Codex App Server, which can now request elevated permissions from the user when needed.

## Updating

**Manual update:**
```bash
cd ~/.codex-claw/repo
git pull --ff-only
cargo build --release
# Restart the service
```

**Update via QQ:** Send `/self-update` and CodexClaw will automatically pull the latest code, compile, and replace the running binary.
