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

## `[codex_provider]` — 自定义 Codex 模型后端（xAI Grok 等）

CodexClaw 仍通过 **Codex App-Server** 跑 agent（工具、审批、沙箱不变），但可以把隔离的 `CODEX_HOME`（`general.codex_home_global`）里的 Codex `config.toml` 写成自定义 `model_providers`，从而走 **xAI Grok OpenAI 兼容 API**，而不是只能用默认 OpenAI/Codex 账号。

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | bool | `false` | 是否在启动时把该 provider 写入隔离 Codex 的 `config.toml` |
| `id` | String | `"xai"` | `model_provider` 标识，对应 `[model_providers.<id>]` |
| `name` | String | `"xAI Grok"` | 展示名称 |
| `base_url` | String | `"https://api.x.ai/v1"` | OpenAI 兼容 API 根路径 |
| `env_key` | String | `"XAI_API_KEY"` | Codex 从此环境变量读取 API Key |
| `wire_api` | String | `"responses"` | Codex 线协议；xAI 支持 `/v1/responses`，与当前 Codex 要求一致 |
| `set_as_default` | bool | `true` | 是否写入顶层 `model_provider`（及可选 `model`） |
| `default_model` | String? | `"grok-4"` | 启用且 `set_as_default` 时写入的顶层 `model` |
| `models` | String[] | Grok 常用 id 列表 | 额外模型 id（与 `config/codex_models.toml` 中的 Grok 条目一起出现在 `/model`） |

### 启用 xAI Grok 的步骤

1. 在 [xAI Console](https://console.x.ai/) 创建 API Key。
2. 导出环境变量（进程需能读到）：

```bash
export XAI_API_KEY="xai-..."
```

3. 在 `codexclaw.toml` 中启用：

```toml
[codex_provider]
enabled = true
# 以下为默认值，可省略；需要时可覆盖 base_url / model
# id = "xai"
# base_url = "https://api.x.ai/v1"
# env_key = "XAI_API_KEY"
# wire_api = "responses"
# default_model = "grok-4"
```

4. 可选：把会话默认模型也改成 Grok：

```toml
[general]
default_model = "grok-4"
```

5. 重启 CodexClaw。启动日志中会出现 `applied codex_provider into isolated Codex home config.toml`。之后可用 `/model grok-4` 或 `/model grok-3-mini` 等切换。

6. 运行时可发 `/切换模型`（或 `/switch_model`）在 **Codex** 与 **Grok** 后端之间一键切换；也可用 `/切换模型 status` 查看、`/切换模型 codex` / `/切换模型 grok` 强制指定。这只会改写隔离 `CODEX_HOME` 的 `config.toml`（`model_provider` + `model`），**不会**新开第二个 bot 进程。

### `[openai_provider]` — `/switch_model` 切回 Codex 时用的后端

多数自建环境的 “Codex” 并不是 `api.openai.com`，而是第三方兼容中转（例如 `https://chat.soruxgpt.com/codex` + `OPENAI_API_KEY`）。  
请在这里写明该中转，否则 `/切换模型` → Codex 可能清掉 `model_provider` 后误打官方 OpenAI（第三方 key 会 401）。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `enabled` | bool | `true` | 是否在切到 Codex 时应用此 provider（仍需配置 `base_url`） |
| `id` | String | `"mirror"` | `model_provider` 标识 |
| `name` | String | `"mirror"` | 展示名 |
| `base_url` | String | `""` | 中转 API 根；**为空则不会应用**，改尝试从现有 config 恢复非 Grok provider |
| `env_key` | String | `"OPENAI_API_KEY"` | API Key 环境变量名 |
| `wire_api` | String | `"responses"` | 线协议 |
| `default_model` | String? | `"gpt-5.5"` | 切到 Codex 时写入的 `model` |
| `requires_openai_auth` | bool? | — | 可选，写入 provider 表 |
| `preferred_auth_method` | String? | — | 可选，如 `"apikey"` |

```toml
[openai_provider]
enabled = true
id = "mirror"
base_url = "https://chat.soruxgpt.com/codex"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
default_model = "gpt-5.5"
requires_openai_auth = false
preferred_auth_method = "apikey"
```

> **说明**：`codex_provider.enabled = false`（默认）时启动行为与改前一致。Grok 模型名会出现在内置目录中，但未启用 Grok provider 时后端仍是原路径。

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

# --- 可选：xAI Grok / 自定义 OpenAI 兼容后端 ----------------------
# 默认关闭。启用后会把隔离 CODEX_HOME 的 config.toml 写成 Grok provider。
# [codex_provider]
# enabled       = true
# id            = "xai"
# name          = "xAI Grok"
# base_url      = "https://api.x.ai/v1"
# env_key       = "XAI_API_KEY"
# wire_api      = "responses"
# set_as_default = true
# default_model = "grok-4"
# models        = ["grok-4", "grok-4.5", "grok-3", "grok-3-mini"]
```
