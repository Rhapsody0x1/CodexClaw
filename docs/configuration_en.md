*Read this in: [English](configuration_en.md) | [中文](configuration.md)*

# CodexClaw Configuration Reference

This document describes all configuration options for CodexClaw. The configuration file uses the TOML format.

---

## Config Loading Order

CodexClaw looks for a configuration file in the following order, using the first one found:

1. The path specified by the `CODEX_CLAW_CONFIG` environment variable (if set)
2. `./codexclaw.toml` in the current working directory
3. Fallback to `~/.codex-claw/codexclaw.toml`

### Startup Validation

The following fields are required and must not be empty strings; otherwise the program will fail to start:

- `qq.app_id`
- `qq.app_secret`
- `general.self_build_command`

---

## Path Handling Notes

- **Tilde Expansion**: In all fields of type `PathBuf`, a leading `~` is expanded at runtime to the actual value of `$HOME`. For example, `~/.codex-claw/data` expands to `/home/youruser/.codex-claw/data`.
- **Relative Paths**: If a relative path is used in the configuration (e.g. `"."`), it is resolved relative to the CodexClaw process's current working directory.

---

## `[general]` — General Settings

Controls runtime directories, Codex CLI invocation method, and self-update behavior.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `data_dir` | PathBuf | `~/.codex-claw/data` | Root directory for runtime data |
| `system_codex_home` | PathBuf | `~/.codex` | System Codex installation directory |
| `codex_home_global` | PathBuf | `~/.codex-claw/.codex` | CodexClaw-isolated Codex runtime directory |
| `default_workspace_dir` | PathBuf | `~/.codex-claw/data/session/workspace` | Default working directory for new temporary foreground sessions |
| `codex_binary` | String | `"codex"` | Codex CLI executable path or command name |
| `default_model` | String | `"gpt-5.4"` | Default model for new sessions |
| `default_reasoning_effort` | ReasoningEffort | `medium` | Default reasoning depth; valid values: `low` / `medium` / `high` / `xhigh` |
| `self_repo_dir` | PathBuf | `"."` | CodexClaw repository root directory (used by the `/self-update` command) |
| `self_build_command` | String | `"cargo build --release"` | Build command to run during self-update (**required, must not be empty**) |
| `self_binary_path` | PathBuf | `"./target/release/codex-claw"` | Path to the build output binary |

---

## `[qq]` — QQ Bot Settings

Configures authentication credentials and API endpoints for the QQ Open Platform.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `app_id` | String | **(required)** | QQ bot AppID |
| `app_secret` | String | **(required)** | QQ bot AppSecret |
| `api_base_url` | String | `"https://sandbox.api.sgroup.qq.com"` | QQ API endpoint. For production, change to `https://api.sgroup.qq.com` |
| `token_url` | String | `"https://bots.qq.com/app/getAppAccessToken"` | Token acquisition endpoint |

> **Note**: The default `api_base_url` points to the sandbox environment. When deploying to production, be sure to change it to `https://api.sgroup.qq.com`.

---

## `[shadow]` — Shadow Distillation Settings

Controls the behavior of the background memory distillation and skill distillation modules. When `enabled = false`, the entire shadow subsystem will not run.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Whether to enable background memory/skill distillation |
| `memory_min_user_chars` | usize | `40` | Minimum user message length (in characters) to trigger memory distillation |
| `memory_reasoning` | String | `"low"` | Reasoning depth used for memory distillation |
| `memory_model` | String | `""` | Model used for memory distillation. Leave empty to follow the current session model |
| `memory_deadline_secs` | u64 | `120` | Timeout for a single distillation run (in seconds) |
| `skill_files_threshold` | usize | `2` | Minimum number of modified files to trigger skill distillation |
| `skill_tool_threshold` | usize | `5` | Minimum number of tool calls to trigger skill distillation |

---

## `[scheduler]` — Scheduler Settings

Controls the task scheduler. The scheduler supports cron-expression-based task triggering and includes built-in retry and circuit-breaker mechanisms.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Whether to enable the scheduler |
| `tick_secs` | u64 | `30` | Scheduler polling interval (in seconds) |
| `default_tz` | String | `"Asia/Shanghai"` | Default timezone (IANA timezone name) |
| `max_concurrent_jobs` | usize | `4` | Maximum number of concurrent jobs |
| `max_turn_secs` | u64 | `600` | Timeout for a single job execution (in seconds) |
| `max_attempts` | u32 | `3` | Maximum number of retries per run |
| `retry_backoff_secs` | u64 | `30` | Retry backoff interval (in seconds) |
| `circuit_breaker_threshold` | u32 | `5` | Consecutive failure threshold; the job is auto-disabled after reaching this count |
| `runs_retention` | usize | `30` | Number of recent run logs to retain |

---

## Full Example

Below is a complete configuration file containing all fields, which can be used as a starting template.

```toml
# ============================================================
#  CodexClaw Configuration File
#  Copy this file to one of the following locations:
#    - ./codexclaw.toml (current working directory)
#    - ~/.codex-claw/codexclaw.toml (user home directory)
#  Or specify the path via the CODEX_CLAW_CONFIG environment variable.
# ============================================================

# --- General Settings ---------------------------------------------------
[general]
data_dir              = "~/.codex-claw/data"
system_codex_home     = "~/.codex"
codex_home_global     = "~/.codex-claw/.codex"
default_workspace_dir = "~/.codex-claw/data/session/workspace"
codex_binary          = "codex"
default_model         = "gpt-5.4"
default_reasoning_effort = "medium"      # low | medium | high | xhigh
self_repo_dir         = "."
self_build_command    = "cargo build --release"
self_binary_path      = "./target/release/codex-claw"

# --- QQ Bot --------------------------------------------------
[qq]
app_id       = "YOUR_APP_ID"             # (required) Replace with your AppID
app_secret   = "YOUR_APP_SECRET"         # (required) Replace with your AppSecret
api_base_url = "https://sandbox.api.sgroup.qq.com"   # For production, change to https://api.sgroup.qq.com
token_url    = "https://bots.qq.com/app/getAppAccessToken"

# --- Shadow Distillation ---------------------------------------------------
[shadow]
enabled              = true
memory_min_user_chars = 40               # Memory distillation not triggered if user message is shorter than this
memory_reasoning     = "low"
memory_model         = ""                # Leave empty = follow session model
memory_deadline_secs = 120
skill_files_threshold = 2                # Trigger skill distillation when modified files >= 2
skill_tool_threshold  = 5                # Trigger skill distillation when tool calls >= 5

# --- Scheduler -----------------------------------------------------
[scheduler]
enabled                  = true
tick_secs                = 30
default_tz               = "Asia/Shanghai"
max_concurrent_jobs      = 4
max_turn_secs            = 600           # 10 minutes
max_attempts             = 3
retry_backoff_secs       = 30
circuit_breaker_threshold = 5            # Auto-disable after 5 consecutive failures
runs_retention           = 30
```

---
