*Read this in: [English](configuration_en.md) | [中文](configuration.md)*

# CodexClaw 配置参考 (Configuration Reference)

本文档描述 CodexClaw 的全部配置项。配置文件使用 TOML 格式。

---

## 配置文件加载顺序

CodexClaw 按以下顺序查找配置文件，使用第一个找到的文件：

1. 环境变量 `CODEX_CLAW_CONFIG` 指定的路径（如果设置）
2. 当前工作目录下的 `./codexclaw.toml`
3. 回退路径 `~/.codex-claw/codexclaw.toml`

### 启动校验

以下字段为必填项，不能为空字符串，否则程序启动将失败：

- `qq.app_id`
- `qq.app_secret`
- `general.self_build_command`

---

## 路径处理说明

- **波浪号展开**：所有 `PathBuf` 类型的字段中，前缀 `~` 会在运行时展开为 `$HOME` 的实际值。例如 `~/.codex-claw/data` 会展开为 `/home/youruser/.codex-claw/data`。
- **相对路径**：如果配置中使用了相对路径（如 `"."`），则相对于 CodexClaw 进程的当前工作目录解析。

---

## `[general]` — 通用配置

控制运行时目录、Codex CLI 调用方式、以及自更新行为。

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `data_dir` | PathBuf | `~/.codex-claw/data` | 运行时数据根目录 |
| `system_codex_home` | PathBuf | `~/.codex` | 系统 Codex 安装目录 |
| `codex_home_global` | PathBuf | `~/.codex-claw/.codex` | CodexClaw 隔离的 Codex 运行目录 |
| `default_workspace_dir` | PathBuf | `~/.codex-claw/data/session/workspace` | 新建临时前台会话的默认工作目录 |
| `codex_binary` | String | `"codex"` | Codex CLI 可执行文件路径或命令名 |
| `default_model` | String | `"gpt-5.4"` | 新建会话的默认模型 |
| `default_reasoning_effort` | ReasoningEffort | `medium` | 默认推理深度，可选值：`low` / `medium` / `high` / `xhigh` |
| `self_repo_dir` | PathBuf | `"."` | CodexClaw 仓库根目录（用于 `/self-update` 命令） |
| `self_build_command` | String | `"cargo build --release"` | 自更新时执行的编译命令（**必填，不可为空**） |
| `self_binary_path` | PathBuf | `"./target/release/codex-claw"` | 编译产物路径 |

---

## `[qq]` — QQ 机器人配置

配置 QQ 开放平台的认证信息和 API 端点。

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `app_id` | String | **(必填)** | QQ 机器人 AppID |
| `app_secret` | String | **(必填)** | QQ 机器人 AppSecret |
| `api_base_url` | String | `"https://sandbox.api.sgroup.qq.com"` | QQ API 端点。正式环境请改为 `https://api.sgroup.qq.com` |
| `token_url` | String | `"https://bots.qq.com/app/getAppAccessToken"` | Token 获取端点 |

> **注意**：默认的 `api_base_url` 指向沙箱环境。部署到生产环境时，务必将其改为
> `https://api.sgroup.qq.com`。

---

## `[shadow]` — 后台蒸馏配置

控制后台记忆蒸馏和技能蒸馏模块的行为。当 `enabled = false` 时，整个 shadow 子系统不会运行。

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | bool | `true` | 是否启用后台记忆/技能蒸馏 |
| `memory_min_user_chars` | usize | `40` | 触发记忆蒸馏的最小用户消息长度（字符数） |
| `memory_reasoning` | String | `"low"` | 记忆蒸馏使用的推理深度 |
| `memory_model` | String | `""` | 记忆蒸馏使用的模型。留空则跟随当前会话模型 |
| `memory_deadline_secs` | u64 | `120` | 单次蒸馏超时时间（秒） |
| `skill_files_threshold` | usize | `2` | 触发技能蒸馏的最小修改文件数 |
| `skill_tool_threshold` | usize | `5` | 触发技能蒸馏的最小工具调用次数 |

---

## `[scheduler]` — 调度器配置

控制定时任务调度器。调度器支持 cron 表达式触发任务，并内置重试、熔断机制。

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | bool | `true` | 是否启用调度器 |
| `tick_secs` | u64 | `30` | 调度器轮询间隔（秒） |
| `default_tz` | String | `"Asia/Shanghai"` | 默认时区（IANA 时区名称） |
| `max_concurrent_jobs` | usize | `4` | 最大并发任务数 |
| `max_turn_secs` | u64 | `600` | 单次任务执行超时（秒） |
| `max_attempts` | u32 | `3` | 单次运行最大重试次数 |
| `retry_backoff_secs` | u64 | `30` | 重试间隔（秒） |
| `circuit_breaker_threshold` | u32 | `5` | 连续失败阈值，达到后任务自动停用 |
| `runs_retention` | usize | `30` | 保留最近 N 次运行日志 |

---

## 完整配置示例

以下是一份包含所有字段的完整配置文件，可作为起始模板使用。

```toml
# ============================================================
#  CodexClaw 配置文件
#  复制此文件到以下任一位置：
#    - ./codexclaw.toml（当前工作目录）
#    - ~/.codex-claw/codexclaw.toml（用户目录）
#  或者通过 CODEX_CLAW_CONFIG 环境变量指定路径。
# ============================================================

# --- 通用配置 ---------------------------------------------------
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

# --- QQ 机器人 --------------------------------------------------
[qq]
app_id       = "YOUR_APP_ID"             # (必填) 替换为你的 AppID
app_secret   = "YOUR_APP_SECRET"         # (必填) 替换为你的 AppSecret
api_base_url = "https://sandbox.api.sgroup.qq.com"   # 正式环境改为 https://api.sgroup.qq.com
token_url    = "https://bots.qq.com/app/getAppAccessToken"

# --- 后台蒸馏 ---------------------------------------------------
[shadow]
enabled              = true
memory_min_user_chars = 40               # 用户消息少于此字符数时不触发记忆蒸馏
memory_reasoning     = "low"
memory_model         = ""                # 留空 = 跟随会话模型
memory_deadline_secs = 120
skill_files_threshold = 2                # 修改文件 >= 2 时触发技能蒸馏
skill_tool_threshold  = 5                # 工具调用 >= 5 时触发技能蒸馏

# --- 调度器 -----------------------------------------------------
[scheduler]
enabled                  = true
tick_secs                = 30
default_tz               = "Asia/Shanghai"
max_concurrent_jobs      = 4
max_turn_secs            = 600           # 10 分钟
max_attempts             = 3
retry_backoff_secs       = 30
circuit_breaker_threshold = 5            # 连续失败 5 次后自动停用
runs_retention           = 30
```
