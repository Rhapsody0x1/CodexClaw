# CodexClaw Command Reference

> CodexClaw is a Rust-based QQ bot powered by OpenAI Codex. All commands are triggered via QQ direct messages in the `/command` format.
> Chinese aliases are fully equivalent to their English counterparts and are automatically canonicalized at the message dispatch layer.

---

## Table of Contents

- [Command Dispatch](#command-dispatch)
- [Basic Commands](#1-basic-commands)
  - [/help](#help)
  - [/status](#status)
  - [/lang](#lang)
  - [/verbose](#verbose)
- [Session Management](#2-session-management)
  - [/new](#new)
  - [/stop](#stop)
  - [/interrupt](#interrupt)
  - [/save](#save)
  - [/sessions](#sessions)
  - [/import](#import)
  - [/resume](#resume)
  - [/loadbg](#loadbg)
  - [/bg](#bg)
  - [/fg](#fg)
  - [/rename](#rename)
  - [/compact](#compact)
- [Model Settings](#3-model-settings)
  - [/model](#model)
  - [/fast](#fast)
  - [/context](#context)
  - [/reasoning](#reasoning)
- [Approval Flow](#4-approval-flow)
  - [/approvals](#approvals)
  - [/approve](#approve)
  - [/approve-session](#approve-session)
  - [/deny](#deny)
  - [/cancel (approval)](#cancel)
- [Plan Mode](#5-plan-mode)
  - [/plan](#plan)
  - [/execute-plan](#execute-plan)
  - [/keep-planning](#keep-planning)
  - [/cancel-plan](#cancel-plan)
- [Command Aliases](#6-command-aliases)
  - [/alias add](#alias-add)
  - [/alias list](#alias-list)
  - [/alias remove](#alias-remove)
- [Scheduler](#7-scheduler)
  - [/cron list](#cron-list)
  - [/cron pause](#cron-pause)
  - [/cron resume](#cron-resume)
  - [/cron rm](#cron-rm)
  - [/cron run-now](#cron-run-now)
  - [/cron tail](#cron-tail)
- [System Commands](#8-system-commands)
  - [/self-update](#self-update)
  - [/back](#back)
  - [/retry](#retry)
- [Interactive Mode Rules](#interactive-mode-rules)

---

*Read this in: [English](commands_en.md) | [中文](commands.md)*

---

## Command Dispatch

1. The user sends `/command` or `/中文命令` in QQ.
2. Chinese aliases are canonicalized to their corresponding English commands by `canonicalize_core_command()`.
3. If currently in an interactive mode (e.g., model selector), any slash command other than `/back` will automatically exit the interactive mode before executing; non-slash text is consumed by the interactive mode handler.
4. If a user-defined alias matches, the alias is expanded and executed step by step (maximum expansion depth: 3 levels).
5. If nothing matches, the text is sent to Codex as a normal message for execution.

### Complete Chinese-English Alias Mapping Table

| Chinese Command | English Command |
|---|---|
| `/帮助` | `/help` |
| `/语言` | `/lang` |
| `/模型` | `/model` |
| `/快速` | `/fast` |
| `/上下文` | `/context` |
| `/思考` | `/reasoning` |
| `/详细` | `/verbose` |
| `/审批` | `/approvals` |
| `/计划` | `/plan` |
| `/定时` | `/cron` |
| `/实施` | `/execute-plan` |
| `/继续规划` | `/keep-planning` |
| `/取消计划` | `/cancel-plan` |
| `/同意` | `/approve` |
| `/同意本会话` | `/approve-session` |
| `/拒绝` | `/deny` |
| `/取消` | `/cancel` |
| `/重试` | `/retry` |
| `/状态` | `/status` |
| `/会话` | `/sessions` |
| `/导入` | `/import` |
| `/新建` | `/new` |
| `/后台` | `/bg` |
| `/前台` | `/fg` |
| `/恢复` | `/resume` |
| `/载入后台` | `/loadbg` |
| `/保存` | `/save` |
| `/重命名` | `/rename` |
| `/停止` | `/stop` |
| `/中断` | `/interrupt` |
| `/压缩` | `/compact` |
| `/自更新` | `/self-update` |
| `/别名` | `/alias` |
| `/返回` | `/back` |

---

## 1. Basic Commands

### /help

Displays the command help list.

```
/help
/帮助
```

**Chinese alias:** `/帮助`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Returns a complete list of commands organized by category, including sections for basic commands, model settings, approval settings, session management, and advanced commands. The output follows the current language setting (Chinese or English).

**Example:**
```
/help
→ Returns the complete command guide
```

---

### /status

Displays a comprehensive summary of the current session status.

```
/status
/状态
```

**Chinese alias:** `/状态`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Returns the following information:
- Current working directory
- Currently active model (including override info)
- Verbose output toggle status
- Number of background sessions and their list
- Foreground status (running / idle)
- Context window usage (percentage, used tokens / total capacity)
- Current UI language

**Example:**
```
/状态
→ Working directory: `/home/user/project`
  Model: gpt-5.4
  Verbose output: off
  Background sessions: none
  Foreground status: idle
  Context window: — (no usage data yet)
  Language: zh
```

---

### /lang

Switch the UI language.

```
/lang [en|zh|status]
/语言 [en|zh|status]
```

**Chinese alias:** `/语言`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `value` | string | No | `en` (English), `zh` (Chinese), `status` (view current setting) |

**Behavior:**
- No argument: Enter interactive language selector (`PendingSetting::Lang`), prompting the user to input `en` or `zh`.
- `en` / `zh`: Directly switch to the specified language.
- `status`: Display the current language setting.
- Unsupported language value: Returns an error prompt listing available options.

**Example:**
```
/lang zh
→ Language switched to: zh

/语言 status
→ Current language: zh

/lang
→ Current language: zh
  Available values: en / zh
```

---

### /verbose

Toggle verbose/compact mode for tool call output.

```
/verbose [on|off|status]
/详细 [on|off|status]
```

**Chinese alias:** `/详细`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `value` | string | No | `on` (enable), `off` (disable), `status` (view current status) |

**Behavior:**
- No argument: Enter interactive selector.
- `on` / `true`: Enable verbose output. Tool call details during Codex execution will be sent in full.
- `off` / `false`: Disable verbose output.
- `status`: Display the current verbose output status.

**Example:**
```
/verbose on
→ Verbose output enabled

/详细 status
→ Verbose output: off
```

---

## 2. Session Management

### /new

Create a new foreground session.

```
/new [working_directory]
/新建 [working_directory]
```

**Chinese alias:** `/新建`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `working_directory` | path string | No | The working directory for the new session. Supports absolute paths; relative paths are resolved against the current foreground working directory |

**Behavior:**
1. If the current foreground has content (bound to a session_id or already saved), it is automatically moved to the background with an alias assigned.
2. A new temporary foreground session is created.
3. If a working directory is specified, the new session uses that directory; otherwise the default working directory is used.
4. Returns a creation confirmation and a summary of the current runtime configuration (model, reasoning depth, etc.).

**Example:**
```
/new
→ New temporary foreground session created.

/新建 /home/user/another-project
→ Previous foreground session moved to background: `bg-1`
  New temporary foreground session created.
  Working directory: `/home/user/another-project`
```

---

### /stop

End the current foreground session.

```
/stop
/停止
```

**Chinese alias:** `/停止`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:**
1. End the current foreground session: saved sessions are retained, unsaved ones are discarded.
2. If there are background sessions, automatically switch back to the most recent background session.
3. If there are no background sessions, create a new temporary foreground session.
4. If there was no active foreground session, it will still attempt to switch back to a background session or reset.

**Note:** When an interactive session with an in-progress scheduled task exists, `/stop` will prioritize ending that interactive task and restoring the original conversation, rather than executing the normal stop logic.

**Example:**
```
/停止
→ Foreground session ended and retained. Automatically switched to most recent background session `work`.

/stop
→ Foreground session ended and discarded (unsaved). New temporary foreground session created.
```

---

### /interrupt

Stop the currently running task without ending the session.

```
/interrupt
/中断
```

**Chinese alias:** `/中断`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Sends a cancellation signal to the currently executing Codex turn. The session itself remains unchanged and new messages can continue to be sent.

**Example:**
```
/中断
→ Stop request sent for current run.
```

---

### /save

Mark the current foreground session for persistent storage.

```
/save
/保存
```

**Chinese alias:** `/保存`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Marks the current foreground session as `saved` status. Saved sessions are retained rather than discarded on `/stop`. If already in saved state, displays "Already in saved state."

**Example:**
```
/保存
→ Foreground session marked for persistent storage.

/save
→ Foreground session is already in saved state.
```

---

### /sessions

Browse historical sessions, grouped by working directory (project).

```
/sessions [all]
/sessions <project_number> [page]
/会话 [all]
/会话 <project_number> [page]
```

**Chinese alias:** `/会话`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `scope` | string | No | Only `all` is supported (default is also `all`) |
| `project_number` | number | No | A number from the project list to enter the corresponding project's sessions |
| `page` | number | No | Page number for session pagination, defaults to 1 |

**Behavior:**
1. No argument or `all`: Display a project list grouped by working directory, entering interactive mode (`PendingSetting::SessionsProjects`).
2. Enter a project number: Enter the session list for that project (paginated), entering interactive mode (`PendingSetting::SessionsSessions`).
3. In the session list, you can use `/resume` or `/loadbg` to operate on specific sessions.

**Example:**
```
/会话
→ Project list total=3:
  1. /home/user/project-a | sessions=5 | latest=2026-05-10
  2. /home/user/project-b | sessions=2 | latest=2026-05-08
  3. /home/user/project-c | sessions=1 | latest=2026-05-01
  Enter a project number (e.g., `1`) to go deeper, or `/back` to exit.
```

---

### /import

Import sessions from the host Codex system at `~/.codex/sessions`.

```
/import
/import <number|session_id>
/导入
/导入 <number|session_id>
```

**Chinese alias:** `/导入`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `selector` | number or session_id | No | Identifier of the session to import |

**Behavior:**
1. No argument: Display a list of importable projects from `~/.codex/sessions` (grouped by working directory), entering interactive mode (`PendingSetting::ImportProjects`).
2. Enter a project number: Drill down to view the list of importable sessions under that project.
3. Enter a session number or ID: Execute the import by copying the session to `~/.codex-claw/.codex/sessions/`. If the session already exists, the import configuration is refreshed.

**Example:**
```
/导入
→ Importable projects total=2
  1. /home/user/project-a | sessions=3 | latest=2026-05-10
  2. /home/user/project-b | sessions=1 | latest=2026-05-05
  Enter a project number to go deeper, or `/back` to exit.
```

---

### /resume

Restore a session from disk to the foreground.

```
/resume <number|session_id>
/恢复 <number|session_id>
```

**Chinese alias:** `/恢复`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `selector` | number or session_id | Yes | The list number or full/prefix session ID of the session to restore |

**Behavior:**
1. Restore the specified disk session to the foreground.
2. The previous foreground session is automatically moved to the background.
3. After restoration, display the session summary and runtime configuration.
4. If restoration fails (e.g., Codex session resume error), enter interactive recovery mode (`PendingSetting::ResumeRecovery`) with options for `/retry`, `/new`, or `/cancel`.

**Note:** When invoked without arguments, enters a project browsing interactive mode similar to `/sessions`, allowing you to drill down level by level to select a session.

**Example:**
```
/恢复 1
→ Previous foreground moved to background: `bg-1`.
  Restored session: Fix login bug (workspace: `/home/user/project`).
```

**When restoration fails:**
```
→ Failed to restore current Codex session. A new thread has not been automatically created.
  You can choose: `/retry` to try again, `/new` to start a new session, or `/cancel` to abandon the restoration.
```

---

### /loadbg

Load a disk session into the background.

```
/loadbg <number|session_id> [alias]
/载入后台 <number|session_id> [alias]
```

**Chinese alias:** `/载入后台`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `selector` | number or session_id | Yes | Identifier of the session to load |
| `alias` | string | No | Alias label for the background session |

**Behavior:** Loads a session from disk into the background with an optional alias. Does not affect the current foreground session. When invoked without arguments, enters a project browsing interactive mode.

**Example:**
```
/载入后台 3 work
→ Session loaded to background label `work`: Refactor database module.
```

---

### /bg

Move the current foreground session to the background.

```
/bg <alias>
/后台 <alias>
```

**Chinese alias:** `/后台`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `alias` | string | Yes | Alias label for the background session |

**Behavior:**
- Move the current foreground session to the background with the specified alias.
- If the foreground is a blank temporary session (no session_id, not saved), it will not be moved to the background; it will simply be reset to a new temporary session.
- When invoked without arguments, displays usage instructions.

**Example:**
```
/后台 work
→ Foreground session moved to background: `work`.

/bg temp
→ Current foreground is a blank temporary session; it has been reset to a new temporary session.
```

---

### /fg

Switch a background session to the foreground.

```
/fg <alias>
/前台 <alias>
```

**Chinese alias:** `/前台`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `alias` | string | No | Alias label of the background session |

**Behavior:**
- With alias specified: Switch the corresponding background session to the foreground; the previous foreground is moved to the background.
- No argument: Enter interactive background session selector listing all background sessions for selection. If there are no background sessions, displays "No background sessions available."

**Example:**
```
/前台 work
→ Previous foreground moved to background: `bg-2`.
  Switched to background session `work`.

/fg
→ Background sessions:
    • `work`
    • `debug`
```

---

### /rename

Rename a background session label.

```
/rename <old_alias> <new_alias>
/重命名 <old_alias> <new_alias>
```

**Chinese alias:** `/重命名`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `old_alias` | string | Yes | Current background session alias |
| `new_alias` | string | Yes | New alias |

**Behavior:** Renames the specified background session's alias from the old name to the new name. Requires exactly two parameters, otherwise displays usage instructions.

**Example:**
```
/重命名 bg-1 refactor
→ Background label renamed: `bg-1` -> `refactor`
```

---

### /compact

Manually compress the current session context.

```
/compact
/压缩
```

**Chinese alias:** `/压缩`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:**
1. Prerequisites: No task is currently running, and there is an active Codex session in the foreground (session_id is not empty).
2. Triggers Codex's session compression feature, compressing the current conversation history into a summary.
3. After compression, subsequent conversations continue based on the compressed summary.
4. Returns a success confirmation and warning prompt after compression is complete.

**Note:** Repeated compression may reduce model accuracy. When context approaches the capacity limit (>=80%), the system automatically issues a warning suggesting the use of this command. If conditions permit, it is recommended to create a more focused new session with `/new` in a timely manner.

**Example:**
```
/压缩
→ Compressing current session context, will notify you when done.
  ...
→ Current session context has been manually compressed. Subsequent conversations will continue based on the compressed summary.
  Tip: After long conversation threads and repeated compactions, model accuracy may decrease. If conditions permit, consider creating a more focused new session with `/new`.
```

**Error scenarios:**
```
/compact  (when a task is running)
→ A task is currently running. Please wait for it to complete before executing `/compact`.

/compact  (when no active session exists)
→ There is no Codex session in the foreground to compress yet. Start a conversation first, then execute `/compact`.
```

---

## 3. Model Settings

> **Global vs. session-level settings:** When the foreground session is an unsaved temporary session, changes to model, fast mode, context, and reasoning depth are written to the global runtime configuration file (`config.toml`). When the foreground session is saved (`saved=true`), changes apply only to the current session.

### /model

Set or view the currently active model.

```
/model [name|inherit|status]
/模型 [name|inherit|status]
```

**Chinese alias:** `/模型`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `value` | string | No | Model name, `inherit` (restore config default), `status` (view current status) |

**Behavior:**
- No argument: Enter interactive model selector, displaying the current model and all available models (including aliases and descriptions).
- Specify a model name: Set to that model. Supports model alias matching.
- `inherit` / `default`: Clear the override, restoring the default model from the configuration file.
- `status`: Display the currently active model and override value.

**Example:**
```
/模型
→ **Current model:** gpt-5.4
  **Available models:**
  - `gpt-5.4`
    - Default model
  - `o3`
    - Alias: o3-mini
  ...

/model o3
→ Model updated to: o3

/模型 status
→ model: gpt-5.4    override: inherit
```

---

### /fast

Set the fast reasoning mode (Fast service tier).

```
/fast [on|off|inherit|status]
/快速 [on|off|inherit|status]
```

**Chinese alias:** `/快速`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `value` | string | No | `on` (enable), `off` (disable), `inherit` (restore default), `status` (view status) |

**Behavior:**
- No argument: Enter interactive selector.
- `on`: Enable Fast service tier.
- `off`: Use Flex tier.
- `inherit`: Clear the override, restoring the default.
- `status`: Display the current fast setting value.
- Setting is always written to the global runtime configuration (`SetGlobalFast`).

**Example:**
```
/快速 on
→ fast updated to: on

/fast status
→ fast: off
```

---

### /context

Set the context window mode.

```
/context [standard|1m|inherit|status]
/上下文 [standard|1m|inherit|status]
```

**Chinese alias:** `/上下文`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `value` | string | No | `standard` (standard 272K tokens), `1m` / `1M` (long context 1M tokens), `inherit` (restore default), `status` (view status) |

**Behavior:**
- No argument: Enter interactive selector.
- `standard`: Set to standard context window (272K tokens).
- `1m` / `1M`: Set to long context window (1M tokens).
- `inherit`: Clear the override, restoring the default.
- `status`: Display the current context mode.
- For saved sessions, this is a session-level setting; for unsaved sessions, it is a global setting (`SetGlobalContext`).

**Example:**
```
/上下文 1m
→ Context mode updated to: 1M

/context status
→ Context: 272K
```

---

### /reasoning

Set the reasoning (thinking) depth.

```
/reasoning [low|medium|high|xhigh|inherit|status]
/思考 [low|medium|high|xhigh|inherit|status]
```

**Chinese alias:** `/思考`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `value` | string | No | `low`, `medium`, `high`, `xhigh` (extra high), `inherit` (restore default), `status` (view status) |

**Behavior:**
- No argument: Enter interactive selector.
- Sets the reasoning depth, which affects the model's thinking token allocation.
- For saved sessions, this is a session-level setting; for unsaved sessions, it is a global setting (`SetGlobalReasoning`).

**Example:**
```
/思考 high
→ Reasoning depth updated to: high

/reasoning status
→ Reasoning depth: medium
```

---

## 4. Approval Flow

When Codex needs to execute shell commands, write/modify files, or request permission escalation, it sends an approval request via QQ message and waits for the user's decision.

### /approvals

View and switch the execution approval policy.

```
/approvals [on|strict|off|status]
/审批 [on|strict|off|status]
```

**Chinese alias:** `/审批`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `value` | string | No | `on` (on-request approval, default), `strict` (unless-trusted strict mode), `off` (disable approval), `status` (view current policy) |

**Behavior:**
- No argument: Display the current policy and enter interactive selector (`PendingSetting::Approvals`).
- `on` / `on-request`: On-request approval (default).
- `strict` / `unless-trusted`: Strict mode, all operations require approval.
- `off` / `never`: Disable approval, automatically allow all operations.
- `status`: Only display the current policy.

**Example:**
```
/审批
→ Current approval policy: on-request (default)
  Reply with the following options to switch:
  /approvals on           On-request approval (default)
  /approvals strict       Strict (unless-trusted)
  /approvals off          Disable approval

/approvals strict
→ Approval policy switched to: strict (unless-trusted)
```

---

### /approve

Allow the current pending approval request (one-time only).

```
/approve
/同意
```

**Chinese alias:** `/同意`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Approves the earliest pending approval request. Valid only for this instance; the next similar operation will still require approval. If there are no pending approval requests, displays "No pending approval requests."

**Example:**
```
(Codex requests to execute a shell command)
/同意
→ Request approved.
```

---

### /approve-session

Automatically allow similar commands in the current session.

```
/approve-session
/同意本会话
```

**Chinese alias:** `/同意本会话`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Approves the current approval request and automatically allows subsequent similar commands in the current session.

**Example:**
```
/同意本会话
→ Request approved. Similar commands will be automatically allowed going forward.
```

---

### /deny

Deny the current approval request.

```
/deny
/拒绝
```

**Chinese alias:** `/拒绝`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Denies the current approval request. Codex will attempt to complete the task using alternative methods.

**Example:**
```
/拒绝
→ Request denied.
```

---

### /cancel

Deny the approval request and terminate the current turn.

```
/cancel
/取消
```

**Chinese alias:** `/取消`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Denies the current approval request and instructs Codex to terminate the current execution turn. Stronger than `/deny` -- not only does it deny, but it also aborts the entire turn.

**Note:** In the resume-failure interactive mode, `/cancel`'s behavior is to cancel the resume flow (clearing `PendingSetting::ResumeRecovery`), rather than handling approvals.

**Example:**
```
/取消
→ Denied and requested termination of current turn.
```

---

## 5. Plan Mode

In Plan Mode, Codex operates in read-only mode, creating a plan before executing.

### /plan

Enter or exit Plan Mode.

```
/plan [on|off|status]
/计划 [on|off|status]
```

**Chinese alias:** `/计划`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `value` | string | No | `on` (enable), `off` (disable), `status` (view status). Also supports Chinese: `开`/`开启`/`关`/`关闭` |

**Behavior:**
- No argument: Display the current Plan Mode status and enter interactive selector (`PendingSetting::Plan`).
- `on`: Enter Plan Mode. Codex will create a plan in a read-only sandbox, then emit a `<proposed_plan>` block.
- `off`: Exit Plan Mode, resuming the default execution mode.
- When Codex produces a `<proposed_plan>`, the system automatically prompts the user to use `/execute-plan`, `/keep-planning`, or `/cancel-plan`.

**Example:**
```
/计划 on
→ Entered Plan Mode. Codex will create a plan in a read-only sandbox, then send a <proposed_plan>. You can use /execute-plan to approve execution.
```

---

### /execute-plan

Approve the pending plan and begin execution.

```
/execute-plan
/实施
```

**Chinese alias:** `/实施`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:**
1. Exit Plan Mode (`plan_mode = false`).
2. Clear `pending_plan`.
3. Prompt the user to send a follow-up message (e.g., "Start") to trigger plan-based execution.
4. If there is no pending plan, displays "No pending plan to execute."

**Example:**
```
/实施
→ Exited Plan Mode and approved the plan. You can reply with "Start" or describe the next step, and I will execute according to the plan.
```

---

### /keep-planning

Stay in Plan Mode to continue refining the plan.

```
/keep-planning
/继续规划
```

**Chinese alias:** `/继续规划`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Keeps Plan Mode enabled and clears the current `pending_plan`. The next message continues the planning iteration.

**Example:**
```
/继续规划
→ Plan Mode retained, continuing to refine the plan. The next message will continue planning.
```

---

### /cancel-plan

Discard the current pending plan.

```
/cancel-plan
/取消计划
```

**Chinese alias:** `/取消计划`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Clears `pending_plan` without changing the Plan Mode toggle state.

**Example:**
```
/取消计划
→ Current plan discarded.
```

---

## 6. Command Aliases

Users can create custom command aliases that support multi-step piped execution.

### /alias add

Create a command alias.

```
/alias add <name> <command1> | <command2> | ...
/别名 add <name> <command1> | <command2> | ...
```

**Chinese alias:** `/别名`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | Yes | Alias name. Rules: 1-20 characters, no `\|`, cannot start with `/` |
| `commands...` | string | Yes | One or more sub-commands separated by `\|` |

**Behavior:**
- Creates a command alias that executes all sub-commands sequentially when invoked.
- An alias cannot conflict with built-in command names (including both Chinese and English).
- Maximum expansion depth of 3 levels (to prevent infinite loops from recursive aliases).
- During expansion, non-command text is skipped (marked as "Skipped: not a command").

**Example:**
```
/alias add setup /model o3 | /reasoning high | /context 1m
→ Alias `/setup` registered with 3 steps

/setup
→ Alias `/setup` execution results:
  Model updated to: o3
  Reasoning depth updated to: high
  Context mode updated to: 1M
```

---

### /alias list

List all registered aliases.

```
/alias list
/别名 list
```

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Lists all aliases registered by the current user along with their sub-commands. If there are no aliases, displays instructions on how to create one. Also supports `ls` as an abbreviation for `list`.

**Example:**
```
/alias list
→ Registered aliases:
    /setup → /model o3 | /reasoning high | /context 1m
    /quick → /fast on | /reasoning low
```

---

### /alias remove

Delete a command alias.

```
/alias remove <name>
/别名 remove <name>
```

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | Yes | Name of the alias to delete |

**Behavior:** Deletes the specified alias. If the alias does not exist, displays "Does not exist." Also supports `rm`, `delete`, and `del` as abbreviations for `remove`.

**Example:**
```
/alias remove setup
→ Alias `/setup` deleted
```

---

## 7. Scheduler

Manage your scheduled tasks via QQ. Tasks are executed in the background by CodexClaw's scheduler engine.

### /cron list

List your scheduled tasks.

```
/cron list
/定时 list
```

**Chinese alias:** `/定时 list`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:** Lists all scheduled tasks owned by the current user, displaying the job_id, enabled status, next run time, run count, failure count, and title. Also supports `ls` as an abbreviation for `list`.

**Example:**
```
/cron list
→ Your scheduled tasks:
  abc123  enabled  next=2026-05-12T08:00:00Z  runs=5  failures=0  Daily check
  def456  disabled next=-                      runs=12 failures=2  Weekly report
```

---

### /cron pause

Pause a specified scheduled task.

```
/cron pause <job_id>
/定时 pause <job_id>
```

| Parameter | Type | Required | Description |
|---|---|---|---|
| `job_id` | string | Yes | ID of the task to pause |

**Behavior:** Marks the specified task as `disabled`. You can only manage your own tasks.

**Example:**
```
/cron pause abc123
→ `Daily check` paused.
```

---

### /cron resume

Resume a paused scheduled task.

```
/cron resume <job_id>
/定时 resume <job_id>
```

| Parameter | Type | Required | Description |
|---|---|---|---|
| `job_id` | string | Yes | ID of the task to resume |

**Behavior:** Restores the specified task to `enabled` and recalculates the next run time. For one-time tasks (OneShot) that have already expired, it will be scheduled to run immediately.

**Example:**
```
/cron resume abc123
→ `Daily check` resumed.
```

---

### /cron rm

Delete a scheduled task.

```
/cron rm <job_id>
/定时 rm <job_id>
```

| Parameter | Type | Required | Description |
|---|---|---|---|
| `job_id` | string | Yes | ID of the task to delete |

**Behavior:** Deletes the specified task and its associated files. You can only delete your own tasks. Also supports `remove` as a synonym for `rm`.

**Example:**
```
/cron rm def456
→ `Weekly report` deleted.
```

---

### /cron run-now

Trigger a scheduled task immediately.

```
/cron run-now <job_id>
/定时 run-now <job_id>
```

| Parameter | Type | Required | Description |
|---|---|---|---|
| `job_id` | string | Yes | ID of the task to trigger immediately |

**Behavior:** Executes the specified task once immediately at the next scheduling tick, without affecting the normal cron schedule.

**Example:**
```
/cron run-now abc123
→ `Daily check` has been scheduled to run immediately once.
```

---

### /cron tail

View the most recent run log.

```
/cron tail <job_id>
/定时 tail <job_id>
```

| Parameter | Type | Required | Description |
|---|---|---|---|
| `job_id` | string | Yes | ID of the task whose log to view |

**Behavior:** Reads the log file from the most recent run of the specified task, returning the last 3500 characters of content. If there are no run records, displays "No run logs yet."

**Example:**
```
/cron tail abc123
→ Most recent run log `/path/to/runs/2026-05-11T080000Z.log`:
  [Log content...]
```

---

## 8. System Commands

### /self-update

Build and deploy the latest version.

```
/self-update
/自更新
```

**Chinese alias:** `/自更新`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:**
1. Checks whether a task is currently running; if so, refuses to execute.
2. Runs the build command in the repository directory (default: `cargo build --release`).
3. After a successful build, replaces the currently running executable with the newly compiled binary.
4. Notifies the codex app-server to shut down.
5. Exits the current process (`exit(0)`). An external service manager (e.g., systemd, launchd) is responsible for restarting it.

**Note:** This is a destructive operation; the process will exit immediately. Ensure that an external daemon service is configured for automatic restart.

**Example:**
```
/自更新
→ Running binary overwritten: `/home/user/.codex-claw/bin/codex-claw`
  Exiting current process (codex app-server has been notified to shut down). If an external daemon service is configured, it will restart automatically; otherwise, please restart manually.
```

---

### /back

Exit the current interactive setting.

```
/back
/返回
```

**Chinese alias:** `/返回`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:**
- In interactive mode (e.g., model selector, language selector, session browsing, etc.): Exit interactive mode and clear `pending_setting`.
- When not in interactive mode: Displays "No interactive setting is currently in progress."

**Example:**
```
(In the model selector)
/返回
→ Exited `Model` settings.

(When not in interactive mode)
/back
→ No interactive setting is currently in progress.
```

---

### /retry

Retry restoration in the resume-failure interactive mode.

```
/retry
/重试
```

**Chinese alias:** `/重试`

| Parameter | Type | Required | Description |
|---|---|---|---|
| (none) | — | — | — |

**Behavior:**
- In the resume-failure interactive mode (`PendingSetting::ResumeRecovery`): Clears the pending state and retries restoring the previously failed session.
- When not in that mode: Displays "No failed session to restore."

**Example:**
```
(After restoration failure)
/重试
→ (Retrying restoration)

(When no failed restoration exists)
/retry
→ No failed session to restore.
```

---

## Interactive Mode Rules

The following commands enter an interactive setting mode when invoked without arguments: `/model`, `/fast`, `/context`, `/reasoning`, `/verbose`, `/lang`, `/approvals`, `/plan`, `/sessions`, `/import`, `/fg`, `/resume`, `/loadbg`.

Behavior rules in interactive mode:

1. **Non-slash text** is consumed by the interactive handler for matching options, entering values, etc. It is not forwarded to Codex.
2. **`/back`** (or `/返回`) exits interactive mode and clears the pending state.
3. **Other slash commands** implicitly exit the current interactive mode (clearing the pending state and sending an "Exited `XXX` settings" notification), then execute that command.
4. **Fuzzy matching in interactive mode:** Entered values are fuzzy-matched against the option list. If a unique match is found, it is applied; if multiple matches are found, the user is prompted to "be more specific"; if no match is found, the user is prompted with "No match found."
