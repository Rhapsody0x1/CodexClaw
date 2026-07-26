use std::{process::Stdio, time::Instant};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use rust_i18n::t;
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::{mpsc, oneshot},
    time::{sleep, timeout},
};

use crate::{
    codex::{ExecutionRequest, ExecutionUpdate, agent_messages_from_stdout},
    session::state::{ApprovalPolicySetting, DialogProfile, SessionSettings, SessionState},
    util::{layout::DataLayout, text::truncate_middle},
};

use super::{
    cron_expr,
    ctx::SchedulerCtx,
    store::{
        CronJob, CronKind, DeliverPolicy, InteractiveSpec, JobAction, RunStatus, SessionStrategy,
        write_run_log,
    },
};

const MAX_LOG_OUTPUT_CHARS: usize = 64 * 1024;
const MAX_CODEX_EXEC_ERROR_CHARS: usize = 8 * 1024;

struct CodexTurnRun {
    prompt: String,
    model: Option<String>,
    session_state: Option<SessionState>,
    approval_policy: Option<ApprovalPolicySetting>,
    session_strategy: SessionStrategy,
    interactive: Option<InteractiveSpec>,
}

pub(crate) async fn run_job(
    ctx: std::sync::Arc<SchedulerCtx>,
    mut job: CronJob,
) -> Result<CronJob> {
    let started_at = Utc::now();
    let manual_run = job
        .run_now_at
        .is_some_and(|run_now_at| run_now_at <= started_at);
    let timer = Instant::now();
    let max_duration = std::time::Duration::from_secs(ctx.config.scheduler.max_turn_secs);
    let outcome = execute_with_retries(&ctx, &mut job, max_duration, timer).await;
    let next_run_at = compute_next_run(&job, manual_run)?;
    let job = commit_run_result(&ctx, job, &outcome, started_at, manual_run, next_run_at).await?;
    notify_circuit_breaker(&ctx, &job, &outcome.status).await;
    Ok(job)
}

/// What the attempt loop settled on: the final status plus the material for
/// the run log.
struct RetriesOutcome {
    status: RunStatus,
    attempt_logs: Vec<String>,
    output: String,
}

/// Run the job's action up to `max_attempts` times, retrying transient-looking
/// failures with exponential backoff (`retry_backoff_secs * 2^(attempt-1)`,
/// exponent capped at 5).
async fn execute_with_retries(
    ctx: &std::sync::Arc<SchedulerCtx>,
    job: &mut CronJob,
    max_duration: std::time::Duration,
    timer: Instant,
) -> RetriesOutcome {
    let max_attempts = ctx.config.scheduler.max_attempts.max(1);
    let mut attempt_logs = Vec::new();
    let mut final_output = String::new();
    let mut final_attempt = 1;
    let mut final_error = None;
    let mut success = false;

    for attempt in 1..=max_attempts {
        final_attempt = attempt;
        let attempt_started = Utc::now();
        let result = run_job_inner(ctx.clone(), job, max_duration).await;
        match result {
            Ok(output) => {
                final_output = output;
                success = true;
                attempt_logs.push(format!(
                    "attempt={attempt} started_at={} status=success duration_ms={}",
                    attempt_started.to_rfc3339(),
                    timer.elapsed().as_millis()
                ));
                break;
            }
            Err(err) => {
                let error = err.to_string();
                if is_interactive_job(job) {
                    super::interactive::finish_job(ctx, &job.id, "failed")
                        .await
                        .ok();
                }
                attempt_logs.push(format!(
                    "attempt={attempt} started_at={} status=failure error={error}",
                    attempt_started.to_rfc3339()
                ));
                let retry = attempt < max_attempts && is_retryable_error(&error);
                final_error = Some(error);
                if !retry {
                    break;
                }
            }
        }
        let multiplier = 1_u64 << (attempt - 1).min(5);
        sleep(std::time::Duration::from_secs(
            ctx.config
                .scheduler
                .retry_backoff_secs
                .saturating_mul(multiplier),
        ))
        .await;
    }

    let status = if success {
        RunStatus::Success {
            duration_ms: timer.elapsed().as_millis() as u64,
            output_chars: final_output.chars().count(),
        }
    } else {
        RunStatus::Failure {
            error: final_error.unwrap_or_else(|| "unknown scheduler failure".to_string()),
            attempt: final_attempt,
        }
    };
    RetriesOutcome {
        status,
        attempt_logs,
        output: final_output,
    }
}

/// A manual `run-now` keeps whatever was already scheduled; a scheduled
/// one-shot is done; a recurring job advances to its next occurrence.
fn compute_next_run(job: &CronJob, manual_run: bool) -> Result<Option<chrono::DateTime<Utc>>> {
    if manual_run {
        Ok(job.next_run_at)
    } else if matches!(job.kind, CronKind::OneShot { .. }) {
        Ok(None)
    } else {
        cron_expr::next_after(&job.kind, Utc::now())
    }
}

/// Persist the run's outcome into the job table (counters, failure streak,
/// circuit breaker, one-shot completion), write the run log, and recycle a
/// completed scheduled one-shot's files.
async fn commit_run_result(
    ctx: &std::sync::Arc<SchedulerCtx>,
    job: CronJob,
    outcome: &RetriesOutcome,
    started_at: chrono::DateTime<Utc>,
    manual_run: bool,
    next_run_at: Option<chrono::DateTime<Utc>>,
) -> Result<CronJob> {
    let scheduled_one_shot_complete = !manual_run && matches!(job.kind, CronKind::OneShot { .. });
    let circuit_breaker_threshold = ctx.config.scheduler.circuit_breaker_threshold.max(1);
    let job = ctx
        .session
        .update_cron_job(&job.id, {
            let status = outcome.status.clone();
            let action = job.action.clone();
            move |current| {
                if let (JobAction::CodexTurn { session_state, .. }, JobAction::CodexTurn { .. }) =
                    (&action, &current.action)
                    && let JobAction::CodexTurn {
                        session_state: current_session_state,
                        ..
                    } = &mut current.action
                {
                    *current_session_state = session_state.clone();
                }
                current.run_count += 1;
                current.last_run_at = Some(started_at);
                current.last_run_status = Some(status.clone());
                current.failure_streak = match status {
                    RunStatus::Success { .. } => 0,
                    RunStatus::Failure { .. } => current.failure_streak.saturating_add(1),
                    RunStatus::Skipped { .. } => current.failure_streak,
                };
                current.run_now_at = None;
                let keep_disabled = manual_run && current.disabled;
                current.disabled = keep_disabled
                    || scheduled_one_shot_complete
                    || (matches!(status, RunStatus::Failure { .. })
                        && current.failure_streak >= circuit_breaker_threshold);
                current.next_run_at = if current.disabled { None } else { next_run_at };
                Ok(())
            }
        })
        .await?
        .unwrap_or(job.clone());

    let log = format_run_log(
        &job,
        started_at,
        &outcome.status,
        &outcome.attempt_logs,
        &outcome.output,
    );
    write_run_log(&job, started_at, &log, ctx.config.scheduler.runs_retention)
        .await
        .ok();
    if scheduled_one_shot_complete {
        super::store::recycle_job_files(
            &ctx.config.general.data_dir,
            &ctx.config.general.codex_home_global,
            &job.id,
        )
        .await
        .ok();
    }
    Ok(job)
}

/// If this run's failure tripped the circuit breaker (the commit above just
/// disabled the job), tell the owner; a failed queue write is swallowed here
/// because the run result itself is already persisted.
async fn notify_circuit_breaker(ctx: &SchedulerCtx, job: &CronJob, status: &RunStatus) {
    let circuit_breaker_threshold = ctx.config.scheduler.circuit_breaker_threshold.max(1);
    if !(matches!(status, RunStatus::Failure { .. })
        && job.disabled
        && job.failure_streak >= circuit_breaker_threshold)
    {
        return;
    }
    let lang = ctx.session.command_locale(&job.owner_openid).await;
    let text = t!(
        "scheduler.failure.disabled",
        title = job.title.as_str(),
        count = job.failure_streak,
        locale = lang.as_str()
    )
    .into_owned();
    push_or_queue(ctx, &job.owner_openid, &job.id, &job.title, text)
        .await
        .ok();
}

async fn run_job_inner(
    ctx: std::sync::Arc<SchedulerCtx>,
    job: &mut CronJob,
    max_duration: std::time::Duration,
) -> Result<String> {
    match job.action.clone() {
        JobAction::Reminder { message } => {
            deliver(&ctx, job, &message).await?;
            Ok(message)
        }
        JobAction::Shell { program, args, env } => {
            let output = run_shell(&program, &args, &env, &job.workspace_dir, max_duration).await?;
            deliver(&ctx, job, &output).await?;
            Ok(output)
        }
        JobAction::CodexExec {
            prompt,
            model,
            extra_args,
            env,
        } => {
            let output = run_codex_exec(
                &ctx,
                job,
                &prompt,
                model.as_deref(),
                &extra_args,
                &env,
                max_duration,
            )
            .await?;
            deliver(&ctx, job, &output).await?;
            Ok(output)
        }
        JobAction::CodexTurn {
            prompt,
            model,
            session_state,
            approval_policy,
            session_strategy,
            interactive,
        } => {
            let output = run_codex_turn(
                &ctx,
                job,
                CodexTurnRun {
                    prompt,
                    model,
                    session_state,
                    approval_policy,
                    session_strategy,
                    interactive,
                },
                max_duration,
            )
            .await?;
            deliver(&ctx, job, &output).await?;
            Ok(output)
        }
    }
}

async fn run_shell(
    program: &str,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
    cwd: &std::path::Path,
    max_duration: std::time::Duration,
) -> Result<String> {
    let mut child = Command::new(program);
    child
        .args(args)
        .envs(env)
        .current_dir(cwd)
        .kill_on_drop(true);
    let output = timeout(max_duration, child.output())
        .await
        .map_err(|_| anyhow!("timed out after {}s", max_duration.as_secs()))?
        .with_context(|| format!("failed to execute `{program}`"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stderr.trim().is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n[stderr]\n{stderr}")
    };
    if !output.status.success() {
        return Err(anyhow!(
            "process exited with {}: {}",
            output.status,
            combined.trim()
        ));
    }
    Ok(combined)
}

async fn run_codex_exec(
    ctx: &SchedulerCtx,
    job: &CronJob,
    prompt: &str,
    model: Option<&str>,
    extra_args: &[String],
    env: &std::collections::BTreeMap<String, String>,
    max_duration: std::time::Duration,
) -> Result<String> {
    let mut cmd = Command::new(&ctx.config.general.codex_binary);
    cmd.args(codex_exec_args(model, extra_args, prompt));
    cmd.env("CODEX_HOME", &ctx.config.general.codex_home_global);
    cmd.envs(env);
    cmd.current_dir(&job.workspace_dir);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().context("failed to spawn codex exec")?;
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_task = tokio::spawn(async move {
        let mut stdout = Vec::new();
        if let Some(out) = stdout_pipe.as_mut() {
            out.read_to_end(&mut stdout).await?;
        }
        Ok::<_, std::io::Error>(stdout)
    });
    let stderr_task = tokio::spawn(async move {
        let mut stderr = Vec::new();
        if let Some(err) = stderr_pipe.as_mut() {
            err.read_to_end(&mut stderr).await?;
        }
        Ok::<_, std::io::Error>(stderr)
    });
    let status = match timeout(max_duration, child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            child.kill().await.ok();
            child.wait().await.ok();
            stdout_task.abort();
            stderr_task.abort();
            return Err(anyhow!("timed out after {}s", max_duration.as_secs()));
        }
    };
    let stdout = stdout_task
        .await
        .context("failed to join codex exec stdout reader")?
        .context("failed to read codex exec stdout")?;
    let stderr = stderr_task
        .await
        .context("failed to join codex exec stderr reader")?
        .context("failed to read codex exec stderr")?;
    let combined = format!(
        "{}{}{}",
        String::from_utf8_lossy(&stdout),
        if stderr.is_empty() {
            ""
        } else {
            "\n[stderr]\n"
        },
        String::from_utf8_lossy(&stderr)
    );
    if !status.success() {
        return Err(anyhow!(
            "codex exec exited with {status}: {}",
            truncate_middle(combined.trim(), MAX_CODEX_EXEC_ERROR_CHARS)
        ));
    }
    let agent_output = agent_messages_from_stdout(&stdout);
    if agent_output.trim().is_empty() {
        Ok(String::from_utf8_lossy(&stdout).trim().to_string())
    } else {
        Ok(agent_output)
    }
}

fn codex_exec_args(model: Option<&str>, extra_args: &[String], prompt: &str) -> Vec<String> {
    let mut args = vec!["exec".to_string()];
    if !extra_args.iter().any(|arg| arg == "--skip-git-repo-check") {
        args.push("--skip-git-repo-check".to_string());
    }
    if !extra_args.iter().any(|arg| arg == "--json") {
        args.push("--json".to_string());
    }
    if let Some(model) = model {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    args.extend(extra_args.iter().cloned());
    args.push(prompt.to_string());
    args
}

async fn run_codex_turn(
    ctx: &SchedulerCtx,
    job: &mut CronJob,
    run: CodexTurnRun,
    max_duration: std::time::Duration,
) -> Result<String> {
    if let Some(spec) = run.interactive.as_ref() {
        super::interactive::prepare_foreground(ctx, job, spec).await?;
    }
    write_scheduler_turn_context(&job.workspace_dir, &job.owner_openid, &job.id)
        .await
        .ok();
    let settings = SessionSettings {
        approval_policy_override: Some(run.approval_policy.unwrap_or(ApprovalPolicySetting::Never)),
        ..Default::default()
    };
    let mut state = match run.session_strategy {
        SessionStrategy::Persistent => run.session_state.unwrap_or(SessionState {
            session_id: None,
            settings: settings.clone(),
        }),
        SessionStrategy::PerInvocation => SessionState {
            session_id: None,
            settings: settings.clone(),
        },
    };
    state.settings.approval_policy_override =
        Some(run.approval_policy.unwrap_or(ApprovalPolicySetting::Never));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let prompt_text = if let Some(spec) = run.interactive.as_ref() {
        super::interactive::build_protocol_prompt(&job.owner_openid, &job.title, &run.prompt, spec)
    } else {
        run.prompt.clone()
    };
    let request = ExecutionRequest {
        prompt: prompt_text,
        workspace_dir: job.workspace_dir.clone(),
        codex_home: ctx.config.general.codex_home_global.clone(),
        config_overrides: Vec::new(),
        add_dirs: scheduler_add_dirs(ctx),
        session_state: state,
        model: run
            .model
            .clone()
            .or_else(|| Some(ctx.config.general.default_model.clone())),
        service_tier: None,
        context_mode: None,
        reasoning_effort: ctx.config.general.default_reasoning_effort,
        image_paths: Vec::new(),
    };
    let codex = ctx.codex.clone();
    let handle =
        tokio::spawn(async move { codex.execute(request, Some(cancel_rx), Some(tx)).await });
    let execution = timeout(max_duration, handle).await;
    let execution = match execution {
        Ok(result) => result?,
        Err(_) => {
            let _ = cancel_tx.send(());
            keep_interrupted_thread(job, run.session_strategy, &mut rx);
            if run.interactive.is_some() {
                super::interactive::finish_job(ctx, &job.id, "timed_out")
                    .await
                    .ok();
            }
            return Err(anyhow!("timed out after {}s", max_duration.as_secs()));
        }
    };
    let execution = match execution {
        Ok(execution) => execution,
        Err(err) => {
            keep_interrupted_thread(job, run.session_strategy, &mut rx);
            return Err(err);
        }
    };
    let mut streamed = String::new();
    while let Ok(update) = rx.try_recv() {
        if let ExecutionUpdate::AgentMessage { text } = update {
            streamed.push_str(&text);
            streamed.push('\n');
        }
    }
    if let JobAction::CodexTurn { session_state, .. } = &mut job.action
        && run.session_strategy == SessionStrategy::Persistent
    {
        *session_state = Some(SessionState {
            session_id: execution.session_id.clone(),
            settings: SessionSettings::default(),
        });
    }
    if run.interactive.is_some() {
        super::interactive::update_pending_session(
            &ctx.config.general.data_dir,
            &job.id,
            execution.session_id.clone(),
        )
        .await?;
        ctx.session
            .bind_foreground_session_profile(
                &job.owner_openid,
                execution.session_id.clone(),
                DialogProfile {
                    model_override: run
                        .model
                        .clone()
                        .or_else(|| Some(ctx.config.general.default_model.clone())),
                    reasoning_effort: Some(ctx.config.general.default_reasoning_effort),
                    service_tier: None,
                    context_mode: None,
                },
            )
            .await?;
    }
    let output = if streamed.trim().is_empty() {
        execution.text
    } else {
        streamed
    };
    if run.interactive.is_some() {
        super::interactive::finish_if_needed_after_scheduler_turn(ctx, job, &output).await
    } else {
        Ok(output)
    }
}

/// A turn that failed or timed out mid-flight has usually already announced
/// its thread id on the update stream (SessionStarted). For a Persistent job,
/// fold that id back into the job's session state before bailing out, so the
/// next run (or retry) resumes the same thread instead of starting fresh and
/// dropping the job's accumulated context. Leaves the state untouched when no
/// thread was established.
fn keep_interrupted_thread(
    job: &mut CronJob,
    strategy: SessionStrategy,
    rx: &mut mpsc::UnboundedReceiver<ExecutionUpdate>,
) {
    if strategy != SessionStrategy::Persistent {
        return;
    }
    let Some(session_id) = drain_session_started(rx) else {
        return;
    };
    if let JobAction::CodexTurn { session_state, .. } = &mut job.action {
        *session_state = Some(SessionState {
            session_id: Some(session_id),
            settings: SessionSettings::default(),
        });
    }
}

fn drain_session_started(rx: &mut mpsc::UnboundedReceiver<ExecutionUpdate>) -> Option<String> {
    let mut found = None;
    while let Ok(update) = rx.try_recv() {
        if let ExecutionUpdate::SessionStarted { session_id } = update {
            found = Some(session_id);
        }
    }
    found
}

async fn deliver(ctx: &SchedulerCtx, job: &CronJob, output: &str) -> Result<()> {
    let payload = match job.deliver {
        DeliverPolicy::LogOnly => None,
        DeliverPolicy::PushIfNonEmpty if output.trim().is_empty() => None,
        DeliverPolicy::PushTruncated { max_chars } => Some(if output.chars().count() > max_chars {
            output.chars().take(max_chars).collect::<String>()
        } else {
            output.to_string()
        }),
        DeliverPolicy::PushToOwner | DeliverPolicy::PushIfNonEmpty => Some(output.to_string()),
    };
    let Some(text) = payload else {
        return Ok(());
    };
    push_or_queue(ctx, &job.owner_openid, &job.id, &job.title, text).await
}

/// Push `text` to the owner proactively; when the QQ send fails, queue it as a
/// `PendingDelivery` to be replayed on the user's next turn. The returned
/// `Result` is the queueing outcome: callers deliberately differ on whether
/// they propagate it (`?`) or swallow it (`.ok()`).
pub(crate) async fn push_or_queue(
    ctx: &SchedulerCtx,
    openid: &str,
    job_id: &str,
    title: &str,
    text: String,
) -> Result<()> {
    match ctx.notifier.send_markdown_proactive(openid, &text).await {
        Ok(()) => Ok(()),
        Err(err) => {
            let error = err.to_string();
            super::store::queue_pending_delivery(
                &ctx.config.general.data_dir,
                openid,
                &super::store::PendingDelivery {
                    job_id: job_id.to_string(),
                    title: title.to_string(),
                    text,
                    failed_at: Utc::now(),
                    error,
                },
            )
            .await
        }
    }
}

fn scheduler_add_dirs(ctx: &SchedulerCtx) -> Vec<std::path::PathBuf> {
    let mut dirs = vec![ctx.session.inbox_dir().to_path_buf()];
    dirs.extend(DataLayout::new(&ctx.config.general.data_dir).turn_add_dirs());
    dirs
}

fn is_interactive_job(job: &CronJob) -> bool {
    matches!(
        job.action,
        JobAction::CodexTurn {
            interactive: Some(_),
            ..
        }
    )
}

async fn write_scheduler_turn_context(
    workspace_dir: &std::path::Path,
    owner_openid: &str,
    job_id: &str,
) -> Result<()> {
    tokio::fs::create_dir_all(workspace_dir).await?;
    let raw = serde_json::json!({
        "owner_openid": owner_openid,
        "openid": owner_openid,
        "scheduler_job_id": job_id,
    });
    tokio::fs::write(
        workspace_dir.join(".claw-turn.json"),
        serde_json::to_string_pretty(&raw)?,
    )
    .await?;
    Ok(())
}

fn is_retryable_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "timed out",
        "timeout",
        "temporarily",
        "connection",
        "reset",
        "refused",
        "broken pipe",
        "service unavailable",
        "rate limit",
        " 429",
        " 500",
        " 502",
        " 503",
        " 504",
        "exit status: 137",
        "signal: 9",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

fn format_run_log(
    job: &CronJob,
    started_at: chrono::DateTime<Utc>,
    status: &RunStatus,
    attempts: &[String],
    output: &str,
) -> String {
    let mut log = String::new();
    log.push_str(&format!("job_id={}\n", job.id));
    log.push_str(&format!("title={}\n", job.title));
    log.push_str(&format!("started_at={}\n", started_at.to_rfc3339()));
    log.push_str(&format!("status={status:?}\n"));
    log.push_str("\n[attempts]\n");
    for attempt in attempts {
        log.push_str(attempt);
        log.push('\n');
    }
    if !output.is_empty() {
        log.push_str("\n[output]\n");
        log.push_str(&truncate_middle(output, MAX_LOG_OUTPUT_CHARS));
        log.push('\n');
    }
    log
}

#[cfg(test)]
mod tests {
    use super::{agent_messages_from_stdout, codex_exec_args, keep_interrupted_thread};
    use crate::codex::ExecutionUpdate;
    use crate::model::cron::fixtures::shell_job;
    use crate::scheduler::store::{CronJob, JobAction, SessionStrategy};
    use crate::session::state::SessionState;
    use chrono::Utc;
    use tokio::sync::mpsc;

    fn codex_turn_job(strategy: SessionStrategy) -> CronJob {
        let mut job = shell_job("job-1", std::path::PathBuf::from("/tmp"), Utc::now());
        job.action = JobAction::CodexTurn {
            prompt: "p".to_string(),
            model: None,
            session_state: None,
            approval_policy: None,
            session_strategy: strategy,
            interactive: None,
        };
        job.next_run_at = None;
        job
    }

    fn job_session_id(job: &CronJob) -> Option<String> {
        match &job.action {
            JobAction::CodexTurn { session_state, .. } => session_state
                .as_ref()
                .and_then(|state| state.session_id.clone()),
            _ => None,
        }
    }

    #[test]
    fn keep_interrupted_thread_folds_session_started_into_persistent_job() {
        let mut job = codex_turn_job(SessionStrategy::Persistent);
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(ExecutionUpdate::SessionStarted {
            session_id: "thread-abc".to_string(),
        })
        .unwrap();
        tx.send(ExecutionUpdate::ToolCall {
            display: "[Tool: Bash]".to_string(),
        })
        .unwrap();
        drop(tx);

        keep_interrupted_thread(&mut job, SessionStrategy::Persistent, &mut rx);

        assert_eq!(job_session_id(&job).as_deref(), Some("thread-abc"));
    }

    #[test]
    fn keep_interrupted_thread_leaves_state_without_session_started() {
        let mut job = codex_turn_job(SessionStrategy::Persistent);
        if let JobAction::CodexTurn { session_state, .. } = &mut job.action {
            *session_state = Some(SessionState {
                session_id: Some("previous-thread".to_string()),
                settings: Default::default(),
            });
        }
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(ExecutionUpdate::ToolCall {
            display: "[Tool: Bash]".to_string(),
        })
        .unwrap();
        drop(tx);

        keep_interrupted_thread(&mut job, SessionStrategy::Persistent, &mut rx);

        // No thread was established this run: the previous one stays bound.
        assert_eq!(job_session_id(&job).as_deref(), Some("previous-thread"));
    }

    #[test]
    fn keep_interrupted_thread_ignores_per_invocation_jobs() {
        let mut job = codex_turn_job(SessionStrategy::PerInvocation);
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(ExecutionUpdate::SessionStarted {
            session_id: "thread-abc".to_string(),
        })
        .unwrap();
        drop(tx);

        keep_interrupted_thread(&mut job, SessionStrategy::PerInvocation, &mut rx);

        assert_eq!(job_session_id(&job), None);
    }

    #[test]
    fn codex_exec_args_include_git_repo_check_skip_and_json_by_default() {
        let extra_args = vec!["--output-schema".to_string(), "{}".to_string()];

        let args = codex_exec_args(Some("gpt-5.5"), &extra_args, "hello");

        assert_eq!(
            args,
            vec![
                "exec",
                "--skip-git-repo-check",
                "--json",
                "--model",
                "gpt-5.5",
                "--output-schema",
                "{}",
                "hello"
            ]
        );
    }

    #[test]
    fn codex_exec_args_do_not_duplicate_explicit_git_repo_check_skip_or_json() {
        let extra_args = vec![
            "--skip-git-repo-check".to_string(),
            "--json".to_string(),
            "--output-schema".to_string(),
            "{}".to_string(),
        ];

        let args = codex_exec_args(None, &extra_args, "hello");

        assert_eq!(
            args,
            vec![
                "exec",
                "--skip-git-repo-check",
                "--json",
                "--output-schema",
                "{}",
                "hello"
            ]
        );
        assert_eq!(
            args.iter()
                .filter(|arg| arg.as_str() == "--skip-git-repo-check")
                .count(),
            1
        );
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == "--json").count(),
            1
        );
    }

    #[test]
    fn codex_exec_stdout_extraction_ignores_events_and_stderr_noise() {
        let stdout = r#"{"type":"thread.started","thread_id":"x"}
{"type":"item.completed","item":{"id":"a","type":"reasoning","text":"hidden"}}
{"type":"item.completed","item":{"id":"b","type":"agent_message","text":"早餐正文"}}
not json
{"type":"turn.completed"}
"#;

        assert_eq!(
            agent_messages_from_stdout(stdout.as_bytes()),
            "早餐正文".to_string()
        );
    }
}
