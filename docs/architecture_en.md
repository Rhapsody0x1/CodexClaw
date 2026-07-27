# CodexClaw System Architecture

> CodexClaw System Architecture Document

*Read this in: [English](architecture_en.md) | [中文](architecture.md)*

---

## Table of Contents

1. [Overview](#overview)
2. [System Diagram](#system-diagram)
3. [Module Responsibilities](#module-responsibilities)
4. [Key Data Flows](#key-data-flows)
5. [Async Patterns](#async-patterns)
6. [Extension Guide](#extension-guide)

---

## Overview

CodexClaw is a Rust async application that bridges QQ (China's major messaging platform) to OpenAI Codex CLI through a long-lived app-server child process. The system provides the following core capabilities:

| Capability                      | Description                                                                                      |
| ------------------------------- | ------------------------------------------------------------------------------------------------ |
| Session Management              | Maintains independent session state per user, supports foreground/background session switching    |
| Background Memory Distillation  | Automatically extracts facts and insights from conversation turns                                |
| Scheduled Tasks                 | Cron-like scheduling system supporting reminders, Codex tasks, and shell commands                 |
| Self-Update                     | Automatically detects source file modifications made by Codex and triggers recompilation          |

---

## System Diagram

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

**Data Flow Overview:**

- QQ Gateway pushes message events to `qq/gateway.rs` via WebSocket.
- `App` (`app/mod.rs`) serves as the central dispatcher, coordinating command parsing, Codex execution, approval flows, and message sending.
- `CodexExecutor` (`codex/executor.rs`) communicates with the long-lived codex app-server child process via JSON-RPC (stdio pipes).
- `Scheduler` runs scheduled tasks independently and sends proactive notifications through the `ProactiveNotifier` trait.
- `ShadowWorker` (`shadow/mod.rs`) asynchronously extracts memories in the background.

---

## Module Responsibilities

Modules are organized from bottom to top by dependency. L0 contains leaf modules with no runtime crate-internal dependencies; L5 is the composition hub.

### Dependency DAG

```
util, model  (L0 leaves)
    ↓
config, memory  (L1)
    ↓
codex, session, self_update  (L2)
    ↓
qq, scheduler, shadow  (L3)
    ↓
commands  (L4)
    ↓
app  (L5 hub)
    ↓
main.rs  (entry point)
```

### L0: src/util/ -- Leaf Utilities

Pure utility functions with no crate-internal dependencies.

| File      | Responsibility                                                    |
| --------- | ----------------------------------------------------------------- |
| fs.rs     | Atomic writes, optional reads, directory walks                    |
| layout.rs | `DataLayout`: canonical on-disk directory structure definition    |
| path.rs   | Home directory resolution, PATH search                            |
| lang.rs   | Language tag normalization (zh/en)                                |
| text.rs   | Text truncation, JSON extraction, tool labels, token display      |
| time.rs   | RFC3339 parsing, relative-time formatting (locale + timezone aware)|

### L0: src/model/ -- Pure Value Types

Pure data types with no I/O and no service dependencies.

| File          | Responsibility                                                                              |
| ------------- | ------------------------------------------------------------------------------------------- |
| message.rs    | `IncomingMessage` type (text, image, file, quote, @mention)                                 |
| settings.rs   | `SessionSettings`, `DialogState`, `DialogProfile`, `ReasoningEffort`, `ContextMode`, `ServiceTier`, `ApprovalPolicySetting`, `UserSessionState`, `TokenUsageSnapshot`, `CommandAlias`, `PendingSetting` |
| cron.rs       | `CronJob`, `CronKind`, `JobAction`, `DeliverPolicy`, `RunStatus`, `SessionStrategy`, `InteractiveSpec` |
| wire_compat.rs| Golden serde snapshot tests protecting `state.json`/`jobs.json` format compatibility        |

### L1: src/config.rs -- Configuration

TOML configuration file (`AppConfig`) loading, path normalization, and validation.

### L1: src/memory/ -- User Memory

Management of persistent per-user memory files.

| File      | Responsibility                                             |
| --------- | ---------------------------------------------------------- |
| store.rs  | `MemoryStore`: snapshot retrieval, append with dedup        |
| inject.rs | Renders memory snapshots into prompt injection blocks       |
| scan.rs   | Safety scanning of memory content                           |

### L2: src/codex/ -- Codex CLI Integration

The Codex integration subsystem.

#### codex/app_server/ -- Long-lived JSON-RPC Child Process

| File           | Responsibility                                                   |
| -------------- | ---------------------------------------------------------------- |
| protocol.rs    | Hand-copied JSON-RPC protocol type definitions                   |
| transport.rs   | `StdioTransport`: spawns child, reads/writes NDJSON lines        |
| client.rs      | `JsonRpcClient`: typed JSON-RPC client over transport             |
| approvals.rs   | Routes server-initiated approval/elicitation requests            |
| events.rs      | Translates app-server notifications into `ExecutionUpdate` stream|
| session.rs     | `AppServerSession`: drives a turn via thread/start + turn/start  |
| supervisor.rs  | Process lifecycle: file-lock, auto-respawn, atomic client swap   |

#### Other codex/ Files

| File                | Responsibility                                                              |
| ------------------- | --------------------------------------------------------------------------- |
| executor.rs         | `CodexExecutor` facade over the app-server session                          |
| types.rs            | `ExecutionRequest`, `ExecutionResult`, `ExecutionUpdate`, `CompactRequest`  |
| display.rs          | Human-readable one-liners for tool blocks streamed during turns             |
| events.rs           | NDJSON wire-format deserialization for `codex exec --json`                  |
| exec_cli.rs         | Shared one-shot `codex exec --json` subprocess runner                       |
| exec_output.rs      | Parses codex exec NDJSON output into agent message text                     |
| runtime.rs          | `CodexRuntimeProfile`: reads/writes codex config.toml                       |
| prompt.rs           | Assembles the full turn prompt (system prompt + memory injection + qqbot instructions) |
| config_snapshot.rs  | Bootstraps `~/.codex-claw/.codex` from the system `~/.codex`               |

### L3: src/qq/ -- QQ Platform Integration

QQ Bot platform integration module.

| File        | Responsibility                                                                                           |
| ----------- | -------------------------------------------------------------------------------------------------------- |
| types.rs    | Gateway wire types (`C2CMessageEvent`, `GatewayEnvelope`)                                                |
| api.rs      | `QqApiClient`: HTTP+WebSocket API client, token caching, chunked file upload (>5MB)                      |
| gateway.rs  | `spawn_gateway`: WebSocket connect, identify, heartbeat, exponential backoff reconnection                 |
| directive.rs| Parses \`\`\`qqbot fence protocol for image/file attachment directives                                  |
| render.rs   | `PassiveTurnEmitter`: streams `ExecutionUpdate` into QQ messages                                         |

### L2: src/session/ -- Session State

Session state management module.

| File        | Responsibility                                                                                          |
| ----------- | ------------------------------------------------------------------------------------------------------- |
| dialogs.rs  | Pure state machine: dialog topology (foreground/background transitions), sticky aliases, CAS binding with generation counter |
| store.rs    | `SessionStore`: locking, disk persistence, workspace lifecycle                                          |
| rollout.rs  | Scans codex-home sessions/ tree, parses rollout .jsonl files                                            |
| jobs_file.rs| Advisory-locked persistence for scheduler `jobs.json`                                                   |
| state.rs    | Re-export shim for `model::settings` types                                                              |

### L4: src/commands/ -- Slash Command Dispatch

Command parsing and orchestration module. Handlers return `CommandOutcome` and may persist through `SessionStore` or use scheduler/filesystem helpers; `app/` owns the outer QQ and global-config effects.

| File            | Responsibility                                                              |
| --------------- | --------------------------------------------------------------------------- |
| mod.rs          | `Dispatcher`: `maybe_handle_command`, `CmdCtx`, command canonicalization    |
| alias.rs        | Command alias resolution, protection, and expansion (3-level recursion limit)|
| cron_cmds.rs    | `/cron` family of command handlers                                          |
| interactive.rs  | Multi-step interactive pickers (model, reasoning effort, etc.)              |
| listing.rs      | Shared list rendering (sessions, models, cron jobs, status)                 |
| session_cmds.rs | `/new`, `/bg`, `/fg`, `/stop`, `/save`, `/rename`, `/resume`, `/loadbg`, `/import`, `/sessions` |
| settings_cmds.rs| `/model`, `/lang`, `/fast`, `/context`, `/reasoning`, `/verbose`, `/approvals` |
| tests.rs        | Integration tests for command dispatch                                      |

### L3: src/scheduler/ -- Cron Scheduling

Scheduled task module.

| File            | Responsibility                                                         |
| --------------- | ---------------------------------------------------------------------- |
| mod.rs          | Module declarations + re-exports                                      |
| loop_.rs        | `Scheduler`: periodic tick loop (default 30s), in-flight set, concurrency semaphore |
| ctx.rs          | `SchedulerCtx` + `ProactiveNotifier` trait                            |
| store.rs        | Job directory layout, Toml persistence, pending-delivery queue        |
| runner.rs       | Runs a single cron job, writes run logs                               |
| interactive.rs  | Interactive job flows                                                  |
| cron_expr.rs    | `next_after`: computes next fire time from cron expression + timezone  |
| cli.rs          | Standalone `codex-claw cron` CLI subcommand                            |

### L3: src/shadow/ -- Background Memory Distillation

Background distillation module.

| File      | Responsibility                                                   |
| --------- | ---------------------------------------------------------------- |
| memory.rs | `ShadowContext`, threshold check, JSON response parsing          |
| prompt.rs | Distillation prompt template                                     |
| runner.rs | Runs one-shot codex exec with strict success-only contract       |

### L2: src/self_update.rs -- Self-Update

Binary self-update: build, stage, back up, and atomically replace.

### L5: src/app/ -- Composition Hub

The composition layer that depends on all other modules.

| File        | Responsibility                                                                              |
| ----------- | ------------------------------------------------------------------------------------------- |
| mod.rs      | `App` struct, `BusyGuard` RAII, `ProactiveNotifier` impl                                    |
| turn.rs     | Turn execution and finalization: memory inject → prompt build → execute with streamed rendering → bind result → deliver remaining replies/directives → shadow distillation, including success, failure, and interruption outcomes |
| inbound.rs  | Normalizes incoming QQ events, acquires the busy guard, downloads attachments, extracts quotes, dispatches command outcomes, and renders `DialogError` |
| approvals.rs| Routes server-initiated approval requests to QQ users                                       |
| format.rs   | Pure formatting: plan blocks, token snapshots, context warnings                             |

### src/main.rs -- Entry Point

The program entry point. Parses CLI arguments (routing to the `cron` subcommand or Bot main service), loads `AppConfig`, creates `SessionStore`, spawns the codex app-server child process, creates `App`, starts the `Scheduler`, spawns the QQ gateway, and services C2C events.

```
Load config → Create SessionStore → spawn app-server → create App
  → start Scheduler → spawn QQ gateway → service C2C events
```

### src/lib.rs -- Crate Root

Declares 12 modules, re-exports `model::message` to the crate root (backward compatibility) and `util::layout::DataLayout` (for main.rs). Initializes the `rust_i18n` macro.

---

## Key Data Flows

### 1. Conversation Turn

```
QQ WebSocket event
  → qq/gateway.rs dispatches C2CMessageEvent
  → App.handle_c2c_event()
  → app/inbound.rs: normalize event, download attachments, extract quotes
  → commands/mod.rs: maybe_handle_command dispatch
    → If command: process and return CommandReply
    → If Continue: proceed with turn execution
  → Acquire BusyGuard (AtomicBool)
  → memory/inject.rs: inject memory into prompt
  → codex/prompt.rs: build full turn prompt
  → codex/app_server/session.rs: turn/start (JSON-RPC)
  → codex/app_server/events.rs: stream ExecutionUpdate
    → qq/render.rs: PassiveTurnEmitter pushes to QQ
  → session/store.rs: bind_turn_result to SessionStore
  → Deliver text not sent during streaming, then parse and send qqbot directives (image/file)
  → shadow/runner.rs: asynchronously launch background memory distillation
  → BusyGuard released
```

### 2. Approval Flow

```
codex app-server → Approval notification (JSON-RPC)
  → codex/app_server/approvals.rs receives
  → Enqueue to App.pending_approvals[openid]
  → Send approval request message to QQ user
  → User sends /approve or /deny
  → app/approvals.rs: find earliest pending approval
  → Send ApprovalOutcome via oneshot channel
  → app-server continues execution or cancels the turn
```

### 3. Scheduler

```
Scheduler loop (tick_secs, default 30s)
  → Scan all jobs, find those due (next_run_at <= now)
  → Acquire semaphore permit (max_concurrent_jobs)
  → scheduler/runner.rs: dispatch by JobAction
    → Reminder: Send message via QQ API
    → CodexTurn: Call the shared CodexExecutor (app-server) directly
    → CodexExec: Launch a codex exec --json subprocess
    → Shell: Launch subprocess
    → CodexTurn with interactive config: Temporarily take over foreground session
  → On success: Update run_count, last_run_status, write run log
  → On failure: Increment failure_streak, retry with backoff
  → On reaching circuit_breaker_threshold: Auto-disable and notify job owner
  → One-shot jobs: Recycle to cron-jobs-trash/
```

### 4. Dialog State Machine

```
foreground ↔ background transitions
  /bg: Move foreground dialog into background map, assign or retain alias
  /fg: Restore dialog from background map to foreground, alias migrates with it
  /new: Archive current foreground, create new foreground dialog
  /stop: Terminate current foreground dialog (cleanup workspace)

Sticky aliases: A dialog remembers its user-given name across /fg and /bg cycles
CAS binding: Generation counter prevents mid-turn dialog swaps from corrupting the wrong dialog
```

---

## Async Patterns

CodexClaw is built on the tokio multi-threaded runtime and uses the following async patterns:

| Pattern                         | Purpose                                                  |
| ------------------------------- | -------------------------------------------------------- |
| `Arc<App>`                      | Shared application state across C2C handler tasks spawned by the gateway |
| `AtomicBool`                    | Global single-turn busy flag (BusyGuard RAII)            |
| `tokio::sync::Mutex`           | active turn, active openid, pending approvals, resume messages |
| `tokio::sync::RwLock`          | `PersistedSessionState` session state (including pending setting) |
| `tokio::sync::Semaphore`       | Scheduler job concurrency control                        |
| `oneshot` channel               | Approval resolution                                      |
| `mpsc::unbounded_channel`      | gateway → App C2C event flow                             |
| `Weak<SchedulerCtx>`            | Scheduler tick loop points to the context strongly owned by `App` |
| `fs2` file locking              | Cross-process serialization for `scheduler/jobs.json` reads and writes |

---

## Extension Guide

### How to Add a New Command

1. Add the command string to the protected command list in `src/commands/alias.rs`.
2. Add a Chinese alias in `canonicalize_core_command()` in `src/commands/mod.rs`.
3. Implement the handler function in the appropriate command file (`session_cmds.rs`, `settings_cmds.rs`, or `cron_cmds.rs`).
4. Add a match branch in the main dispatch `maybe_handle_command()` in `src/commands/mod.rs`.
5. The handler returns a `CommandOutcome`, which is processed uniformly by `app/inbound.rs`.
6. Add locale keys in `locales/en.yml` and `locales/zh.yml`.
7. Add a help entry under `commands.help` in both locale files.

### How to Add a New Module

1. Create `src/<module>/mod.rs` (and its sub-files).
2. Add `pub mod <module>;` in `src/lib.rs`.
3. Determine the module's tier in the dependency DAG to avoid circular dependencies.
4. If stateful, pass it into the `App` struct via `Arc` (follow the patterns used by session/memory/shadow).
5. Add unit tests using `#[cfg(test)] mod tests` in each file.
6. Use the `rust-i18n` `t!` macro for internationalization, updating both locale files.
