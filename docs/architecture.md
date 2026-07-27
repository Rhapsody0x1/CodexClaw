# CodexClaw 系统架构

> CodexClaw 系统架构文档

*Read this in: [English](architecture_en.md) | [中文](architecture.md)*

---

## 目录

1. [概览](#概览)
2. [系统架构图](#系统架构图)
3. [模块职责](#模块职责)
4. [关键数据流](#关键数据流)
5. [异步模式](#异步模式)
6. [扩展指南](#扩展指南)

---

## 概览

CodexClaw 是一个 Rust 异步应用，通过长驻的 app-server 子进程将 QQ（中国主流即时通讯平台）桥接到 OpenAI Codex CLI。系统提供以下核心能力：

| 能力               | 说明                                              |
| ------------------ | ------------------------------------------------- |
| 会话管理           | 按用户维护独立会话状态，支持前台/后台会话切换     |
| 后台记忆蒸馏       | 自动从对话轮次中提取事实和洞察                     |
| 定时调度           | 类 cron 调度系统，支持提醒、Codex 任务、Shell 命令 |
| 自我更新           | 当 Codex 修改了源码文件时，自动检测并触发重编译   |

---

## 系统架构图

```
┌─────────────────┐       WebSocket        ┌──────────────┐
│  QQ Gateway     │ <---------------------> │  gateway.rs  │
│  (Tencent API)  │                         └──────┬───────┘
└─────────────────┘                                │ C2CMessageEvent
                                                   v
┌─────────────────┐  HTTP REST   ┌──────────────────────────────────┐
│  QQ API Server  │ <----------> │              App                 │
│  (send msgs)    │              │  ┌──────────┐ ┌──────────────┐   │
└─────────────────┘              │  │commands/ │ │ PassiveTurn  │   │
                                 │  │dispatch  │ │   Emitter    │   │
                                 │  └──────────┘ └──────────────┘   │
                                 └────┬──────┬──────┬──────┬────────┘
                                      │      │      │      │
                        ┌─────────────┘      │      │      └──────────────┐
                        v                    v      v                     v
                 ┌──────────────┐  ┌─────────────┐ ┌──────────────┐ ┌─────────┐
                 │ SessionStore │  │CodexExecutor│ │  Scheduler   │ │ Shadow  │
                 │ (session/)   │  │ (codex/)    │ │ (scheduler/) │ │ Worker  │
                 └──────────────┘  └──────┬──────┘ └──────────────┘ └─────────┘
                                          │ JSON-RPC (stdio)
                                          v
                                 ┌──────────────────┐
                                 │ codex app-server  │
                                 │ (child process)   │
                                 └──────────────────┘
```

**数据流概述：**

- QQ Gateway 通过 WebSocket 将消息事件推送到 `qq/gateway.rs`。
- `App`（`app/mod.rs`）作为中央调度器，协调命令解析、Codex 执行、审批流程和消息发送。
- `CodexExecutor`（`codex/executor.rs`）通过 JSON-RPC（stdio 管道）与长驻的 codex app-server 子进程通信。
- `Scheduler` 独立运行定时任务，通过 `ProactiveNotifier` trait 发送合成消息。
- `ShadowWorker`（`shadow/mod.rs`）在后台异步提取记忆。

---

## 模块职责

模块按依赖关系从底层到上层组织。L0 为叶子模块（无内部依赖），L4 为组合中枢（依赖所有其他模块）。

### 依赖 DAG

```
util, model  (L0 叶子)
    ↓
config, memory  (L2)
    ↓
codex, qq, session, self_update  (L2)
    ↓
commands, shadow, scheduler  (L3)
    ↓
app  (L4 中枢)
    ↓
main.rs  (入口)
```

### L0: src/util/ -- 叶子工具

无 crate 内部依赖的纯工具函数集合。

| 文件      | 职责                                             |
| --------- | ------------------------------------------------ |
| fs.rs     | 原子写入、可选读取、目录遍历                     |
| layout.rs | `DataLayout`：规范化磁盘目录结构定义             |
| path.rs   | Home 目录解析、PATH 搜索                         |
| lang.rs   | 语言标签规范化（zh/en）                          |
| text.rs   | 文本截断、JSON 提取、工具标签、token 数量展示    |
| time.rs   | RFC3339 解析、相对时间格式化（支持 locale + 时区）|

### L0: src/model/ -- 纯值类型

无 I/O、无服务的纯数据类型定义。

| 文件          | 职责                                                                 |
| ------------- | -------------------------------------------------------------------- |
| message.rs    | `IncomingMessage` 类型（文本、图片、文件、引用、@提及）              |
| settings.rs   | `SessionSettings`、`DialogState`、`DialogProfile`、`ReasoningEffort`、`ContextMode`、`ServiceTier`、`ApprovalPolicySetting`、`UserSessionState`、`TokenUsageSnapshot`、`CommandAlias`、`PendingSetting` |
| cron.rs       | `CronJob`、`CronKind`、`JobAction`、`DeliverPolicy`、`RunStatus`、`SessionStrategy`、`InteractiveSpec` |
| wire_compat.rs| Golden serde 快照测试，保护 `state.json`/`jobs.json` 格式兼容性     |

### L2: src/config.rs -- 配置

TOML 配置文件（`AppConfig`）加载、路径规范化和校验。

### L2: src/memory/ -- 用户记忆

持久化按用户记忆文件的管理。

| 文件      | 职责                                             |
| --------- | ------------------------------------------------ |
| store.rs  | `MemoryStore`：快照获取、追加写入、条目去重       |
| inject.rs | 将记忆快照渲染为 prompt 注入块                    |
| scan.rs   | 记忆内容安全扫描                                 |

### L2: src/codex/ -- Codex CLI 集成

Codex 集成子系统。

#### codex/app_server/ -- 长驻 JSON-RPC 子进程

| 文件           | 职责                                                    |
| -------------- | ------------------------------------------------------- |
| protocol.rs    | 手动复制的 JSON-RPC 协议类型定义                        |
| transport.rs   | `StdioTransport`：spawn 子进程，读写 NDJSON 行           |
| client.rs      | `JsonRpcClient`：transport 之上的类型化 JSON-RPC 客户端  |
| approvals.rs   | 路由服务端发起的审批/澄清请求                           |
| events.rs      | 将 app-server 通知转换为 `ExecutionUpdate` 流            |
| session.rs     | `AppServerSession`：通过 thread/start + turn/start 驱动一个轮次 |
| supervisor.rs  | 进程生命周期：文件锁、自动重生、原子化 client 替换      |

#### 其他 codex/ 文件

| 文件                | 职责                                                       |
| ------------------- | ---------------------------------------------------------- |
| executor.rs         | `CodexExecutor` facade，封装 app-server 会话               |
| types.rs            | `ExecutionRequest`、`ExecutionResult`、`ExecutionUpdate`、`CompactRequest` |
| display.rs          | 轮次中流式工具块的人类可读单行摘要                         |
| events.rs           | `codex exec --json` 的 NDJSON 线格式反序列化               |
| exec_cli.rs         | 共享的一次性 `codex exec --json` 子进程运行器               |
| exec_output.rs      | 解析 codex exec NDJSON 输出为 agent 消息文本                |
| runtime.rs          | `CodexRuntimeProfile`：读写 codex config.toml               |
| prompt.rs           | 组装完整轮次 prompt（系统 prompt + 记忆注入 + qqbot 指令）  |
| config_snapshot.rs  | 从系统 `~/.codex` 引导初始化 `~/.codex-claw/.codex`        |

### L2: src/qq/ -- QQ 平台集成

QQ Bot 平台集成模块。

| 文件        | 职责                                                                     |
| ----------- | ------------------------------------------------------------------------ |
| types.rs    | Gateway 线格式类型（`C2CMessageEvent`、`GatewayEnvelope`）               |
| api.rs      | `QqApiClient`：HTTP+WebSocket API 客户端，令牌缓存，分块文件上传（>5MB） |
| gateway.rs  | `spawn_gateway`：WebSocket 连接、identify、心跳、指数退避重连            |
| directive.rs| 解析 \`\`\`qqbot 围栏协议中的图片/文件附件指令                          |
| render.rs   | `PassiveTurnEmitter`：将 `ExecutionUpdate` 流式推送到 QQ 消息            |

### L2: src/session/ -- 会话状态

会话状态管理模块。

| 文件        | 职责                                                                                 |
| ----------- | ------------------------------------------------------------------------------------ |
| dialogs.rs  | 纯状态机：对话拓扑（前台/后台转换），别名粘性，CAS 绑定与代数计数器                 |
| store.rs    | `SessionStore`：锁、磁盘持久化、工作区生命周期                                       |
| rollout.rs  | 扫描 codex-home sessions/ 目录树，解析 rollout .jsonl 文件                           |
| jobs_file.rs| 调度器 `jobs.json` 的建议锁持久化                                                    |
| state.rs    | 对 `model::settings` 类型的重导出 shim                                               |

### L3: src/commands/ -- 斜杠命令分发

命令解析与分发模块（纯决策层）。

| 文件            | 职责                                                       |
| --------------- | ---------------------------------------------------------- |
| mod.rs          | `Dispatcher`：`maybe_handle_command`、`CmdCtx`、命令规范化 |
| alias.rs        | 命令别名解析、保护和展开（递归深度限制 3 层）              |
| cron_cmds.rs    | `/cron` 系列命令处理                                       |
| interactive.rs  | 多步交互式选择器（模型、推理等级等）                       |
| listing.rs      | 共享列表渲染（会话、模型、cron 任务、状态）               |
| session_cmds.rs | `/new`、`/bg`、`/fg`、`/stop`、`/save`、`/rename`、`/resume`、`/loadbg`、`/import`、`/sessions` |
| settings_cmds.rs| `/model`、`/lang`、`/fast`、`/context`、`/reasoning`、`/verbose`、`/approvals` |
| tests.rs        | 命令分发的集成测试                                         |

### L3: src/scheduler/ -- 定时调度

定时任务调度模块。

| 文件            | 职责                                                         |
| --------------- | ------------------------------------------------------------ |
| mod.rs          | 模块声明 + 重导出                                           |
| loop_.rs        | `Scheduler`：周期性 tick 循环（默认 30s），飞行中任务集合，并发信号量 |
| ctx.rs          | `SchedulerCtx` + `ProactiveNotifier` trait                   |
| store.rs        | 任务目录布局，Toml 持久化，待投递队列                       |
| runner.rs       | 执行单个 cron 任务，写入运行日志                             |
| interactive.rs  | 交互式任务流程                                               |
| cron_expr.rs    | `next_after`：根据 cron 表达式 + 时区计算下次触发时间        |
| cli.rs          | 独立的 `codex-claw cron` CLI 子命令                          |

### L3: src/shadow/ -- 后台记忆蒸馏

后台蒸馏模块。

| 文件      | 职责                                                   |
| --------- | ------------------------------------------------------ |
| memory.rs | `ShadowContext`、阈值检查、JSON 响应解析               |
| prompt.rs | 蒸馏 prompt 模板                                       |
| runner.rs | 运行一次性 codex exec，严格 success-only 契约          |

### L3: src/self_update.rs -- 自我更新

二进制自更新：构建、冒烟测试、原子替换。

### L4: src/app/ -- 组合中枢

依赖所有模块的组合层。

| 文件        | 职责                                                                 |
| ----------- | -------------------------------------------------------------------- |
| mod.rs      | `App` struct、`BusyGuard` RAII、`ProactiveNotifier` impl              |
| turn.rs     | 完整轮次生命周期：获取 busy → 注入记忆 → 构建 prompt → 执行 → 渲染被动回复 → shadow 蒸馏 → 中断/取消 |
| inbound.rs  | 规范化入站 QQ 事件，下载附件，提取引用，分发命令结果，渲染 `DialogError` |
| approvals.rs| 将服务端发起的审批请求路由到 QQ 用户                                 |
| format.rs   | 纯格式化：plan 块、token 快照、上下文警告                            |

### src/main.rs -- 入口

程序入口。解析 CLI 参数（路由到 `cron` 子命令或 Bot 主服务），加载 `AppConfig`，创建 `SessionStore`，spawn codex app-server 子进程，创建 `App`，启动 `Scheduler`，spawn QQ gateway，服务 C2C 事件。

```
加载配置 → 创建 SessionStore → spawn app-server → 创建 App
  → 启动 Scheduler → spawn QQ gateway → 服务 C2C 事件
```

### src/lib.rs -- crate 根

声明 12 个模块，重导出 `model::message` 到 crate root（向后兼容）和 `util::layout::DataLayout`（供 main.rs 使用）。初始化 `rust_i18n` 宏。

---

## 关键数据流

### 1. 对话轮次

```
QQ WebSocket 事件
  → qq/gateway.rs 分发 C2CMessageEvent
  → App.handle_c2c_event()
  → app/inbound.rs: 规范化事件、下载附件、提取引用
  → commands/mod.rs: maybe_handle_command 分发
    → 如果是命令: 处理并返回 CommandReply
    → 如果是 Continue: 继续执行轮次
  → 获取 BusyGuard (AtomicBool)
  → memory/inject.rs: 注入记忆到 prompt
  → codex/prompt.rs: 构建完整轮次 prompt
  → codex/app_server/session.rs: turn/start (JSON-RPC)
  → codex/app_server/events.rs: 流式 ExecutionUpdate
  → qq/render.rs: PassiveTurnEmitter 推送到 QQ
  → 解析 qqbot 指令 (image/file)
  → 通过 QQ API 发送指令附件
  → shadow/runner.rs: 异步启动后台记忆蒸馏
  → session/store.rs: bind_turn_result 到 SessionStore
  → BusyGuard 释放
```

### 2. 审批流程

```
codex app-server → 审批通知 (JSON-RPC)
  → codex/app_server/approvals.rs 接收
  → 入队到 App.pending_approvals[openid]
  → 向 QQ 用户发送审批请求消息
  → 用户发送 /approve 或 /deny
  → app/approvals.rs: 查找最早待处理审批
  → 通过 oneshot channel 发送 ApprovalOutcome
  → app-server 继续执行或取消轮次
```

### 3. 调度器

```
Scheduler loop (tick_secs, 默认 30s)
  → 扫描所有任务，查找到期的 (next_run_at <= now)
  → 获取信号量许可 (max_concurrent_jobs)
  → scheduler/runner.rs: 按 JobAction 分发
    → Reminder: 通过 QQ API 发送消息
    → CodexTurn: 通过 App 作为合成消息分发
    → Shell: 启动子进程
    → Interactive: 劫持前台会话
  → 成功时: 更新 run_count、last_run_status、写入运行日志
  → 失败时: 递增 failure_streak、带退避重试
  → 达到 circuit_breaker_threshold: 自动禁用并通知任务所有者
  → 一次性任务: 回收至 cron-jobs-trash/
```

### 4. 对话状态机

```
foreground ↔ background 转换
  /bg: 将前台对话移入后台映射，分配或保留别名
  /fg: 从后台映射恢复对话到前台，别名随之迁移
  /new: 将当前前台归档，创建新前台对话
  /stop: 终止当前前台对话（清理工作区）
  
别名粘性：对话在 /fg 和 /bg 之间保持用户给定的别名
CAS 绑定：代数计数器防止轮次中途的对话切换损坏错误对话
```

---

## 异步模式

CodexClaw 基于 tokio 多线程运行时构建，使用以下异步模式：

| 模式                          | 用途                                        |
| ----------------------------- | ------------------------------------------- |
| `Arc<App>`                    | 在 gateway 和 scheduler 任务间共享应用状态  |
| `AtomicBool`                  | 全局单轮次忙碌标志（BusyGuard RAII）        |
| `tokio::sync::Mutex`         | pending_approvals、pending_settings、resume_messages |
| `tokio::sync::RwLock`        | `PersistedSessionState` 会话状态            |
| `tokio::sync::Semaphore`     | 调度器任务并发控制                          |
| `oneshot` channel             | 审批决议                                   |
| `mpsc::unbounded_channel`    | gateway → App C2C 事件流转                  |
| `Weak<SchedulerCtx>`          | 调度器 → App 链路，避免引用循环             |
| `fs2` 文件锁                 | 会话状态写入的磁盘同步                      |

---

## 扩展指南

### 添加新命令

1. 在 `src/commands/alias.rs` 的受保护命令列表中添加命令字符串。
2. 在 `src/commands/mod.rs` 的 `canonicalize_core_command()` 中添加中文别名。
3. 在对应的命令文件（`session_cmds.rs`、`settings_cmds.rs` 或 `cron_cmds.rs`）中实现处理函数。
4. 在 `src/commands/mod.rs` 的主分发 `maybe_handle_command()` 中添加 match 分支。
5. 处理函数返回 `CommandOutcome`，由 `app/inbound.rs` 统一处理。
6. 在 `locales/en.yml` 和 `locales/zh.yml` 中添加语言键。
7. 在两个语言文件的 `commands.help` 下添加帮助条目。

### 添加新模块

1. 创建 `src/<module>/mod.rs`（及其子文件）。
2. 在 `src/lib.rs` 中添加 `pub mod <module>;`。
3. 确定模块在依赖 DAG 中的层级，避免循环依赖。
4. 如需引入状态，通过 `Arc` 传入 `App` 结构体（参考 session/memory/shadow 的模式）。
5. 在每个文件中使用 `#[cfg(test)] mod tests` 添加单元测试。
6. 使用 `rust-i18n` 的 `t!` 宏进行国际化，更新两个语言文件。
