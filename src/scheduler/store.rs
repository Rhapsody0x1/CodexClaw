use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::session::state::ApprovalPolicySetting;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CronJob {
    pub id: String,
    pub owner_openid: String,
    pub title: String,
    pub kind: CronKind,
    pub action: JobAction,
    pub workspace_dir: PathBuf,
    #[serde(default)]
    pub deliver: DeliverPolicy,
    pub created_at: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub run_now_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_run_status: Option<RunStatus>,
    #[serde(default)]
    pub run_count: u64,
    #[serde(default)]
    pub failure_streak: u32,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum CronKind {
    Recurring { cron: String, tz: String },
    OneShot { at: DateTime<Utc> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum JobAction {
    Reminder {
        message: String,
    },
    CodexTurn {
        prompt: String,
        model: Option<String>,
        session_state: Option<crate::session::state::SessionState>,
        #[serde(default)]
        approval_policy: Option<ApprovalPolicySetting>,
        session_strategy: SessionStrategy,
        #[serde(default)]
        interactive: Option<InteractiveSpec>,
    },
    CodexExec {
        prompt: String,
        model: Option<String>,
        #[serde(default)]
        extra_args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Shell {
        program: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InteractiveSpec {
    #[serde(default = "default_reply_ttl_secs")]
    pub reply_ttl_secs: u64,
    #[serde(default = "default_end_signal")]
    pub end_signal: String,
    #[serde(default = "default_max_rounds_hard_cap")]
    pub max_rounds_hard_cap: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingDelivery {
    pub job_id: String,
    pub title: String,
    pub text: String,
    pub failed_at: DateTime<Utc>,
    pub error: String,
}

impl Default for InteractiveSpec {
    fn default() -> Self {
        Self {
            reply_ttl_secs: default_reply_ttl_secs(),
            end_signal: default_end_signal(),
            max_rounds_hard_cap: default_max_rounds_hard_cap(),
        }
    }
}

fn default_reply_ttl_secs() -> u64 {
    86_400
}

fn default_end_signal() -> String {
    "<<<CLAW_END>>>".to_string()
}

fn default_max_rounds_hard_cap() -> u32 {
    10
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SessionStrategy {
    #[default]
    PerInvocation,
    Persistent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DeliverPolicy {
    #[default]
    PushToOwner,
    PushIfNonEmpty,
    LogOnly,
    PushTruncated {
        max_chars: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RunStatus {
    Success {
        duration_ms: u64,
        output_chars: usize,
    },
    Failure {
        error: String,
        attempt: u32,
    },
    Skipped {
        reason: String,
    },
}

pub fn new_job_dir(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join("cron-jobs").join(id)
}

pub async fn prepare_job_dirs(data_dir: &Path, id: &str) -> Result<PathBuf> {
    let job_dir = new_job_dir(data_dir, id);
    tokio::fs::create_dir_all(job_dir.join("workspace")).await?;
    tokio::fs::create_dir_all(job_skill_dir(data_dir, id)).await?;
    tokio::fs::create_dir_all(job_dir.join("runs")).await?;
    Ok(job_dir)
}

pub async fn write_job_metadata(
    job: &CronJob,
    data_dir: &Path,
    codex_home_global: &Path,
) -> Result<()> {
    let job_dir = new_job_dir(data_dir, &job.id);
    tokio::fs::create_dir_all(job_dir.join("workspace")).await?;
    tokio::fs::create_dir_all(job_skill_dir(data_dir, &job.id)).await?;
    tokio::fs::create_dir_all(job_dir.join("runs")).await?;
    let job_toml = toml::to_string_pretty(job)?;
    tokio::fs::write(job_dir.join("job.toml"), job_toml).await?;
    let claw_job = serde_json::json!({
        "job_id": job.id,
        "title": job.title,
        "owner_openid": job.owner_openid,
        "job_dir": job_dir,
        "workspace_dir": job.workspace_dir,
        "skills_dir": job_skill_dir(data_dir, &job.id),
    });
    tokio::fs::create_dir_all(&job.workspace_dir).await?;
    tokio::fs::write(
        job.workspace_dir.join(".claw-job.json"),
        serde_json::to_string_pretty(&claw_job)?,
    )
    .await?;
    ensure_job_skill_link(data_dir, codex_home_global, &job.id).await?;
    Ok(())
}

pub async fn remove_job_files(
    data_dir: &Path,
    codex_home_global: &Path,
    id: &str,
    keep_files: bool,
) -> Result<()> {
    remove_job_skill_link(codex_home_global, id).await?;
    if !keep_files {
        let job_dir = new_job_dir(data_dir, id);
        match tokio::fs::remove_dir_all(&job_dir).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("failed to remove {}", job_dir.display()));
            }
        }
    }
    Ok(())
}

pub async fn recycle_job_files(data_dir: &Path, codex_home_global: &Path, id: &str) -> Result<()> {
    remove_job_skill_link(codex_home_global, id).await?;
    let job_dir = new_job_dir(data_dir, id);
    match tokio::fs::symlink_metadata(&job_dir).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to stat {}", job_dir.display()));
        }
    }
    let trash_root = data_dir.join("cron-jobs-trash");
    tokio::fs::create_dir_all(&trash_root).await?;
    let target = trash_root.join(format!("{}-{}", Utc::now().format("%Y%m%dT%H%M%SZ"), id));
    tokio::fs::rename(&job_dir, &target)
        .await
        .with_context(|| {
            format!(
                "failed to move {} to {}",
                job_dir.display(),
                target.display()
            )
        })?;
    Ok(())
}

pub async fn ensure_job_skill_link(
    data_dir: &Path,
    codex_home_global: &Path,
    id: &str,
) -> Result<()> {
    let skills_dir = job_skill_dir(data_dir, id);
    let link = codex_home_global
        .join("skills")
        .join(format!("claw-cron-{id}"));
    tokio::fs::create_dir_all(link.parent().unwrap_or(codex_home_global)).await?;
    if link.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&skills_dir, &link).with_context(|| {
            format!(
                "failed to symlink {} -> {}",
                link.display(),
                skills_dir.display()
            )
        })?;
    }
    #[cfg(not(unix))]
    {
        tokio::fs::create_dir_all(&link).await?;
    }
    Ok(())
}

pub async fn remove_job_skill_link(codex_home_global: &Path, id: &str) -> Result<()> {
    let link = codex_home_global
        .join("skills")
        .join(format!("claw-cron-{id}"));
    match tokio::fs::symlink_metadata(&link).await {
        Ok(meta) if meta.file_type().is_symlink() || meta.is_file() => {
            tokio::fs::remove_file(&link).await?;
        }
        Ok(meta) if meta.is_dir() => {
            tokio::fs::remove_dir_all(&link).await?;
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("failed to stat {}", link.display())),
    }
    Ok(())
}

pub fn job_skill_dir(data_dir: &Path, id: &str) -> PathBuf {
    new_job_dir(data_dir, id)
        .join("workspace")
        .join(".agents")
        .join("skills")
}

pub async fn queue_pending_delivery(
    data_dir: &Path,
    openid: &str,
    delivery: &PendingDelivery,
) -> Result<()> {
    let path = pending_delivery_path(data_dir, openid);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut raw = serde_json::to_string(delivery)?;
    raw.push('\n');
    let path_for_blocking = path.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path_for_blocking)
            .with_context(|| format!("failed to open {}", path_for_blocking.display()))?;
        file.write_all(raw.as_bytes())
            .with_context(|| format!("failed to write {}", path_for_blocking.display()))?;
        Ok(())
    })
    .await?
}

pub async fn take_pending_deliveries(
    data_dir: &Path,
    openid: &str,
) -> Result<Vec<PendingDelivery>> {
    let path = pending_delivery_path(data_dir, openid);
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).with_context(|| format!("failed to remove {}", path.display()));
        }
    }
    let mut deliveries = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let delivery = serde_json::from_str::<PendingDelivery>(line)
            .with_context(|| format!("failed to parse pending delivery line {}", index + 1))?;
        deliveries.push(delivery);
    }
    Ok(deliveries)
}

fn pending_delivery_path(data_dir: &Path, openid: &str) -> PathBuf {
    data_dir
        .join("scheduler")
        .join("pending-deliveries")
        .join(format!("{}.jsonl", sanitize_path_segment(openid)))
}

fn sanitize_path_segment(raw: &str) -> String {
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub async fn write_run_log(
    job: &CronJob,
    run_at: DateTime<Utc>,
    body: &str,
    runs_retention: usize,
) -> Result<()> {
    let runs_dir = job
        .workspace_dir
        .parent()
        .unwrap_or(job.workspace_dir.as_path())
        .join("runs");
    tokio::fs::create_dir_all(&runs_dir).await?;
    let name = run_at.format("%Y%m%dT%H%M%SZ.log").to_string();
    tokio::fs::write(runs_dir.join(name), body).await?;
    prune_run_logs(&runs_dir, runs_retention).await?;
    Ok(())
}

async fn prune_run_logs(runs_dir: &Path, runs_retention: usize) -> Result<()> {
    if runs_retention == 0 {
        return Ok(());
    }
    let mut entries = tokio::fs::read_dir(runs_dir).await?;
    let mut logs = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with(".log"))
        {
            logs.push(entry.path());
        }
    }
    logs.sort();
    let remove_count = logs.len().saturating_sub(runs_retention);
    for path in logs.into_iter().take(remove_count) {
        tokio::fs::remove_file(path).await.ok();
    }
    Ok(())
}

pub fn new_id() -> String {
    Ulid::new().to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{DateTime, Duration, Utc};
    use tempfile::tempdir;

    use super::*;

    fn sample_job(workspace_dir: PathBuf) -> CronJob {
        let created_at = DateTime::parse_from_rfc3339("2026-05-10T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        CronJob {
            id: "job-1".to_string(),
            owner_openid: "owner".to_string(),
            title: "sample".to_string(),
            kind: CronKind::OneShot { at: created_at },
            action: JobAction::Shell {
                program: "/bin/echo".to_string(),
                args: vec!["ok".to_string()],
                env: BTreeMap::new(),
            },
            workspace_dir,
            deliver: DeliverPolicy::LogOnly,
            created_at,
            next_run_at: Some(created_at),
            run_now_at: None,
            last_run_at: None,
            last_run_status: None,
            run_count: 0,
            failure_streak: 0,
            disabled: false,
        }
    }

    #[tokio::test]
    async fn write_run_log_prunes_old_logs() {
        let temp = tempdir().unwrap();
        let workspace_dir = temp.path().join("workspace");
        tokio::fs::create_dir_all(&workspace_dir).await.unwrap();
        let job = sample_job(workspace_dir);
        let base = DateTime::parse_from_rfc3339("2026-05-10T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        for offset in 0..4 {
            write_run_log(
                &job,
                base + Duration::seconds(offset),
                &format!("run {offset}"),
                2,
            )
            .await
            .unwrap();
        }

        let runs_dir = temp.path().join("runs");
        let mut entries = tokio::fs::read_dir(runs_dir).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().to_string());
        }
        names.sort();
        assert_eq!(names, vec!["20260510T100002Z.log", "20260510T100003Z.log"]);
    }

    #[tokio::test]
    async fn write_job_metadata_creates_operational_files() {
        let data = tempdir().unwrap();
        let codex_home = tempdir().unwrap();
        let job_dir = prepare_job_dirs(data.path(), "job-1").await.unwrap();
        let job = sample_job(job_dir.join("workspace"));

        write_job_metadata(&job, data.path(), codex_home.path())
            .await
            .unwrap();

        assert!(data.path().join("cron-jobs/job-1/job.toml").exists());
        assert!(
            data.path()
                .join("cron-jobs/job-1/workspace/.claw-job.json")
                .exists()
        );
        assert!(
            data.path()
                .join("cron-jobs/job-1/workspace/.agents/skills")
                .exists()
        );
        assert!(codex_home.path().join("skills/claw-cron-job-1").exists());
    }
}
