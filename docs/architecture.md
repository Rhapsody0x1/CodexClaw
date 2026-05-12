# CodexClaw System Architecture

> CodexClaw 系统架构文档
>
> 本文档以中文为主体，章节标题使用英文。文末附有英文摘要。

---

## Table of Contents

1. [Overview](#overview)
2. [System Diagram](#system-diagram)
3. [Module Responsibilities](#module-responsibilities)
4. [Key Data Flows](#key-data-flows)
5. [Async Patterns](#async-patterns)
6. [Extension Guide](#extension-guide)
7. [English Summary](#english-summary)

---

## Overview

CodexClaw 是一个约 24,000 行的 Rust 异步应用，通过长驻的 app-server 进程将 QQ（中国主流即时通讯平台）桥接到 OpenAI Codex CLI。系统提供以下核心能力：

| 能力               | 说明                                              |
| ------------------ | ------------------------------------------------- |
| 会话管理           | 按用户维护独立会话状态，支持前台/后台会话切换     |
| 后台记忆与技能蒸馏 | 自动从对话轮次中提取事实、洞察和技能信息           |
| 定时调度           | 类 cron 调度系统，支持提醒、Codex 任务、Shell 命令 |
| 自我更新           | 当 Codex 修改了源码文件时，自动检测并触发重编译   |

---

## System Diagram

```
┌─────────────────┐       WebSocket        ┌──────────────┐
│  QQ Gateway     │ <---------------------> │  gateway.rs  │
│  (Tencent API)  │                         └──────┬───────┘
└─────────────────┘                                │ C2CMessageEvent
                                                   v
┌─────────────────┐  HTTP REST   ┌─────────────────────────────────┐
│  QQ API Server  │ <----------> │            App (app.rs)         │
│  (send msgs)    │              │  ┌───────────┐ ┌─────────────┐  │
└─────────────────┘              │  │commands.rs│ │ PassiveEmit │  │
                                 │  └───────────┘ └─────────────┘  │
                                 └────┬──────┬──────┬──────┬───────┘
                                      │      │      │      │
                        ┌─────────────┘      │      │      └──────────────┐
                        v                    v      v                     v
                 ┌──────────────┐  ┌─────────────┐ ┌──────────────┐ ┌─────────┐
                 │ SessionStore │  │CodexExecutor│ │  Scheduler   │ │ Shadow  │
                 │ (session/)   │  │ (codex/)    │ │ (scheduler/) │ │Worker   │
                 └──────────────┘  └──────┬──────┘ └──────────────┘ └─────────┘
                                          │ JSON-RPC (stdio)
                                          v
                                 ┌──────────────────┐
                                 │ codex app-server  │
                                 │ (child process)   │
                                 └──────────────────┘
```

**数据流概述：**

- QQ Gateway 通过 WebSocket 将消息事件推送到 `gateway.rs`。
- `App` 作为中央调度器，协调命令解析、Codex 执行、审批流程和消息发送。
- `CodexExecutor` 通过 JSON-RPC（stdio 管道）与长驻的 codex app-server 子进程通信。
- `Scheduler` 独立运行定时任务，可通过 `App` 发送合成消息。
- `ShadowWorker` 在后台异步提取记忆和技能。

---

## Module Responsibilities

### src/main.rs -- Entry Point

程序入口。负责解析 CLI 参数（路由到调度器 CLI 或 Bot 主服务），加载配置文件，规范化路径，并按以下顺序引导启动所有服务：

```
数据目录 -> 会话存储 -> QQ API -> app-server -> executor
  -> memory -> shadow -> scheduler -> gateway
```

### src/app.rs (~1656 lines) -- Central Dispatcher

中央调度模块，是系统的核心枢纽。

| 职责                 | 说明                                                     |
| -------------------- | -------------------------------------------------------- |
| 轮次协调             | 使用 `AtomicBool` 忙碌标志，全局同一时间只允许一个轮次   |
| 消息处理流水线       | QQ 事件规范化 -> 附件下载 -> 命令解析 -> Codex 执行      |
| 审批代理集成         | 将 app-server 发起的审批请求路由到 QQ 用户               |
| 自我更新检测         | 当 Codex 修改了源码文件时触发重编译                      |

### src/commands.rs (~600+ lines) -- Command Dispatch

命令解析与分发模块。

- 解析斜杠命令（slash commands），支持中文别名规范化
- 交互式设置处理器（如模型选择器等）
- 别名展开，递归深度限制为 3 层
- 受保护命令列表，防止别名冲突

### src/codex/ -- Codex Integration

Codex 集成子系统，是代码量最大的模块。

#### app_server/ -- 长驻 JSON-RPC 子进程

| 文件           | 行数/大小 | 职责                           |
| -------------- | --------- | ------------------------------ |
| supervisor.rs  | --        | 子进程生命周期管理             |
| client.rs      | --        | JSON-RPC 请求/响应             |
| transport.rs   | --        | tokio stdio 流传输             |
| session.rs     | ~46KB     | 每轮次会话包装器（最大文件）   |
| protocol.rs    | ~31KB     | 手动复制的协议类型定义         |
| approvals.rs   | --        | 审批请求路由                   |
| events.rs      | --        | 事件流解析                     |

#### 其他 codex/ 文件

| 文件              | 职责                                                       |
| ----------------- | ---------------------------------------------------------- |
| executor.rs       | `ExecutionRequest`/`Result` 类型定义，轮次策略选择         |
| prompt.rs         | 系统提示词构建，包含记忆注入和 qqbot 指令                  |
| output.rs         | 解析 qqbot 代码围栏指令（图片/文件附件）                   |
| runtime.rs        | 读写 Codex config.toml 配置文件和模型列表                  |
| config_snapshot.rs| 从系统 `~/.codex` 引导初始化 `~/.codex-claw/.codex`       |
| compact.rs        | 会话上下文压缩                                             |

### src/qq/ -- QQ Platform Integration

QQ 平台集成模块。

| 文件        | 职责                                                                     |
| ----------- | ------------------------------------------------------------------------ |
| api.rs      | REST 客户端，令牌缓存，分块文件上传（>5MB），消息序列号追踪             |
| gateway.rs  | WebSocket 连接，心跳维护，指数退避重连                                   |
| passive.rs  | `PassiveTurnEmitter` -- 将 Codex 事件流式推送到 QQ（批量工具调用，500 字符长消息拆分） |
| types.rs    | QQ Gateway 协议类型定义                                                  |

### src/session/ -- Session State

会话状态模块。

| 文件      | 职责                                                                                 |
| --------- | ------------------------------------------------------------------------------------ |
| state.rs  | `SessionSettings`、`ReasoningEffort`、`ContextMode`、`ApprovalPolicy`、`TokenUsageSnapshot` 等类型 |
| store.rs  | 磁盘持久化的按用户状态存储，文件锁，前台/后台会话管理，工作区目录                   |

### src/scheduler/ -- Cron Scheduling

定时调度模块。

| 文件            | 职责                                             |
| --------------- | ------------------------------------------------ |
| mod.rs          | 主循环，信号量并发控制                           |
| store.rs        | `CronJob` 模式定义，磁盘持久化，任务目录管理    |
| runner.rs       | 任务执行（提醒/codex-turn/codex-exec/shell）     |
| cli.rs          | CLI 子命令                                       |
| cron_expr.rs    | Cron 表达式解析，时区支持                        |
| interactive.rs  | 多轮次调度任务，劫持前台会话                     |

### src/memory/ -- User Memory

用户记忆模块。

| 文件      | 职责                                                   |
| --------- | ------------------------------------------------------ |
| store.rs  | 按用户的 `MEMORY.md` 和 `USER.md` 文件，`§` 分隔条目  |
| inject.rs | 格式化记忆块用于提示词注入                             |
| scan.rs   | 记忆条目扫描与过滤                                     |

### src/shadow/ -- Background Distillation

后台蒸馏模块。

| 文件      | 职责                                     |
| --------- | ---------------------------------------- |
| mod.rs    | `ShadowWorker`，按 openid 的 FIFO 去重  |
| memory.rs | 从对话轮次中提取事实和洞察               |
| skill.rs  | 从修改的文件中提取技能信息               |
| prompt.rs | 记忆/技能合成的提示词                    |
| runner.rs | 一次性 Codex 调用，用于 shadow 任务      |

### src/skills/ -- Skill Discovery

技能发现模块。

| 文件      | 职责                            |
| --------- | ------------------------------- |
| index.rs  | 文件系统技能扫描，带缓存       |
| writer.rs | 写入 `SKILL.md` 文件           |

### Other Files

| 文件              | 职责                                               |
| ----------------- | -------------------------------------------------- |
| src/config.rs     | TOML 配置文件加载与校验                            |
| src/message.rs    | `IncomingMessage` 类型（文本、图片、文件、引用、@） |
| src/self_update.rs| 二进制自更新（编译 -> 替换 -> 退出）               |
| src/lib.rs        | 模块映射与 i18n 初始化（`rust_i18n` 宏）           |

---

## Key Data Flows

### 1. Normal Conversation Turn -- 正常对话轮次

```
QQ WebSocket 事件
  -> gateway.rs 分发 C2CMessageEvent
  -> App.handle_c2c_event()
  -> 下载/缓存附件
  -> commands.rs: 检查是否为斜杠命令
    -> 如果是命令: 处理并返回回复
    -> 如果不是: run_normal_message()
  -> 获取忙碌锁 (AtomicBool)
  -> 设置 active_openid 用于审批路由
  -> 构建提示词 (会话状态 + 记忆注入)
  -> CodexExecutor.execute(ExecutionRequest)
    -> AppServerSession -> codex app-server (JSON-RPC)
  -> PassiveTurnEmitter 流式推送更新到 QQ
  -> 解析输出中的 qqbot 指令 (图片/文件)
  -> 通过 QQ API 发送指令附件
  -> ShadowWorker 异步启动后台记忆/技能提取
  -> 释放忙碌锁
```

### 2. Approval Flow -- 审批流程

```
codex app-server -> 审批通知 (JSON-RPC)
  -> ApprovalBroker 接收
  -> 入队到 App.pending_approvals[openid]
  -> 向 QQ 用户发送审批请求消息
  -> 用户发送 /approve, /deny, 或 /cancel
  -> App.resolve_pending_approval() 查找最早的待处理审批
  -> 通过 oneshot channel 发送 ApprovalOutcome
  -> app-server 继续执行或取消轮次
```

### 3. Scheduler -- 调度器

```
Scheduler.tick() 每隔 tick_secs (默认 30 秒) 执行一次
  -> 扫描所有任务，查找到期的 (next_run_at <= now 或 run_now_at 已设置)
  -> 获取信号量许可 (max_concurrent_jobs)
  -> runner.run_job() 按动作类型分发
    -> Reminder: 通过 QQ API 发送消息
    -> CodexTurn: 作为合成消息通过 App 分发
    -> Shell: 启动子进程
  -> 成功时: 更新 run_count, last_run_status, 写入运行日志
  -> 失败时: 递增 failure_streak, 带退避重试
  -> 达到 circuit_breaker_threshold: 自动禁用并通知任务所有者
  -> 一次性任务: 回收到 cron-jobs-trash/
```

---

## Async Patterns

CodexClaw 基于 tokio 多线程运行时构建，使用以下异步模式：

| 模式                          | 用途                                        |
| ----------------------------- | ------------------------------------------- |
| `Arc<App>`                    | 在所有 spawned task 之间共享应用状态        |
| `AtomicBool`                  | 忙碌标志（快速，预期无竞争）                |
| `tokio::sync::Mutex`         | 按用户状态（active_turn, pending_approvals）|
| `tokio::sync::RwLock`        | Gateway 会话状态                            |
| `tokio::sync::Semaphore`     | 调度器并发控制                              |
| `oneshot` channel             | 审批决议和轮次取消                          |
| `mpsc::unbounded_channel`    | 执行更新流式推送                            |
| `Weak<App>`                   | Scheduler 中避免引用循环                    |
| `fs2` 文件锁                 | 并发会话状态写入的磁盘同步                  |

---

## Extension Guide

### How to Add a New Command -- 添加新命令

1. 在 `src/commands.rs` 的 `PROTECTED_COMMANDS` 中添加命令字符串。
2. 在 `commands.rs` 底部的 `canonicalize_core_command()` 中添加中文别名。
3. 在主分发块（约第 245 行）中添加 match 分支。
4. 实现处理函数（async，接收 openid + session + 其他状态参数）。
5. 在 `locales/en.yml` 和 `locales/zh.yml` 中添加语言键。
6. 在两个语言文件的 `commands.help` 下添加帮助条目。

### How to Add a New Module -- 添加新模块

1. 创建 `src/<module>/mod.rs`。
2. 在 `src/lib.rs` 中添加 `pub mod <module>;`。
3. 如需要，将模块接入 `App` 结构体（参考 session/memory/shadow 的模式）。
4. 在每个文件中使用 `#[cfg(test)] mod tests` 添加单元测试。
5. 如有需要，在 `tests/` 目录下添加集成测试。

---

## English Summary

CodexClaw is a ~24,000-line Rust async application that bridges QQ (China's major messaging platform) to OpenAI's Codex CLI through a long-lived app-server child process communicating over JSON-RPC via stdio.

**Core components:**

- **gateway.rs** -- Maintains a WebSocket connection to Tencent's QQ Gateway, handling heartbeats and reconnection with exponential backoff.
- **app.rs** -- Central dispatcher (~1,656 lines) that coordinates the entire message processing pipeline: event normalization, attachment download, command parsing, Codex execution, approval brokering, and self-update detection. Uses an atomic busy flag to serialize turns globally.
- **commands.rs** -- Parses slash commands with Chinese alias canonicalization, supports alias expansion (3-level recursion limit), and maintains a protected command list to prevent collisions.
- **codex/** -- The largest subsystem. Contains a JSON-RPC client for the long-lived codex app-server process (`supervisor.rs`, `client.rs`, `transport.rs`), per-turn session management (`session.rs`, ~46KB), hand-copied protocol types (`protocol.rs`, ~31KB), approval routing, prompt construction with memory injection, and output parsing for qqbot directives.
- **qq/** -- QQ platform integration: REST API client with token caching and chunked file upload, WebSocket gateway, and `PassiveTurnEmitter` that streams Codex events to QQ users (batching tool calls, splitting messages at 500 characters).
- **session/** -- Disk-backed per-user session state with file locking, supporting foreground/background session management and workspace directories.
- **scheduler/** -- Cron scheduling system with semaphore-based concurrency, support for reminder/codex-turn/codex-exec/shell job types, failure streak tracking with circuit breaker auto-disable, and one-shot job recycling.
- **memory/** -- Per-user memory files (`MEMORY.md`, `USER.md`) with section-delimited entries, injected into prompts for personalized context.
- **shadow/** -- Background distillation worker that extracts facts, insights, and skill information from conversation turns and modified files via one-shot Codex invocations.
- **skills/** -- Filesystem-based skill discovery with caching and `SKILL.md` generation.

**Async architecture:** Built on the tokio multi-threaded runtime. Uses `Arc<App>` for shared state, `AtomicBool` for the busy flag, `tokio::sync::Mutex` for per-user state, `RwLock` for gateway sessions, `Semaphore` for scheduler concurrency, `oneshot` channels for approval resolution, `mpsc::unbounded_channel` for execution event streaming, `Weak<App>` to break reference cycles in the scheduler, and `fs2` file locking for safe concurrent disk writes.
