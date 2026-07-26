use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::app::App;
use crate::util::{fs::read_json_opt_async, layout::DataLayout};

use super::store::{CronJob, InteractiveSpec, JobAction, SessionStrategy, new_job_dir};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingInteraction {
    pub job_id: String,
    pub title: String,
    pub owner_openid: String,
    pub codex_session_id: Option<String>,
    pub workspace_dir: PathBuf,
    pub end_signal: String,
    pub rounds_done: u32,
    pub max_rounds_hard_cap: u32,
    pub expires_at: DateTime<Utc>,
    pub parked_fg_alias: Option<String>,
    pub cron_fg_alias: String,
    pub session_strategy: SessionStrategy,
}

pub fn build_protocol_prompt(
    owner_openid: &str,
    title: &str,
    prompt: &str,
    spec: &InteractiveSpec,
) -> String {
    format!(
        "[CLAW SCHEDULED INTERACTIVE TASK]\n\
You are running scheduled task `{title}` on behalf of user openid={owner_openid}. \
codex-claw has made this scheduled task the user's foreground conversation. \
The user's next QQ messages will be routed to this same thread.\n\n\
When you decide the interaction is complete, end your final reply with the literal token \
`{}` on its own line. codex-claw will detect it and restore the user's previous foreground conversation. \
Do not emit the token until the interaction is truly complete. Hard cap: at most {} user replies.\n\n\
{}\n",
        spec.end_signal, spec.max_rounds_hard_cap, prompt
    )
}

pub fn strip_end_signal(text: &str, signal: &str) -> (String, bool) {
    if !text.contains(signal) {
        return (text.to_string(), false);
    }
    let stripped = text
        .lines()
        .filter(|line| line.trim() != signal)
        .collect::<Vec<_>>()
        .join("\n")
        .replace(signal, "")
        .trim()
        .to_string();
    (stripped, true)
}

pub async fn prepare_foreground(
    app: &App,
    job: &CronJob,
    spec: &InteractiveSpec,
) -> Result<PendingInteraction> {
    let switched = app
        .session
        .new_foreground_in_workspace(&job.owner_openid, &job.workspace_dir)
        .await?;
    let cron_fg_alias = format!(
        "cron-{}-{}",
        slug(&job.title),
        &job.id[..job.id.len().min(8)]
    );
    let pending = PendingInteraction {
        job_id: job.id.clone(),
        title: job.title.clone(),
        owner_openid: job.owner_openid.clone(),
        codex_session_id: None,
        workspace_dir: job.workspace_dir.clone(),
        end_signal: spec.end_signal.clone(),
        rounds_done: 0,
        max_rounds_hard_cap: spec.max_rounds_hard_cap,
        expires_at: Utc::now() + Duration::seconds(spec.reply_ttl_secs as i64),
        parked_fg_alias: switched.parked_alias,
        cron_fg_alias,
        session_strategy: match &job.action {
            JobAction::CodexTurn {
                session_strategy, ..
            } => *session_strategy,
            _ => SessionStrategy::PerInvocation,
        },
    };
    write_pending(&app.config.general.data_dir, &pending).await?;
    let lang = app.command_locale(&job.owner_openid).await;
    let locale = lang.as_str();
    let banner = if let Some(alias) = pending.parked_fg_alias.as_deref() {
        t!(
            "scheduler.interactive.started_with_parked",
            title = job.title.as_str(),
            alias = alias,
            locale = locale
        )
        .into_owned()
    } else {
        t!(
            "scheduler.interactive.started",
            title = job.title.as_str(),
            locale = locale
        )
        .into_owned()
    };
    if let Err(err) = app
        .qq_client
        .send_markdown_proactive(&job.owner_openid, &banner)
        .await
    {
        super::store::queue_pending_delivery(
            &app.config.general.data_dir,
            &job.owner_openid,
            &super::store::PendingDelivery {
                job_id: job.id.clone(),
                title: job.title.clone(),
                text: banner,
                failed_at: Utc::now(),
                error: err.to_string(),
            },
        )
        .await?;
    }
    Ok(pending)
}

pub async fn update_pending_session(
    data_dir: &Path,
    job_id: &str,
    session_id: Option<String>,
) -> Result<()> {
    let Some(mut pending) = read_pending(data_dir, job_id).await? else {
        return Ok(());
    };
    pending.codex_session_id = session_id;
    write_pending(data_dir, &pending).await
}

pub async fn finish_if_needed_after_scheduler_turn(
    app: &App,
    job: &CronJob,
    output: &str,
) -> Result<String> {
    let Some(spec) = interactive_spec(job) else {
        return Ok(output.to_string());
    };
    let (stripped, ended) = strip_end_signal(output, &spec.end_signal);
    if ended {
        finish_job(app, &job.id, "ended").await?;
    }
    Ok(stripped)
}

pub async fn on_fg_turn_completed(app: &App, openid: &str, assistant_text: &str) -> Result<()> {
    let mut pending = match pending_for_owner(&app.config.general.data_dir, openid).await? {
        Some(pending) => pending,
        None => return Ok(()),
    };
    if let Some(expected_session_id) = pending.codex_session_id.as_deref() {
        let snapshot = app.session.snapshot_for_user(openid).await?;
        if snapshot.foreground.session_id.as_deref() != Some(expected_session_id) {
            return Ok(());
        }
    }
    pending.rounds_done = pending.rounds_done.saturating_add(1);
    let ended = assistant_text.contains(&pending.end_signal)
        || pending.rounds_done >= pending.max_rounds_hard_cap;
    if ended {
        finish_job(app, &pending.job_id, "ended").await?;
    } else {
        write_pending(&app.config.general.data_dir, &pending).await?;
    }
    Ok(())
}

pub async fn finish_job_for_owner(app: &App, openid: &str, reason: &str) -> Result<()> {
    let Some(pending) = pending_for_owner(&app.config.general.data_dir, openid).await? else {
        return Ok(());
    };
    finish_job(app, &pending.job_id, reason).await
}

pub async fn sweep_expired(app: &App) -> Result<()> {
    let root = DataLayout::new(&app.config.general.data_dir).cron_jobs_dir();
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", root.display())),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path().join("pending.json");
        let raw = match tokio::fs::read_to_string(&path).await {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                warn!(path = %path.display(), error = %err, "failed to read pending interaction");
                continue;
            }
        };
        let pending = match serde_json::from_str::<PendingInteraction>(&raw) {
            Ok(pending) => pending,
            Err(err) => {
                warn!(path = %path.display(), error = %err, "failed to parse pending interaction");
                continue;
            }
        };
        if pending.expires_at <= Utc::now() {
            finish_job(app, &pending.job_id, "no_answer").await?;
        }
    }
    Ok(())
}

pub async fn finish_job(app: &App, job_id: &str, reason: &str) -> Result<()> {
    let Some(pending) = read_pending(&app.config.general.data_dir, job_id).await? else {
        return Ok(());
    };
    // session_strategy only controls whether the codex thread id is kept on the
    // job for the next run (see runner.rs). It must not park the finished cron
    // dialog into the user's background list (Issue #5: /status showed a
    // lingering cron-*-history entry after "stopped").
    // Persistent still needs the on-disk rollout for resume, so mark the dialog
    // saved before stop_foreground; otherwise an unsaved Local session would be
    // pruned and the next invocation could not reopen the stored thread id.
    if pending.session_strategy == SessionStrategy::Persistent {
        let _ = app.session.save_foreground(&pending.owner_openid).await;
    }
    let _ = app.session.stop_foreground(&pending.owner_openid).await;
    if let Some(alias) = pending.parked_fg_alias.as_deref() {
        let _ = app
            .session
            .foreground_from_background(&pending.owner_openid, alias)
            .await;
    }
    remove_pending(&app.config.general.data_dir, job_id).await?;
    let lang = app.command_locale(&pending.owner_openid).await;
    let locale = lang.as_str();
    let suffix = if let Some(alias) = pending.parked_fg_alias.as_deref() {
        t!(
            "scheduler.interactive.restored",
            alias = alias,
            locale = locale
        )
        .into_owned()
    } else {
        t!("scheduler.interactive.blank", locale = locale).into_owned()
    };
    // Three banner shapes: a normal end and a manual stop read as plain
    // statements; only genuinely abnormal endings surface a reason.
    let text = match reason {
        "ended" => t!(
            "scheduler.interactive.ended",
            title = pending.title.as_str(),
            suffix = suffix.as_str(),
            locale = locale
        )
        .into_owned(),
        "stopped" => t!(
            "scheduler.interactive.stopped",
            title = pending.title.as_str(),
            suffix = suffix.as_str(),
            locale = locale
        )
        .into_owned(),
        abnormal => {
            let reason = localized_finish_reason(abnormal, locale);
            t!(
                "scheduler.interactive.ended_abnormal",
                title = pending.title.as_str(),
                reason = reason.as_str(),
                suffix = suffix.as_str(),
                locale = locale
            )
            .into_owned()
        }
    };
    if let Err(err) = app
        .qq_client
        .send_markdown_proactive(&pending.owner_openid, &text)
        .await
    {
        super::store::queue_pending_delivery(
            &app.config.general.data_dir,
            &pending.owner_openid,
            &super::store::PendingDelivery {
                job_id: pending.job_id.clone(),
                title: pending.title.clone(),
                text,
                failed_at: Utc::now(),
                error: err.to_string(),
            },
        )
        .await?;
    }
    Ok(())
}

fn interactive_spec(job: &CronJob) -> Option<&InteractiveSpec> {
    match &job.action {
        JobAction::CodexTurn { interactive, .. } => interactive.as_ref(),
        _ => None,
    }
}

pub async fn pending_for_owner(
    data_dir: &Path,
    openid: &str,
) -> Result<Option<PendingInteraction>> {
    let root = DataLayout::new(data_dir).cron_jobs_dir();
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", root.display())),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path().join("pending.json");
        let Some(pending) = read_json_opt_async::<PendingInteraction>(&path).await? else {
            continue;
        };
        if pending.owner_openid == openid {
            return Ok(Some(pending));
        }
    }
    Ok(None)
}

async fn read_pending(data_dir: &Path, job_id: &str) -> Result<Option<PendingInteraction>> {
    read_json_opt_async(&pending_path(data_dir, job_id)).await
}

async fn write_pending(data_dir: &Path, pending: &PendingInteraction) -> Result<()> {
    let path = pending_path(data_dir, &pending.job_id);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, serde_json::to_string_pretty(pending)?)
        .await
        .with_context(|| format!("failed to write {}", path.display()))
}

async fn remove_pending(data_dir: &Path, job_id: &str) -> Result<()> {
    let path = pending_path(data_dir, job_id);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn pending_path(data_dir: &Path, job_id: &str) -> PathBuf {
    new_job_dir(data_dir, job_id).join("pending.json")
}

fn localized_finish_reason(reason: &str, locale: &str) -> String {
    match reason {
        "failed" => t!("scheduler.interactive.reason_failed", locale = locale).into_owned(),
        "timed_out" => t!("scheduler.interactive.reason_timed_out", locale = locale).into_owned(),
        "no_answer" => t!("scheduler.interactive.reason_no_answer", locale = locale).into_owned(),
        other => other.to_string(),
    }
}

fn slug(raw: &str) -> String {
    let mut out = raw
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else if ch.is_whitespace() || matches!(ch, '-' | '_') {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "job".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abnormal_finish_reasons_are_localized_not_raw_tokens() {
        // Guards the regression where a reason passed to finish_job has no
        // catalog entry and falls through `other => other.to_string()`, leaking
        // a raw snake_case token into the localized banner.
        for locale in ["en", "zh"] {
            for reason in ["failed", "timed_out", "no_answer"] {
                let rendered = localized_finish_reason(reason, locale);
                assert_ne!(
                    rendered, reason,
                    "reason `{reason}` was not localized for locale `{locale}`"
                );
                assert!(
                    !rendered.contains('_'),
                    "raw token leaked for `{reason}`/`{locale}`: {rendered}"
                );
            }
        }
    }

    #[test]
    fn strip_end_signal_removes_standalone_and_inline_tokens() {
        let (text, ended) = strip_end_signal("答案正确\n<<<CLAW_END>>>", "<<<CLAW_END>>>");
        assert!(ended);
        assert_eq!(text, "答案正确");

        let (text, ended) = strip_end_signal("done <<<CLAW_END>>>", "<<<CLAW_END>>>");
        assert!(ended);
        assert_eq!(text, "done");
    }

    #[test]
    fn protocol_prompt_contains_end_signal_and_user_prompt() {
        let spec = InteractiveSpec::default();
        let prompt = build_protocol_prompt("owner", "quiz", "ask a question", &spec);
        assert!(prompt.contains("<<<CLAW_END>>>"));
        assert!(prompt.contains("ask a question"));
        assert!(prompt.contains("openid=owner"));
    }
}
