//! The `jobs.json` / `job.toml` document model for scheduled jobs.
//!
//! Pure value types with no scheduler behaviour attached, so `session` can hold
//! a `CronJob` map without depending on `scheduler`.

use std::{collections::BTreeMap, path::PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::settings::{ApprovalPolicySetting, SessionState};

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
        session_state: Option<SessionState>,
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
    /// Part of the on-disk `jobs.json` format. The CLI has no flag that
    /// produces it — only a hand-edited `jobs.json` can select it — but the
    /// runner honours it, so the variant must stay for wire compatibility.
    PushIfNonEmpty,
    LogOnly,
    /// Part of the on-disk `jobs.json` format. The CLI has no flag that
    /// produces it — only a hand-edited `jobs.json` can select it — but the
    /// runner honours it, so the variant must stay for wire compatibility.
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
    /// Part of the on-disk run-history format. Nothing in the current runner
    /// emits it, but persisted histories may still contain it and the
    /// failure-streak accounting reads it, so the variant must stay.
    Skipped {
        reason: String,
    },
}

#[cfg(test)]
pub(crate) mod fixtures {
    use std::{collections::BTreeMap, path::PathBuf};

    use chrono::{DateTime, Utc};

    use super::{CronJob, CronKind, DeliverPolicy, JobAction};

    pub(crate) fn ts(rfc3339: &str) -> DateTime<Utc> {
        crate::util::time::parse_utc_strict(rfc3339).unwrap()
    }

    /// Baseline one-shot shell job. Callers override just the fields their
    /// assertions depend on.
    pub(crate) fn shell_job(id: &str, workspace_dir: PathBuf, at: DateTime<Utc>) -> CronJob {
        CronJob {
            id: id.to_string(),
            owner_openid: "owner".to_string(),
            title: "sample".to_string(),
            kind: CronKind::OneShot { at },
            action: JobAction::Shell {
                program: "/bin/echo".to_string(),
                args: vec!["ok".to_string()],
                env: BTreeMap::new(),
            },
            workspace_dir,
            deliver: DeliverPolicy::LogOnly,
            created_at: at,
            next_run_at: Some(at),
            run_now_at: None,
            last_run_at: None,
            last_run_status: None,
            run_count: 0,
            failure_streak: 0,
            disabled: false,
        }
    }
}
