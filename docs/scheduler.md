# CodexClaw Scheduler -- 定时任务系统 (Scheduled Task System)

> 版本对应: `vibe-dev` 分支，2026-05

---

## 目录 (Table of Contents)

1. [架构概览 (Architecture)](#架构概览-architecture)
2. [任务模型 (Job Schema)](#任务模型-job-schema)
3. [调度类型 (CronKind)](#调度类型-cronkind)
4. [任务动作 (JobAction)](#任务动作-jobaction)
5. [交互模式 (Interactive Mode)](#交互模式-interactive-mode)
6. [CLI 命令 (CLI Commands)](#cli-命令-cli-commands)
7. [QQ 命令 (QQ Commands)](#qq-命令-qq-commands)
8. [目录结构 (Directory Layout)](#目录结构-directory-layout)
9. [生命周期 (Lifecycle)](#生命周期-lifecycle)
10. [最佳实践 (Best Practices)](#最佳实践-best-practices)
11. [English Summary](#english-summary)

---

## 架构概览 (Architecture)

Scheduler 是一个运行在 tokio 异步运行时上的无限循环任务。核心参数:

| 参数 | 默认值 | 说明 |
|------|--------|------|
| tick 间隔 | 30 秒 | 每次检查是否有任务需要执行的间隔 |
| 最大并发数 | 4 | 通过 tokio `Semaphore` 控制，防止资源耗尽 |
| 去重机制 | in-flight set | 同一任务不会被同时调度两次 |

每个 tick，Scheduler 扫描任务表，找出 `next_run_at <= now` 或 `run_now_at` 已设置的任务，
获取 semaphore permit 后分派执行。

---

## 任务模型 (Job Schema)

任务以 `CronJob` 结构体定义（`src/scheduler/store.rs`），字段如下:

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `String` | ULID，全局唯一标识 |
| `owner_openid` | `String` | 任务所有者的 QQ OpenID |
| `title` | `String` | 任务名称，用于列表展示 |
| `kind` | `CronKind` | 调度类型：周期性或一次性 |
| `action` | `JobAction` | 执行动作，见下方详细说明 |
| `workspace_dir` | `PathBuf` | 任务的工作目录 |
| `deliver` | `DeliverPolicy` | 结果推送策略 |
| `created_at` | `DateTime<Utc>` | 创建时间 |
| `next_run_at` | `DateTime<Utc>` | 下次计划执行时间 |
| `run_now_at` | `Option<DateTime<Utc>>` | 手动触发的立即执行时间 |
| `last_run_at` | `Option<DateTime<Utc>>` | 上次实际执行时间 |
| `last_run_status` | `RunStatus` | 上次执行状态 |
| `run_count` | `u64` | 累计执行次数 |
| `failure_streak` | `u32` | 连续失败次数（熔断依据） |
| `disabled` | `bool` | 是否已停用 |

### 推送策略 (DeliverPolicy)

| 枚举值 | 行为 |
|--------|------|
| `PushToOwner` | 始终将结果推送给 owner |
| `PushIfNonEmpty` | 仅当输出非空时推送 |
| `LogOnly` | 仅写日志，不推送 |
| `PushTruncated` | 推送截断后的结果（适用于大输出） |

### 运行状态 (RunStatus)

| 枚举值 | 说明 |
|--------|------|
| `Success` | 执行成功 |
| `Failure` | 执行失败 |
| `Skipped` | 跳过（例如被熔断或人为禁用） |

---

## 调度类型 (CronKind)

### Recurring -- 周期性任务

```rust
Recurring { cron: String, tz: String }
```

使用标准 **6 字段 cron 表达式**: `秒 分 时 日 月 星期`

如果用户提供的是 5 字段表达式（省略秒），系统会自动在前面补 `"0"`。

示例:

| 表达式 | 含义 |
|--------|------|
| `0 30 9 * * *` | 每天 09:30:00 |
| `0 0 */2 * * *` | 每 2 小时整点 |
| `0 0 8 * * Mon-Fri` | 工作日早 8 点 |

`tz` 字段指定时区（如 `Asia/Shanghai`、`UTC`）。

### OneShot -- 一次性任务

```rust
OneShot { at: DateTime<Utc> }
```

在指定的 UTC 时间点触发一次，执行完成后归档至 `cron-jobs-trash/`。

---

## 任务动作 (JobAction)

### 1. Reminder -- 提醒

```rust
Reminder { message: String }
```

最简单的动作。到达执行时间时，将 `message` 通过 QQ 推送给 owner。
适用于简单的定时提醒场景。

### 2. CodexTurn -- Codex 回合（推荐）

```rust
CodexTurn {
    prompt: String,
    model: String,
    session_state: ...,
    approval_policy: ...,
    session_strategy: SessionStrategy,
    interactive: Option<InteractiveSpec>,
}
```

通过 app-server 管线执行一次完整的 Codex turn，是最强大的任务动作。

**session_strategy** 决定会话生命周期:

| 策略 | 说明 |
|------|------|
| `PerInvocation` | 每次执行创建全新会话（默认） |
| `Persistent` | 跨次执行复用会话，保留上下文 |

可选附带 `interactive` 字段启用交互模式（见下节）。

### 3. CodexExec -- Codex 执行（旧路径）

```rust
CodexExec {
    prompt: String,
    model: String,
    extra_args: Vec<String>,
    env: HashMap<String, String>,
}
```

通过 `codex exec` CLI 执行，属于旧版路径。新任务建议使用 `CodexTurn`。

### 4. Shell -- Shell 命令

```rust
Shell {
    program: String,
    args: Vec<String>,
    env: HashMap<String, String>,
}
```

执行任意 shell 命令。适用于简单的脚本调用或系统命令。

---

## 交互模式 (Interactive Mode)

交互模式允许定时任务在触发后与用户进行多轮对话，由 `InteractiveSpec` 配置:

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `reply_ttl_secs` | `86400`（24 小时） | 等待用户回复的超时时间 |
| `end_signal` | `"<<<CLAW_END>>>"` | Codex 发出此信号表示交互结束 |
| `max_rounds_hard_cap` | `10` | 最大来回轮次上限 |

### 交互流程 (Flow)

```
┌─────────────────────────────────────────────────────────┐
│  1. 任务到达执行时间，携带 interactive spec 触发         │
│  2. Codex 收到包含交互协议说明的 prompt                  │
│  3. 用户当前前台会话被挂起至后台                          │
│  4. 在任务 workspace 中创建新的前台会话                   │
│  5. 用户后续的 QQ 消息路由到此交互线程                    │
│  6. Codex 与用户来回对话                                  │
│     └─ 直到 Codex 发出 end_signal 或达到 max_rounds      │
│  7. 交互结束：                                            │
│     ├─ 前台会话停止（Persistent 模式下转为后台）          │
│     └─ 恢复用户原来的前台会话                             │
│  8. 向用户发送开始/结束 banner 消息                       │
└─────────────────────────────────────────────────────────┘
```

---

## CLI 命令 (CLI Commands)

所有命令通过 `codex-claw cron` 子命令调用。

### `cron add` -- 创建周期性任务

```bash
codex-claw cron add \
  --owner OPENID \
  --cron "min hour dom month dow" \
  --tz Asia/Shanghai \
  --title "每日新闻摘要" \
  --action codex-turn \
  --prompt "收集今日 AI 领域重要新闻并生成摘要" \
  --workspace /path/to/workspace \
  --model o3-mini \
  --session-strategy per-invocation \
  --approval auto-edit
```

全部标志:

| 标志 | 说明 |
|------|------|
| `--owner OPENID` | 任务所有者 |
| `--cron EXPR` | 5 字段 cron 表达式（秒会自动补 0） |
| `--tz TIMEZONE` | 时区 |
| `--title NAME` | 任务名称 |
| `--action TYPE` | 动作类型：`reminder` / `codex-turn` / `codex-exec` / `shell` |
| `--message TEXT` | `reminder` 动作的消息内容 |
| `--prompt TEXT` | Codex 动作的 prompt |
| `--prompt-file PATH` | 从文件读取 prompt |
| `--workspace PATH` | 工作目录 |
| `--model MODEL` | 模型名称 |
| `--session-strategy` | `per-invocation` 或 `persistent` |
| `--approval POLICY` | 审批策略 |
| `--interactive` | 启用交互模式 |
| `--reply-ttl SECS` | 交互模式：回复超时秒数 |
| `--end-signal TEXT` | 交互模式：结束信号 |
| `--max-rounds N` | 交互模式：最大轮次 |
| `--program PATH` | `shell` 动作的程序路径 |
| `--arg VALUE` | `shell` 动作的参数（可多次指定） |
| `--extra-arg VALUE` | `codex-exec` 动作的额外参数 |

### `cron once` -- 创建一次性任务

```bash
codex-claw cron once \
  --owner OPENID \
  --at "2026-05-12T10:00:00Z" \
  --title "发布提醒" \
  --action reminder \
  --message "v2.0 发布窗口已到，请检查发布清单"
```

与 `cron add` 相同的标志，但用 `--at RFC3339_DATETIME` 替代 `--cron`。

### `cron list` -- 列出任务

```bash
codex-claw cron list            # 列出所有任务
codex-claw cron list --owner ID # 按 owner 过滤
```

输出字段: `id`, `enabled/disabled`, `next_run_at`, `run_count`, `failure_streak`, `owner`, `title`

### `cron rm <job_id>` -- 删除任务

```bash
codex-claw cron rm 01J5K...     # 删除任务及其文件
codex-claw cron rm 01J5K... --keep-files  # 保留工作目录
```

### `cron pause <job_id>` -- 暂停任务

```bash
codex-claw cron pause 01J5K...
```

设置 `disabled = true`，任务不再被调度。

### `cron resume <job_id>` -- 恢复任务

```bash
codex-claw cron resume 01J5K...
```

设置 `disabled = false` 并重新计算 `next_run_at`。

### `cron run-now <job_id>` -- 立即执行

```bash
codex-claw cron run-now 01J5K...
```

设置 `run_now_at` 触发一次额外的立即执行，**不影响**正常的 `next_run_at` 调度。

### `cron tail <job_id>` -- 查看运行日志

```bash
codex-claw cron tail 01J5K...
```

显示任务状态摘要和最近一次运行日志的内容。

---

## QQ 命令 (QQ Commands)

用户可在 QQ 中通过以下命令管理自己的定时任务:

| 命令 | 说明 |
|------|------|
| `/cron list` | 列出自己的任务 |
| `/cron pause <id>` | 暂停任务 |
| `/cron resume <id>` | 恢复任务 |
| `/cron rm <id>` | 删除任务 |
| `/cron run-now <id>` | 立即执行一次 |
| `/cron tail <id>` | 查看最近运行日志 |

中文别名: `/定时`（等价于 `/cron`）

---

## 目录结构 (Directory Layout)

```
data/
├── scheduler/
│   ├── jobs.json                  # 任务表（跨进程文件锁保护）
│   └── pending-deliveries/        # 推送失败暂存
│       └── <openid>.jsonl
├── cron-jobs/
│   └── <job_id>/
│       ├── job.toml               # 任务元数据
│       ├── workspace/             # 执行目录
│       │   ├── .claw-job.json     # 任务上下文
│       │   └── .agents/skills/    # 任务专属 Skill
│       ├── runs/                  # 运行日志（保留最近 N 次）
│       │   └── 20260510T100000Z.log
│       └── pending.json           # 交互式任务状态（仅活跃时存在）
└── cron-jobs-trash/               # 一次性任务完成后归档
    └── <timestamp>-<job_id>/
```

每个任务的 skill 目录通过符号链接关联到
`~/.codex-claw/.codex/skills/claw-cron-<job_id>`，
使 Codex 在执行该任务时能发现其专属 skill。

---

## 生命周期 (Lifecycle)

```
创建 ──> 调度 ──> 执行 ──> 成功 ──> 更新 next_run_at ──> 调度（循环）
 │                 │                                       │
 │                 └──> 失败 ──> 重试 ──> 熔断 ──> 停用    │
 │                                                          │
 └── 一次性任务 ──> 执行 ──> 归档至 cron-jobs-trash/ ──────┘
```

### 详细阶段

1. **创建 (Create)**
   CLI `add` 或 `once` 命令触发。写入 `jobs.json`，创建目录结构，注册 skill 符号链接。

2. **调度 (Schedule)**
   Scheduler tick（默认 30 秒一次）扫描任务表，检查 `next_run_at <= now` 或 `run_now_at` 已设置。

3. **执行 (Execute)**
   获取 semaphore permit，根据 `action` 类型分派执行。每次执行有超时保护（`max_turn_secs`）。

4. **重试 (Retry)**
   执行失败后，按 `retry_backoff_secs` 间隔重试，最多 `max_attempts` 次。

5. **熔断 (Circuit Breaker)**
   连续失败次数达到 `circuit_breaker_threshold` 时，自动停用任务并通知 owner。

6. **归档 (Archive)**
   一次性任务执行完成后，整个任务目录移入 `cron-jobs-trash/`。

7. **推送失败处理 (Delivery Recovery)**
   结果投递失败时，暂存到 `pending-deliveries/<openid>.jsonl`。
   用户下次发送 QQ 消息时自动补发。

---

## 最佳实践 (Best Practices)

### 为定时任务编写专属 Skill

对于新闻收集、市场扫描、论文摘要等有明显工作流特征的定时任务，
Agent 应先和用户确认以下要素，再创建任务:

- **数据源**: 从哪里获取信息
- **过滤规则**: 什么内容需要被纳入
- **输出格式**: 结果以何种形式呈现
- **失败策略**: 出错后如何处理

确认后，在任务的 `workspace/.agents/skills/` 目录中写入任务专属 Skill，
使 Codex 在每次执行时拥有明确的工作指引。

**不要** 只用一句笼统的 prompt 创建每天运行的 `codex-exec` 任务。

### 选择合适的动作类型

| 场景 | 推荐动作 |
|------|----------|
| 简单文字提醒 | `Reminder` |
| 需要 AI 理解和生成的复杂任务 | `CodexTurn`（首选） |
| 需要与用户交互的定时任务 | `CodexTurn` + `--interactive` |
| 固定脚本调用 | `Shell` |
| 旧版兼容 | `CodexExec` |

### 选择会话策略

- **PerInvocation**（默认）: 每次执行互不影响。适合独立的重复性任务。
- **Persistent**: 跨次保留上下文。适合需要记住历史的连续性任务（如追踪项目进度）。

---

## English Summary

CodexClaw's scheduler is a tokio-based background task system that powers recurring and one-shot jobs for the QQ bot. Key highlights:

**Architecture** -- An infinite-loop tokio task ticks every 30 seconds, using a semaphore (max 4 concurrent jobs) and an in-flight set to prevent duplicate execution.

**Job types** -- Each job has a `CronKind` (Recurring with a 6-field cron expression, or OneShot at a fixed UTC time) and a `JobAction`:
- *Reminder* -- send a message to the owner via QQ.
- *CodexTurn* -- run a full Codex turn through the app-server pipeline (recommended).
- *CodexExec* -- run via `codex exec` CLI (legacy).
- *Shell* -- run an arbitrary command.

**Interactive mode** -- CodexTurn jobs can include an `InteractiveSpec` that parks the user's current session, opens a new foreground session in the job workspace, and lets Codex chat with the user in real time until an end signal is emitted or the round cap is reached.

**CLI** -- `codex-claw cron {add, once, list, rm, pause, resume, run-now, tail}`.
**QQ** -- `/cron {list, pause, resume, rm, run-now, tail}` (alias: `/定时`).

**Lifecycle** -- Create -> Schedule -> Execute -> Retry on failure -> Circuit-break after repeated failures -> Archive (one-shot). Delivery failures are buffered in `pending-deliveries/` and retried on the owner's next QQ message.

**Best practice** -- For workflow-oriented tasks (news, market scans, paper summaries), the agent should confirm data sources, filters, output format, and failure strategies with the user, then write a dedicated skill into the job's `workspace/.agents/skills/` directory rather than relying on a single generic prompt.
