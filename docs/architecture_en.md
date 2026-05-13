# CodexClaw System Architecture

> CodexClaw System Architecture Document
>
> This document is written in English. Section headings are in English. A Chinese version is available at the link below.

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

CodexClaw is a ~24,000-line Rust async application that bridges QQ (China's major messaging platform) to OpenAI Codex CLI through a long-lived app-server process. The system provides the following core capabilities:

| Capability                      | Description                                                                                      |
| ------------------------------- | ------------------------------------------------------------------------------------------------ |
| Session Management              | Maintains independent session state per user, supports foreground/background session switching    |
| Background Memory & Skill Distillation | Automatically extracts facts, insights, and skill information from conversation turns       |
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

**Data Flow Overview:**

- QQ Gateway pushes message events to `gateway.rs` via WebSocket.
- `App` serves as the central dispatcher, coordinating command parsing, Codex execution, approval flows, and message sending.
- `CodexExecutor` communicates with the long-lived codex app-server child process via JSON-RPC (stdio pipes).
- `Scheduler` runs scheduled tasks independently and can send synthetic messages through `App`.
- `ShadowWorker` asynchronously extracts memories and skills in the background.

---

## Module Responsibilities

### src/main.rs -- Entry Point

The program entry point. Responsible for parsing CLI arguments (routing to the scheduler CLI or Bot main service), loading configuration files, normalizing paths, and bootstrapping all services in the following order:

```
Data directory -> Session store -> QQ API -> app-server -> executor
  -> memory -> shadow -> scheduler -> gateway
```

### src/app.rs (~1656 lines) -- Central Dispatcher

The central dispatch module and core hub of the system.

| Responsibility            | Description                                                                                  |
| ------------------------- | -------------------------------------------------------------------------------------------- |
| Turn Coordination         | Uses an `AtomicBool` busy flag to globally allow only one turn at a time                      |
| Message Processing Pipeline | QQ event normalization -> attachment download -> command parsing -> Codex execution          |
| Approval Broker Integration | Routes approval requests initiated by app-server to QQ users                                |
| Self-Update Detection     | Triggers recompilation when Codex modifies source files                                      |

### src/commands.rs (~5,700+ lines) -- Command Dispatch

Command parsing and dispatch module.

- Parses slash commands with Chinese alias canonicalization
- Interactive settings handlers (e.g., model selector, etc.)
- Alias expansion with a recursion depth limit of 3 levels
- Protected command list to prevent alias collisions

### src/codex/ -- Codex Integration

The Codex integration subsystem and the largest module by code volume.

#### app_server/ -- Long-lived JSON-RPC Child Process

| File           | Lines/Size | Responsibility                              |
| -------------- | ---------- | ------------------------------------------- |
| supervisor.rs  | --         | Child process lifecycle management          |
| client.rs      | --         | JSON-RPC request/response handling          |
| transport.rs   | --         | tokio stdio stream transport                |
| session.rs     | ~46KB      | Per-turn session wrapper (largest file)     |
| protocol.rs    | ~31KB      | Hand-copied protocol type definitions       |
| approvals.rs   | --         | Approval request routing                    |
| events.rs      | --         | Event stream parsing                        |

#### Other codex/ Files

| File                | Responsibility                                                              |
| ------------------- | --------------------------------------------------------------------------- |
| executor.rs         | `ExecutionRequest`/`Result` type definitions, turn strategy selection       |
| prompt.rs           | System prompt construction with memory injection and qqbot instructions     |
| output.rs           | Parses qqbot code fence directives (image/file attachments)                 |
| runtime.rs          | Reads and writes Codex config.toml configuration file and model list        |
| config_snapshot.rs  | Bootstraps `~/.codex-claw/.codex` from the system `~/.codex`               |
| compact.rs          | Session context compaction                                                   |

### src/qq/ -- QQ Platform Integration

QQ platform integration module.

| File        | Responsibility                                                                                                 |
| ----------- | -------------------------------------------------------------------------------------------------------------- |
| api.rs      | REST client, token caching, chunked file upload (>5MB), message sequence number tracking                       |
| gateway.rs  | WebSocket connection, heartbeat maintenance, exponential backoff reconnection                                  |
| passive.rs  | `PassiveTurnEmitter` -- streams Codex events to QQ (batches tool calls, splits long messages at 500 characters)|
| types.rs    | QQ Gateway protocol type definitions                                                                           |

### src/session/ -- Session State

Session state module.

| File      | Responsibility                                                                                               |
| --------- | ------------------------------------------------------------------------------------------------------------ |
| state.rs  | `SessionSettings`, `ReasoningEffort`, `ContextMode`, `ApprovalPolicy`, `TokenUsageSnapshot`, and other types |
| store.rs  | Disk-backed per-user state store, file locking, foreground/background session management, workspace directories|

### src/scheduler/ -- Cron Scheduling

Scheduled task module.

| File            | Responsibility                                               |
| --------------- | ------------------------------------------------------------ |
| mod.rs          | Main loop, semaphore-based concurrency control               |
| store.rs        | `CronJob` schema definition, disk persistence, task directory management |
| runner.rs       | Task execution (reminder/codex-turn/codex-exec/shell)        |
| cli.rs          | CLI subcommands                                              |
| cron_expr.rs    | Cron expression parsing, timezone support                    |
| interactive.rs  | Multi-turn scheduled tasks, foreground session hijacking     |

### src/memory/ -- User Memory

User memory module.

| File      | Responsibility                                                     |
| --------- | ------------------------------------------------------------------ |
| store.rs  | Per-user `MEMORY.md` and `USER.md` files, `§`-delimited entries    |
| inject.rs | Formats memory blocks for prompt injection                         |
| scan.rs   | Memory entry scanning and filtering                                |

### src/shadow/ -- Background Distillation

Background distillation module.

| File      | Responsibility                                       |
| --------- | ---------------------------------------------------- |
| mod.rs    | `ShadowWorker`, FIFO deduplication by openid         |
| memory.rs | Extracts facts and insights from conversation turns  |
| skill.rs  | Extracts skill information from modified files       |
| prompt.rs | Prompts for memory/skill synthesis                   |
| runner.rs | One-shot Codex invocations for shadow tasks          |

### src/skills/ -- Skill Discovery

Skill discovery module.

| File      | Responsibility                               |
| --------- | -------------------------------------------- |
| index.rs  | Filesystem skill scanning with caching       |
| writer.rs | Writes `SKILL.md` files                      |

### Other Files

| File                | Responsibility                                             |
| ------------------- | ---------------------------------------------------------- |
| src/config.rs       | TOML configuration file loading and validation             |
| src/message.rs      | `IncomingMessage` type (text, image, file, quote, @mention)|
| src/self_update.rs  | Binary self-update (compile -> replace -> exit)            |
| src/lib.rs          | Module mapping and i18n initialization (`rust_i18n` macro) |

---

## Key Data Flows

### 1. Normal Conversation Turn

```
QQ WebSocket event
  -> gateway.rs dispatches C2CMessageEvent
  -> App.handle_c2c_event()
  -> Download/cache attachments
  -> commands.rs: Check if it is a slash command
    -> If it is a command: Process and return reply
    -> If not: run_normal_message()
  -> Acquire busy lock (AtomicBool)
  -> Set active_openid for approval routing
  -> Build prompt (session state + memory injection)
  -> CodexExecutor.execute(ExecutionRequest)
    -> AppServerSession -> codex app-server (JSON-RPC)
  -> PassiveTurnEmitter streams updates to QQ
  -> Parse qqbot directives in output (images/files)
  -> Send directive attachments via QQ API
  -> ShadowWorker asynchronously launches background memory/skill extraction
  -> Release busy lock
```

### 2. Approval Flow

```
codex app-server -> Approval notification (JSON-RPC)
  -> ApprovalBroker receives
  -> Enqueue to App.pending_approvals[openid]
  -> Send approval request message to QQ user
  -> User sends /approve, /deny, or /cancel
  -> App.resolve_pending_approval() finds the earliest pending approval
  -> Send ApprovalOutcome via oneshot channel
  -> app-server continues execution or cancels the turn
```

### 3. Scheduler

```
Scheduler.tick() runs every tick_secs (default 30 seconds)
  -> Scans all jobs, finds those that are due (next_run_at <= now or run_now_at is set)
  -> Acquire semaphore permit (max_concurrent_jobs)
  -> runner.run_job() dispatches by action type
    -> Reminder: Send message via QQ API
    -> CodexTurn: Dispatch as synthetic message through App
    -> Shell: Launch subprocess
  -> On success: Update run_count, last_run_status, write run log
  -> On failure: Increment failure_streak, retry with backoff
  -> On reaching circuit_breaker_threshold: Auto-disable and notify job owner
  -> One-shot jobs: Recycle to cron-jobs-trash/
```

---

## Async Patterns

CodexClaw is built on the tokio multi-threaded runtime and uses the following async patterns:

| Pattern                         | Purpose                                                  |
| ------------------------------- | -------------------------------------------------------- |
| `Arc<App>`                      | Shared application state across all spawned tasks        |
| `AtomicBool`                    | Busy flag (fast, expected uncontended)                   |
| `tokio::sync::Mutex`           | Per-user state (active_turn, pending_approvals)          |
| `tokio::sync::RwLock`          | Gateway session state                                    |
| `tokio::sync::Semaphore`       | Scheduler concurrency control                            |
| `oneshot` channel               | Approval resolution and turn cancellation                |
| `mpsc::unbounded_channel`      | Execution update streaming                               |
| `Weak<App>`                     | Avoids reference cycles in the Scheduler                 |
| `fs2` file locking              | Disk synchronization for concurrent session state writes |

---

## Extension Guide

### How to Add a New Command

1. Add the command string to `PROTECTED_COMMANDS` in `src/commands.rs`.
2. Add a Chinese alias in `canonicalize_core_command()` near the bottom of `commands.rs`.
3. Add a match branch in the main dispatch block (around line 245).
4. Implement the handler function (async, accepting openid + session + other state parameters).
5. Add locale keys in `locales/en.yml` and `locales/zh.yml`.
6. Add a help entry under `commands.help` in both locale files.

### How to Add a New Module

1. Create `src/<module>/mod.rs`.
2. Add `pub mod <module>;` in `src/lib.rs`.
3. If needed, wire the module into the `App` struct (follow the patterns used by session/memory/shadow).
4. Add unit tests using `#[cfg(test)] mod tests` in each file.
5. If needed, add integration tests in the `tests/` directory.
