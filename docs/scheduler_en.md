# CodexClaw Scheduler -- Scheduled Task System

> Branch: `vibe-dev`, 2026-05

---

*Read this in: [English](scheduler_en.md) | [中文](scheduler.md)*

---

## Table of Contents

1. [Architecture](#architecture)
2. [Job Schema](#job-schema)
3. [CronKind](#cronkind)
4. [JobAction](#jobaction)
5. [Interactive Mode](#interactive-mode)
6. [CLI Commands](#cli-commands)
7. [QQ Commands](#qq-commands)
8. [Directory Layout](#directory-layout)
9. [Lifecycle](#lifecycle)
10. [Best Practices](#best-practices)

---

## Architecture

The Scheduler is an infinite-loop task running on the tokio async runtime. Core parameters:

| Parameter | Default | Description |
|-----------|---------|-------------|
| Tick interval | 30 seconds | Interval between checks for tasks to execute |
| Max concurrency | 4 | Controlled via tokio `Semaphore` to prevent resource exhaustion |
| Deduplication | in-flight set | The same task will not be scheduled twice simultaneously |

Each tick, the Scheduler scans the job table, finds tasks where `next_run_at <= now` or `run_now_at` has been set, acquires a semaphore permit, and dispatches execution.

---

## Job Schema

Jobs are defined by the `CronJob` struct (`src/scheduler/store.rs`), with the following fields:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | ULID, globally unique identifier |
| `owner_openid` | `String` | The job owner's QQ OpenID |
| `title` | `String` | Job name, used for display in listings |
| `kind` | `CronKind` | Schedule type: recurring or one-shot |
| `action` | `JobAction` | Execution action, see details below |
| `workspace_dir` | `PathBuf` | The job's working directory |
| `deliver` | `DeliverPolicy` | Result delivery policy |
| `created_at` | `DateTime<Utc>` | Creation time |
| `next_run_at` | `DateTime<Utc>` | Next scheduled execution time |
| `run_now_at` | `Option<DateTime<Utc>>` | Manually triggered immediate execution time |
| `last_run_at` | `Option<DateTime<Utc>>` | Last actual execution time |
| `last_run_status` | `RunStatus` | Last execution status |
| `run_count` | `u64` | Cumulative execution count |
| `failure_streak` | `u32` | Consecutive failure count (circuit breaker basis) |
| `disabled` | `bool` | Whether the job is disabled |

### DeliverPolicy

| Enum Value | Behavior |
|------------|----------|
| `PushToOwner` | Always push results to the owner |
| `PushIfNonEmpty` | Only push if output is non-empty |
| `LogOnly` | Write to log only, do not push |
| `PushTruncated` | Push truncated results (for large outputs) |

### RunStatus

| Enum Value | Description |
|------------|-------------|
| `Success` | Execution succeeded |
| `Failure` | Execution failed |
| `Skipped` | Skipped (e.g., due to circuit breaker or manual disable) |

---

## CronKind

### Recurring

```rust
Recurring { cron: String, tz: String }
```

Uses a standard **6-field cron expression**: `second minute hour day-of-month month day-of-week`

If the user provides a 5-field expression (omitting seconds), the system automatically prepends `"0"`.

Examples:

| Expression | Meaning |
|------------|---------|
| `0 30 9 * * *` | Every day at 09:30:00 |
| `0 0 */2 * * *` | Every 2 hours on the hour |
| `0 0 8 * * Mon-Fri` | Weekdays at 8:00 AM |

The `tz` field specifies the timezone (e.g., `Asia/Shanghai`, `UTC`).

### OneShot

```rust
OneShot { at: DateTime<Utc> }
```

Triggers once at the specified UTC time. After execution completes, it is archived to `cron-jobs-trash/`.

---

## JobAction

### 1. Reminder

```rust
Reminder { message: String }
```

The simplest action. When the execution time is reached, the `message` is pushed to the owner via QQ.
Suitable for simple timed reminder scenarios.

### 2. CodexTurn (Recommended)

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

Executes a complete Codex turn through the app-server pipeline. This is the most powerful job action.

**session_strategy** determines the session lifecycle:

| Strategy | Description |
|----------|-------------|
| `PerInvocation` | Creates a fresh session on each execution (default) |
| `Persistent` | Reuses the session across executions, preserving context |

The optional `interactive` field can be included to enable interactive mode (see the next section).

### 3. CodexExec (Legacy)

```rust
CodexExec {
    prompt: String,
    model: String,
    extra_args: Vec<String>,
    env: HashMap<String, String>,
}
```

Executed via the `codex exec` CLI. This is a legacy path. New jobs should use `CodexTurn`.

### 4. Shell

```rust
Shell {
    program: String,
    args: Vec<String>,
    env: HashMap<String, String>,
}
```

Executes an arbitrary shell command. Suitable for simple script invocations or system commands.

---

## Interactive Mode

Interactive mode allows scheduled jobs to conduct multi-turn conversations with the user after being triggered, configured via `InteractiveSpec`:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `reply_ttl_secs` | `86400` (24 hours) | Timeout for waiting for a user reply |
| `end_signal` | `"<<<CLAW_END>>>"` | Signal emitted by Codex to indicate the interaction has ended |
| `max_rounds_hard_cap` | `10` | Maximum number of back-and-forth rounds |

### Flow

```
+-----------------------------------------------------------+
|  1. Job reaches execution time, triggered with            |
|     interactive spec                                      |
|  2. Codex receives a prompt containing interaction        |
|     protocol instructions                                 |
|  3. User's current foreground session is suspended        |
|     to the background                                     |
|  4. A new foreground session is created in the job        |
|     workspace                                             |
|  5. User's subsequent QQ messages are routed to this      |
|     interaction thread                                    |
|  6. Codex and user converse back and forth                |
|     +-- until Codex emits end_signal or max_rounds        |
|         is reached                                        |
|  7. Interaction ends:                                     |
|     +-- Foreground session stops (in Persistent mode,     |
|     |   it switches to background)                        |
|     +-- User's original foreground session is restored    |
|  8. Start/end banner messages are sent to the user        |
+-----------------------------------------------------------+
```

---

## CLI Commands

All commands are invoked via the `codex-claw cron` subcommand.

### `cron add` -- Create a Recurring Job

```bash
codex-claw cron add \
  --owner OPENID \
  --cron "min hour dom month dow" \
  --tz Asia/Shanghai \
  --title "Daily News Digest" \
  --action codex-turn \
  --prompt "Collect today's important AI news and generate a summary" \
  --workspace /path/to/workspace \
  --model o3-mini \
  --session-strategy per-invocation \
  --approval auto-edit
```

All flags:

| Flag | Description |
|------|-------------|
| `--owner OPENID` | Job owner |
| `--cron EXPR` | 5-field cron expression (seconds will be auto-padded with 0) |
| `--tz TIMEZONE` | Timezone |
| `--title NAME` | Job name |
| `--action TYPE` | Action type: `reminder` / `codex-turn` / `codex-exec` / `shell` |
| `--message TEXT` | Message content for the `reminder` action |
| `--prompt TEXT` | Prompt for Codex actions |
| `--prompt-file PATH` | Read prompt from a file |
| `--workspace PATH` | Working directory |
| `--model MODEL` | Model name |
| `--session-strategy` | `per-invocation` or `persistent` |
| `--approval POLICY` | Approval policy |
| `--interactive` | Enable interactive mode |
| `--reply-ttl SECS` | Interactive mode: reply timeout in seconds |
| `--end-signal TEXT` | Interactive mode: end signal |
| `--max-rounds N` | Interactive mode: maximum rounds |
| `--program PATH` | Program path for the `shell` action |
| `--arg VALUE` | Argument for the `shell` action (can be specified multiple times) |
| `--extra-arg VALUE` | Extra argument for the `codex-exec` action |

### `cron once` -- Create a One-Shot Job

```bash
codex-claw cron once \
  --owner OPENID \
  --at "2026-05-12T10:00:00Z" \
  --title "Release Reminder" \
  --action reminder \
  --message "v2.0 release window is here, please check the release checklist"
```

Same flags as `cron add`, but uses `--at RFC3339_DATETIME` instead of `--cron`.

### `cron list` -- List Jobs

```bash
codex-claw cron list            # List all jobs
codex-claw cron list --owner ID # Filter by owner
```

Output fields: `id`, `enabled/disabled`, `next_run_at`, `run_count`, `failure_streak`, `owner`, `title`

### `cron rm <job_id>` -- Delete a Job

```bash
codex-claw cron rm 01J5K...     # Delete job and its files
codex-claw cron rm 01J5K... --keep-files  # Keep the working directory
```

### `cron pause <job_id>` -- Pause a Job

```bash
codex-claw cron pause 01J5K...
```

Sets `disabled = true`; the job will no longer be scheduled.

### `cron resume <job_id>` -- Resume a Job

```bash
codex-claw cron resume 01J5K...
```

Sets `disabled = false` and recalculates `next_run_at`.

### `cron run-now <job_id>` -- Execute Immediately

```bash
codex-claw cron run-now 01J5K...
```

Sets `run_now_at` to trigger an additional immediate execution, **without affecting** the normal `next_run_at` schedule.

### `cron tail <job_id>` -- View Execution Logs

```bash
codex-claw cron tail 01J5K...
```

Displays a summary of the job status and the content of the most recent execution log.

---

## QQ Commands

Users can manage their scheduled jobs in QQ via the following commands:

| Command | Description |
|---------|-------------|
| `/cron list` | List your own jobs |
| `/cron pause <id>` | Pause a job |
| `/cron resume <id>` | Resume a job |
| `/cron rm <id>` | Delete a job |
| `/cron run-now <id>` | Execute once immediately |
| `/cron tail <id>` | View the most recent execution log |

Chinese alias: `/定时` (equivalent to `/cron`)

---

## Directory Layout

```
data/
├── scheduler/
│   ├── jobs.json                  # Job table (protected by cross-process file lock)
│   └── pending-deliveries/        # Buffer for failed deliveries
│       └── <openid>.jsonl
├── cron-jobs/
│   └── <job_id>/
│       ├── job.toml               # Job metadata
│       ├── workspace/             # Execution directory
│       │   ├── .claw-job.json     # Job context
│       │   └── .agents/skills/    # Job-specific skills
│       ├── runs/                  # Execution logs (retains the most recent N)
│       │   └── 20260510T100000Z.log
│       └── pending.json           # Interactive job state (exists only when active)
└── cron-jobs-trash/               # Archive for completed one-shot jobs
    └── <timestamp>-<job_id>/
```

Each job's skill directory is symlinked to
`~/.codex-claw/.codex/skills/claw-cron-<job_id>`,
so that Codex can discover the job's dedicated skills when executing it.

---

## Lifecycle

```
Create --> Schedule --> Execute --> Success --> Update next_run_at --> Schedule (loop)
 |                   |                                       |
 |                   +--> Failure --> Retry --> Circuit Break --> Disable
 |
 +-- One-shot job --> Execute --> Archive to cron-jobs-trash/ ------+
```

### Detailed Stages

1. **Create**
   Triggered by the CLI `add` or `once` command. Writes to `jobs.json`, creates the directory structure, and registers the skill symlink.

2. **Schedule**
   The Scheduler tick (default every 30 seconds) scans the job table, checking for `next_run_at <= now` or `run_now_at` having been set.

3. **Execute**
   Acquires a semaphore permit and dispatches execution based on the `action` type. Each execution has a timeout guard (`max_turn_secs`).

4. **Retry**
   After an execution failure, retries at `retry_backoff_secs` intervals, up to `max_attempts` times.

5. **Circuit Breaker**
   When the consecutive failure count reaches `circuit_breaker_threshold`, the job is automatically disabled and the owner is notified.

6. **Archive**
   After a one-shot job completes execution, the entire job directory is moved into `cron-jobs-trash/`.

7. **Delivery Recovery**
   When result delivery fails, it is buffered to `pending-deliveries/<openid>.jsonl`.
   It is automatically retried the next time the user sends a QQ message.

---

## Best Practices

### Writing Dedicated Skills for Scheduled Jobs

For scheduled jobs with clear workflow characteristics such as news collection, market scanning, or paper summarization, the agent should first confirm the following elements with the user before creating the job:

- **Data sources**: Where to obtain information
- **Filtering rules**: What content should be included
- **Output format**: In what form the results should be presented
- **Failure strategy**: How to handle errors

After confirmation, write a job-specific skill into the job's `workspace/.agents/skills/` directory, so that Codex has clear working instructions for each execution.

**Do not** create a daily `codex-exec` job with just a single vague prompt.

### Choosing the Right Action Type

| Scenario | Recommended Action |
|----------|-------------------|
| Simple text reminders | `Reminder` |
| Complex tasks requiring AI understanding and generation | `CodexTurn` (preferred) |
| Scheduled jobs that need user interaction | `CodexTurn` + `--interactive` |
| Fixed script invocations | `Shell` |
| Legacy compatibility | `CodexExec` |

### Choosing a Session Strategy

- **PerInvocation** (default): Each execution is independent. Suitable for standalone repetitive tasks.
- **Persistent**: Preserves context across executions. Suitable for continuity tasks that need to remember history (e.g., tracking project progress).
