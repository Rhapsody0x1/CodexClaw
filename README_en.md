<div align="center">

<img src="./assets/banner.svg" width="600" alt="CodexClaw">

A private QQ AI assistant powered by [OpenAI Codex App-Server](https://developers.openai.com/codex/app-server)

*Read this in: [English](#table-of-contents) | [中文](README.md)*

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg)](https://www.rust-lang.org/)

</div>

---

CodexClaw is a private AI assistant built on the Codex App Server and connected to the QQ official bot platform. It allows you to operate Codex on your computer through QQ to complete various tasks.

## Table of Contents

- [Universe-Level Security Disclaimer](#universe-level-security-disclaimer)
- [Why Did You Make Another Claw?](#why-did-you-make-another-claw)
- [Feature Highlights](#feature-highlights)
- [Quick Start](#quick-start)
- [Command Cheat Sheet](#command-cheat-sheet)
- [Scheduled Tasks](#scheduled-tasks)
- [Runtime Files](#runtime-files)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [Deployment Instructions for Codex](#deployment-instructions-for-codex)

## Universe-Level Security Disclaimer

This project was *<u>**entirely built using Vibe Coding**</u>* and is now open-sourced or distributed on an "as-is" basis. The author, contributors, and related third parties make no express or implied warranties regarding the **availability, correctness, security, suitability, ongoing maintenance status**, or compatibility with any particular use of this project.

Users should independently perform code review, environment isolation, permission control, dependency auditing, data backup, and deployment verification, and judge for themselves whether this project complies with the laws, regulations, security requirements, and operational standards of their region, platform rules, organizational policies, and business scenarios.

This project may **call external services, read/write local files, process chat messages**, upload or download attachments, and may cause service interruptions, data leaks, unintended operations, additional charges, account penalties, or other direct, indirect, incidental, special, or punitive losses due to model output, configuration errors, dependency defects, platform API changes, or operational mistakes. Except where required by law, the author and contributors bear no responsibility for such consequences.

When deploying or using this project, you **should properly safeguard all account credentials, access tokens, chat data, and server permissions, and bear all risks and consequences arising therefrom**. If you do not agree to the above conditions, please do not deploy, copy, modify, or use this project.

## Why Did You Make Another Claw?

1. I really dislike OpenClaw -- it feels like a toy, is too heavy, and demands full access to my computer right from the start;
2. Compared to OpenClaw, Codex has a great model and harness, and OpenAI is actively maintaining it;
3. The credits you get with a Codex subscription are truly generous;
4. This project is riding on OpenClaw's popularity : (

Some other ramblings can be found in this [Blog](https://rhapsody0x1.github.io/p/about-codex-claw).

## Feature Highlights

- **Session Management** &mdash; Parallel foreground/background multi-session support, with save, restore, and import of system Codex sessions
- **Scheduled Tasks** &mdash; Built-in cron scheduler supporting reminders, Codex execution, shell scripts, and interactive multi-turn conversations
- **Memory Distillation** &mdash; (Experimental) Automatic background extraction of memories and skills from conversations, injected into subsequent prompts
- **Approval Workflow** &mdash; Requests user approval via QQ before Codex executes sensitive operations, with per-session auto-approval support
- **Self-Update** &mdash; Send `/self-update` via QQ to pull the latest code, compile, and hot-replace

## Quick Start

> For the complete deployment guide, see [docs/getting-started_en.md](docs/getting-started_en.md)

1. Set up Codex login credentials or API key on the device where you want to deploy CodexClaw, and confirm that you can have a normal conversation with Codex in the TUI launched by the `codex` command;

2. Go to the [QQ Open Platform](https://q.qq.com/), and complete login and personal verification as required;

3. On the "Home" page, click "Bot", then click "Create QQ Bot", and record the obtained AppID and AppSecret for later use;
    ![image-20260410150111815](./assets/image-20260410150111815.png)
4. Return to the previous home page and enter the advanced settings page of your newly created bot;
    ![image-20260410150217404](./assets/image-20260410150217404.png)

5. Scroll down the left sidebar, find "Callback Configuration", click "Select All" for private message events, and save;
    ![image-20260410150539552](./assets/image-20260410150539552.png)

6. Fill in the AppID and AppSecret obtained in step 3 into the prompt below and send it to Codex. It will help you complete the rest of the work. Note that since system service installation and similar operations are involved, Codex **may request to execute some privileged commands -- please carefully review the safety of these commands. The author assumes no responsibility for any losses caused thereby**. If you do not want to expose your secrets to Codex, you can also ask Codex to guide you through the manual deployment process.

```plain
Help me deploy CodexClaw on this machine: https://github.com/Rhapsody0x1/CodexClaw . I have obtained the AppId: {} and AppSecret: {} you need. Then register it as a system service and provide me with commands to manage its enabled state.
```

7. By default, CodexClaw starts Codex in workspace-write sandbox mode with network access enabled, which provides some protection for your device. However, due to security mechanisms on various platforms, the sandbox may prevent some of Codex's capabilities from functioning properly. Issues the author has observed so far:

    - On macOS, Playwright cannot be used to control browsers due to the Seatbelt mechanism;
    - On Linux, system-level commands like apt cannot be invoked due to security mechanisms;

    ~~If possible, consider running Codex in danger-full-access mode on an isolated VM/VPS, which can better leverage Codex's capabilities. As for how to modify the configuration, you can ask the ever-capable Codex ; )~~ The project has now migrated to Codex App Server, which can request the user to approve commands that exceed sandbox permissions.

## Command Cheat Sheet

> For the complete command reference, see [docs/commands_en.md](docs/commands_en.md)

If you want to take advantage of the quick commands provided by the QQ official bot platform, you can manually add the following commands to the command list. Of course, not adding them does not affect their normal usage.

| Command | Description |
|---------|-------------|
| `/help` | View the command list |
| `/status` | View current session status |
| `/new [dir]` | Create a new foreground session |
| `/stop` | End the current session |
| `/sessions` | List historical sessions |
| `/model [name]` | Set or view the model |
| `/compact` | Compress session context |
| `/self-update` | Compile and hot-replace the binary |

<details>
<summary>More Commands</summary>

- `/interrupt`: Stop the current run without ending the session
- `/lang [en|zh]`: Switch interface language
- `/fast [on|off]`: Toggle Fast mode
- `/context [1m|standard]`: Set context mode
- `/reasoning [low|medium|high|xhigh]`: Set reasoning depth
- `/verbose [on|off]`: Toggle verbose output
- `/save`: Explicitly save the current foreground session
- `/bg [alias]`: Move the current session to the background
- `/fg <alias>`: Switch back to a background session
- `/resume <id>`: Resume a disk session
- `/import`: Import system Codex sessions
- `/loadbg <id> [alias]`: Load a session to the background
- `/rename <old> <new>`: Rename a background label
- `/alias`: Manage command aliases
- `/approvals`: Toggle approval policy
- `/approve`: Approve a pending request
- `/deny`: Deny a pending request
- `/plan`: Enter plan mode
- `/cron`: Manage scheduled tasks

</details>

## Scheduled Tasks

CodexClaw includes a built-in cron scheduler that supports the following types of tasks:

| Type | Description |
|------|-------------|
| `reminder` | Send a reminder message at a specified time |
| `codex` | Execute a Codex task at a scheduled time |
| `shell` | Run a shell command or script on a schedule |
| `interactive` | Start an interactive multi-turn conversation on a schedule |

> For the complete scheduler reference, see [docs/scheduler_en.md](docs/scheduler_en.md)

## Runtime Files

| Path | Description |
|------|-------------|
| `~/.codex-claw/codexclaw.toml` | Main configuration file |
| `~/.codex-claw/data/` | Runtime data directory |
| `~/.codex-claw/.codex/` | Independent Codex runtime directory |
| `~/.codex-claw/data/session/` | Session storage |
| `~/.codex-claw/data/memory/` | Memory distillation data |
| `~/.codex-claw/data/scheduler/` | Scheduler persistence data |

## Documentation

| Document | 中文 |
|----------|------|
| [Getting Started](docs/getting-started_en.md) | [快速入门](docs/getting-started.md) |
| [Command Reference](docs/commands_en.md) | [命令参考](docs/commands.md) |
| [Configuration Reference](docs/configuration_en.md) | [配置参考](docs/configuration.md) |
| [Scheduler](docs/scheduler_en.md) | [定时任务](docs/scheduler.md) |
| [Architecture](docs/architecture_en.md) | [系统架构](docs/architecture.md) |
| [Contributing](CONTRIBUTING_en.md) | [贡献指南](CONTRIBUTING.md) |

## Contributing

This project primarily evolves around the author's personal needs and interests. If you have new ideas or use cases, feel free to discuss them in the Discussion section. If you find a clear bug or compatibility issue, feel free to submit an Issue. For feature suggestions, the author does not guarantee acceptance, scheduling, or long-term maintenance.

If your needs are highly personalized or do not fully align with the current roadmap, you can fork this repository directly and continue customizing on top of it. If you build something interesting, the author encourages you to **share functionality by sharing the Prompts or Plans you used**.

For detailed development guidelines, see [CONTRIBUTING.md](CONTRIBUTING_en.md).

## Deployment Instructions for Codex

1. First check the basic environment, confirm that the current machine has internet access and that Rust and the Codex CLI are already installed:

```bash
uname -a
whoami
pwd
rustc --version
cargo --version
codex --version
git --version
```

2. If the Rust toolchain is missing, install Rust. If `codex` is missing, install it using the current official method first and require the user to complete login. Verification command. If this step fails, do not continue deployment; first ask the user to fix the Codex runtime environment and credential issues.

```bash
codex exec --json "reply with exactly: ok"
```

3. Confirm that the system Codex directory `~/.codex` already exists and contains at least `config.toml` and `auth.json`. On first startup, `codex-claw` will copy these files along with `skills/` to `~/.codex-claw/.codex/` as an independent runtime directory.

4. Deploy using the user's home directory (no elevated privileges required), for example:

```bash
mkdir -p ~/.codex-claw
cd ~/.codex-claw
git clone https://github.com/Rhapsody0x1/CodexClaw.git repo
cd repo
```

If the directory already contains the repository, instead do:

```bash
cd ~/.codex-claw/repo
git pull --ff-only
```

5. Create the runtime configuration file, e.g. `~/.codex-claw/codexclaw.toml`. Write the user-provided `AppID` and `AppSecret` into the corresponding fields:

```toml
[general]
data_dir = "~/.codex-claw/data"
system_codex_home = "~/.codex"
codex_home_global = "~/.codex-claw/.codex"
default_workspace_dir = "~/.codex-claw/data/session/workspace"
codex_binary = "codex"
default_model = "gpt-5.4"
default_reasoning_effort = "medium"
self_repo_dir = "~/.codex-claw/repo"
self_build_command = "cargo build --release"
self_binary_path = "~/.codex-claw/repo/target/release/codex-claw"

[qq]
app_id = "YOUR_APP_ID"
app_secret = "YOUR_APP_SECRET"
api_base_url = "https://sandbox.api.sgroup.qq.com"
token_url = "https://bots.qq.com/app/getAppAccessToken"

[scheduler]
enabled = true
tick_secs = 30
default_tz = "Asia/Shanghai"
max_concurrent_jobs = 4
max_turn_secs = 600
max_attempts = 3
retry_backoff_secs = 30
circuit_breaker_threshold = 5
runs_retention = 30
```

6. First do a compile check, then build the release:

```bash
cd ~/.codex-claw/repo
cargo check
cargo build --release
```

7. Start once in the foreground to confirm the program can connect to the QQ Gateway normally and there are no obvious configuration errors:

```bash
cd ~/.codex-claw/repo
CODEX_CLAW_CONFIG=~/.codex-claw/codexclaw.toml ./target/release/codex-claw
```

If the logs show access token retrieval failures, Gateway connection failures, or Codex startup failures, stop and fix the problem first before continuing with subsequent steps.

8. When registering as a "user-level auto-start service", handle flexibly based on the system environment (e.g. macOS `launchd`, Linux user-level `systemd`, or other init systems). Core requirements:
- Working directory points to `~/.codex-claw/repo`
- Set `CODEX_CLAW_CONFIG=~/.codex-claw/codexclaw.toml`
- Startup command is `~/.codex-claw/repo/target/release/codex-claw`
- Run as the current logged-in user, no root required

9. Enable and start the service (commands vary by system). Since `/self-update` replaces the currently running binary and exits the current process, it is recommended to let an external service manager handle restarting.

10. Finally, remind the user to send a normal private message from the QQ client for integration testing, and check the service logs.
