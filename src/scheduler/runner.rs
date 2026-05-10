use std::{process::Stdio, time::Instant};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use tokio::{io::AsyncReadExt, process::Command, sync::mpsc, time::timeout};

use crate::{
    app::App,
    codex::executor::{ExecutionRequest, ExecutionUpdate},
    session::state::{ApprovalPolicySetting, SessionSettings, SessionState},
};

use super::{
    cron_expr,
    store::{
        CronJob, CronKind, DeliverPolicy, JobAction, RunStatus, SessionStrategy, write_run_log,
    },
};

pub async fn run_job(app: std::sync::Arc<App>, mut job: CronJob) -> Result<CronJob> {
    let started_at = Utc::now();
    let timer = Instant::now();
    let max_duration = std::time::Duration::from_secs(app.config.scheduler.max_turn_secs);
    let result = timeout(max_duration, run_job_inner(app.clone(), &mut job)).await;
    let status = match result {
        Ok(Ok(output)) => RunStatus::Success {
            duration_ms: timer.elapsed().as_millis() as u64,
            output_chars: output.chars().count(),
        },
        Ok(Err(err)) => RunStatus::Failure {
            error: err.to_string(),
            attempt: 1,
        },
        Err(_) => RunStatus::Failure {
            error: format!("timed out after {}s", app.config.scheduler.max_turn_secs),
            attempt: 1,
        },
    };
    job.run_count += 1;
    job.last_run_at = Some(started_at);
    job.last_run_status = Some(status.clone());
    job.failure_streak = match status {
        RunStatus::Success { .. } => 0,
        RunStatus::Failure { .. } => job.failure_streak.saturating_add(1),
        RunStatus::Skipped { .. } => job.failure_streak,
    };
    if matches!(job.kind, CronKind::OneShot { .. }) {
        job.disabled = true;
        job.next_run_at = None;
    } else {
        job.next_run_at = cron_expr::next_after(&job.kind, Utc::now())?;
    }
    app.session.upsert_cron_job(job.clone()).await?;
    Ok(job)
}

async fn run_job_inner(app: std::sync::Arc<App>, job: &mut CronJob) -> Result<String> {
    match job.action.clone() {
        JobAction::Reminder { message } => {
            deliver(&app, job, &message).await?;
            write_run_log(job, Utc::now(), &message).await.ok();
            Ok(message)
        }
        JobAction::Shell { program, args, env } => {
            let output = run_shell(&program, &args, &env, &job.workspace_dir).await?;
            deliver(&app, job, &output).await?;
            write_run_log(job, Utc::now(), &output).await.ok();
            Ok(output)
        }
        JobAction::CodexExec {
            prompt,
            model,
            extra_args,
            env,
        } => {
            let output =
                run_codex_exec(&app, job, &prompt, model.as_deref(), &extra_args, &env).await?;
            deliver(&app, job, &output).await?;
            write_run_log(job, Utc::now(), &output).await.ok();
            Ok(output)
        }
        JobAction::CodexTurn {
            prompt,
            model,
            session_state,
            session_strategy,
        } => {
            let output =
                run_codex_turn(&app, job, &prompt, model, session_state, session_strategy).await?;
            deliver(&app, job, &output).await?;
            write_run_log(job, Utc::now(), &output).await.ok();
            Ok(output)
        }
    }
}

async fn run_shell(
    program: &str,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
    cwd: &std::path::Path,
) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .envs(env)
        .current_dir(cwd)
        .output()
        .await
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
    app: &App,
    job: &CronJob,
    prompt: &str,
    model: Option<&str>,
    extra_args: &[String],
    env: &std::collections::BTreeMap<String, String>,
) -> Result<String> {
    let mut cmd = Command::new(&app.config.general.codex_binary);
    cmd.arg("exec");
    if let Some(model) = model {
        cmd.arg("--model").arg(model);
    }
    cmd.args(extra_args);
    cmd.arg(prompt);
    cmd.env("CODEX_HOME", &app.config.general.codex_home_global);
    cmd.envs(env);
    cmd.current_dir(&job.workspace_dir);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().context("failed to spawn codex exec")?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_end(&mut stdout).await.ok();
    }
    if let Some(mut err) = child.stderr.take() {
        err.read_to_end(&mut stderr).await.ok();
    }
    let status = child.wait().await?;
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
            combined.trim()
        ));
    }
    Ok(combined)
}

async fn run_codex_turn(
    app: &App,
    job: &mut CronJob,
    prompt: &str,
    model: Option<String>,
    session_state: Option<SessionState>,
    session_strategy: SessionStrategy,
) -> Result<String> {
    let settings = SessionSettings {
        approval_policy_override: Some(ApprovalPolicySetting::Never),
        ..Default::default()
    };
    let mut state = match session_strategy {
        SessionStrategy::Persistent => session_state.unwrap_or(SessionState {
            session_id: None,
            settings: settings.clone(),
        }),
        SessionStrategy::PerInvocation => SessionState {
            session_id: None,
            settings: settings.clone(),
        },
    };
    state.settings.approval_policy_override = Some(ApprovalPolicySetting::Never);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let execution = app
        .codex
        .execute(
            ExecutionRequest {
                prompt: prompt.to_string(),
                workspace_dir: job.workspace_dir.clone(),
                codex_home: app.config.general.codex_home_global.clone(),
                config_overrides: Vec::new(),
                add_dirs: Vec::new(),
                session_state: state,
                model: model.or_else(|| Some(app.config.general.default_model.clone())),
                service_tier: None,
                context_mode: None,
                reasoning_effort: app.config.general.default_reasoning_effort,
                image_paths: Vec::new(),
            },
            None,
            Some(tx),
        )
        .await?;
    let mut streamed = String::new();
    while let Ok(update) = rx.try_recv() {
        if let ExecutionUpdate::AgentMessage { text } = update {
            streamed.push_str(&text);
            streamed.push('\n');
        }
    }
    if let JobAction::CodexTurn { session_state, .. } = &mut job.action
        && session_strategy == SessionStrategy::Persistent
    {
        *session_state = Some(SessionState {
            session_id: execution.session_id.clone(),
            settings: SessionSettings::default(),
        });
    }
    Ok(if streamed.trim().is_empty() {
        execution.text
    } else {
        streamed
    })
}

async fn deliver(app: &App, job: &CronJob, output: &str) -> Result<()> {
    match job.deliver {
        DeliverPolicy::LogOnly => Ok(()),
        DeliverPolicy::PushIfNonEmpty if output.trim().is_empty() => Ok(()),
        DeliverPolicy::PushTruncated { max_chars } => {
            let text = if output.chars().count() > max_chars {
                output.chars().take(max_chars).collect::<String>()
            } else {
                output.to_string()
            };
            app.qq_client
                .send_text_proactive(&job.owner_openid, &text)
                .await
        }
        DeliverPolicy::PushToOwner | DeliverPolicy::PushIfNonEmpty => {
            app.qq_client
                .send_text_proactive(&job.owner_openid, output)
                .await
        }
    }
}
