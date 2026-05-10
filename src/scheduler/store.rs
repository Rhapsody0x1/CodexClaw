use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

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
        session_strategy: SessionStrategy,
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
    tokio::fs::create_dir_all(job_dir.join("skills")).await?;
    tokio::fs::create_dir_all(job_dir.join("runs")).await?;
    Ok(job_dir)
}

pub async fn write_run_log(job: &CronJob, run_at: DateTime<Utc>, body: &str) -> Result<()> {
    let runs_dir = job
        .workspace_dir
        .parent()
        .unwrap_or(job.workspace_dir.as_path())
        .join("runs");
    tokio::fs::create_dir_all(&runs_dir).await?;
    let name = run_at.format("%Y%m%dT%H%M%SZ.log").to_string();
    tokio::fs::write(runs_dir.join(name), body).await?;
    Ok(())
}

pub fn new_id() -> String {
    Ulid::new().to_string()
}
