//! Per-user session settings and the `state.json` document they persist into.
//!
//! These are pure value types: they own their own parsing/formatting but reach
//! for nothing outside `model/`, which is what lets `config` and `codex` depend
//! on them without pulling in `session`.

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::cron::CronJob;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    Xhigh,
}

impl ReasoningEffort {
    pub(crate) fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            _ => None,
        }
    }

    pub(crate) fn parse_supported(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            _ => None,
        }
    }

    pub(crate) fn normalized(self) -> Self {
        match self {
            Self::None | Self::Minimal => Self::Low,
            other => other,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self.normalized() {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::None | Self::Minimal => "low",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceTier {
    Fast,
    Flex,
}

impl ServiceTier {
    pub(crate) fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "fast" | "on" => Some(Self::Fast),
            "flex" | "off" => Some(Self::Flex),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Flex => "flex",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ApprovalPolicySetting {
    UnlessTrusted,
    OnRequest,
    Never,
    GuardianSubagent,
}

impl ApprovalPolicySetting {
    pub(crate) fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "never" | "off" | "关" | "关闭" => Some(Self::Never),
            "on-request" | "on" | "开" | "开启" | "ask" => Some(Self::OnRequest),
            "unless-trusted" | "strict" | "严格" => Some(Self::UnlessTrusted),
            "guardian-subagent" | "guardian" | "guardian_subagent" | "守护" => {
                Some(Self::GuardianSubagent)
            }
            _ => None,
        }
    }

    pub(crate) fn label_zh(self) -> &'static str {
        match self {
            Self::UnlessTrusted => "严格（unless-trusted）",
            Self::OnRequest => "按需（on-request）",
            Self::Never => "关闭（never）",
            Self::GuardianSubagent => "守护子代理（guardian-subagent）",
        }
    }

    pub(crate) fn label_en(self) -> &'static str {
        match self {
            Self::UnlessTrusted => "unless-trusted",
            Self::OnRequest => "on-request",
            Self::Never => "never",
            Self::GuardianSubagent => "guardian-subagent",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ContextMode {
    Standard,
    #[serde(rename = "1m")]
    OneM,
}

impl ContextMode {
    pub(crate) const STANDARD_CONTEXT_WINDOW: u64 = 272_000;

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Standard => "272K",
            Self::OneM => "1M",
        }
    }

    pub(crate) fn from_model_context_window(window: u64) -> Self {
        if window > Self::STANDARD_CONTEXT_WINDOW {
            Self::OneM
        } else {
            Self::Standard
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SessionSettings {
    pub(crate) model_override: Option<String>,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    pub(crate) service_tier: Option<ServiceTier>,
    pub(crate) context_mode: Option<ContextMode>,
    #[serde(default)]
    pub(crate) verbose: bool,
    #[serde(default)]
    pub(crate) plan_mode: bool,
    #[serde(default)]
    pub(crate) approval_policy_override: Option<ApprovalPolicySetting>,
    #[serde(default)]
    pub(crate) pending_plan: Option<String>,
    #[serde(default = "default_language")]
    pub(crate) language: String,
}

pub(crate) fn default_language() -> String {
    "en".to_string()
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            model_override: None,
            reasoning_effort: None,
            service_tier: None,
            context_mode: None,
            verbose: false,
            plan_mode: false,
            approval_policy_override: None,
            pending_plan: None,
            language: default_language(),
        }
    }
}

impl SessionSettings {
    pub(crate) fn merged_with_profile(&self, profile: Option<&DialogProfile>) -> Self {
        let mut merged = self.clone();
        let Some(profile) = profile else {
            return merged;
        };
        if profile.model_override.is_some() {
            merged.model_override = profile.model_override.clone();
        }
        if profile.reasoning_effort.is_some() {
            merged.reasoning_effort = profile.reasoning_effort;
        }
        if profile.service_tier.is_some() {
            merged.service_tier = profile.service_tier;
        }
        if profile.context_mode.is_some() {
            merged.context_mode = profile.context_mode;
        }
        merged
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionState {
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) settings: SessionSettings,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DialogOrigin {
    #[default]
    Local,
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct DialogProfile {
    pub(crate) model_override: Option<String>,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    pub(crate) service_tier: Option<ServiceTier>,
    pub(crate) context_mode: Option<ContextMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct ImportedSessionProfile {
    pub(crate) workspace_dir: PathBuf,
    pub(crate) model_override: Option<String>,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    pub(crate) service_tier: Option<ServiceTier>,
    pub(crate) context_mode: Option<ContextMode>,
}

impl ImportedSessionProfile {
    pub(crate) fn dialog_profile(&self) -> DialogProfile {
        DialogProfile {
            model_override: self.model_override.clone(),
            reasoning_effort: self.reasoning_effort,
            service_tier: self.service_tier,
            context_mode: self.context_mode,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TokenUsageSnapshot {
    pub(crate) total_tokens: u64,
    pub(crate) window: u64,
    #[serde(default)]
    pub(crate) input_tokens: u64,
    #[serde(default)]
    pub(crate) cached_input_tokens: u64,
    #[serde(default)]
    pub(crate) output_tokens: u64,
    pub(crate) updated_at: chrono::DateTime<chrono::Utc>,
}

impl TokenUsageSnapshot {
    pub(crate) fn context_tokens(&self) -> Option<u64> {
        if self.window > 0 && self.total_tokens > self.window {
            return None;
        }
        Some(self.total_tokens)
    }

    pub(crate) fn percent_remaining(&self) -> Option<u64> {
        if self.window == 0 {
            return None;
        }

        const BASELINE_TOKENS: u64 = 12_000;
        if self.window <= BASELINE_TOKENS {
            return Some(0);
        }

        let effective_window = self.window - BASELINE_TOKENS;
        let used = self.context_tokens()?.saturating_sub(BASELINE_TOKENS);
        let remaining = effective_window.saturating_sub(used);
        Some(
            ((remaining as f64 / effective_window as f64) * 100.0)
                .clamp(0.0, 100.0)
                .round() as u64,
        )
    }

    pub(crate) fn percent_used(&self) -> Option<u64> {
        self.percent_remaining()
            .map(|value| 100_u64.saturating_sub(value))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DialogState {
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) origin: DialogOrigin,
    pub(crate) workspace_dir: PathBuf,
    #[serde(default)]
    pub(crate) saved: bool,
    #[serde(default)]
    pub(crate) profile: Option<DialogProfile>,
    #[serde(default)]
    pub(crate) last_usage: Option<TokenUsageSnapshot>,
}

impl DialogState {
    pub(crate) fn new_temporary(workspace_dir: PathBuf) -> Self {
        Self {
            session_id: None,
            origin: DialogOrigin::Local,
            workspace_dir,
            saved: false,
            profile: None,
            last_usage: None,
        }
    }

    pub(crate) fn is_temporary(&self) -> bool {
        self.session_id.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CommandAlias {
    pub(crate) name: String,
    pub(crate) commands: Vec<String>,
    pub(crate) created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PendingSetting {
    Model,
    Reasoning,
    Fast,
    Context,
    Verbose,
    Lang,
    SessionsProjects,
    SessionsSessions {
        project_key: String,
        page: usize,
    },
    ImportProjects,
    ImportSessions {
        project_key: String,
        page: usize,
    },
    Fg,
    ResumeProjects,
    ResumeSessions {
        project_key: String,
        page: usize,
    },
    LoadbgProjects,
    LoadbgSessions {
        project_key: String,
        page: usize,
        #[serde(default)]
        alias: Option<String>,
    },
    Approvals,
    Plan,
    ResumeRecovery,
}

impl PendingSetting {
    pub(crate) fn command_name(&self, locale: &str) -> &'static str {
        use PendingSetting::*;
        let zh = locale.eq_ignore_ascii_case("zh");
        match self {
            Model => {
                if zh {
                    "/模型"
                } else {
                    "/model"
                }
            }
            Reasoning => {
                if zh {
                    "/思考"
                } else {
                    "/reasoning"
                }
            }
            Fast => {
                if zh {
                    "/快速"
                } else {
                    "/fast"
                }
            }
            Context => {
                if zh {
                    "/上下文"
                } else {
                    "/context"
                }
            }
            Verbose => {
                if zh {
                    "/详细"
                } else {
                    "/verbose"
                }
            }
            Lang => {
                if zh {
                    "/语言"
                } else {
                    "/lang"
                }
            }
            SessionsProjects | SessionsSessions { .. } => {
                if zh {
                    "/会话"
                } else {
                    "/sessions"
                }
            }
            ImportProjects | ImportSessions { .. } => {
                if zh {
                    "/导入"
                } else {
                    "/import"
                }
            }
            Fg => {
                if zh {
                    "/前台"
                } else {
                    "/fg"
                }
            }
            ResumeProjects | ResumeSessions { .. } => {
                if zh {
                    "/恢复"
                } else {
                    "/resume"
                }
            }
            LoadbgProjects | LoadbgSessions { .. } => {
                if zh {
                    "/载入后台"
                } else {
                    "/loadbg"
                }
            }
            Approvals => {
                if zh {
                    "/审批"
                } else {
                    "/approvals"
                }
            }
            Plan => {
                if zh {
                    "/计划"
                } else {
                    "/plan"
                }
            }
            ResumeRecovery => {
                if zh {
                    "/恢复"
                } else {
                    "/resume"
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct UserSessionState {
    pub(crate) foreground: DialogState,
    #[serde(default)]
    pub(crate) background: BTreeMap<String, DialogState>,
    #[serde(default)]
    pub(crate) background_order: Vec<String>,
    #[serde(default)]
    pub(crate) settings: SessionSettings,
    #[serde(default)]
    pub(crate) alias_seq: u64,
    #[serde(default)]
    pub(crate) last_projects_view: Vec<String>,
    #[serde(default)]
    pub(crate) last_sessions_view: Vec<String>,
    #[serde(default)]
    pub(crate) last_import_projects_view: Vec<String>,
    #[serde(default)]
    pub(crate) last_import_sessions_view: Vec<String>,
    #[serde(default)]
    pub(crate) saved_local_session_ids: Vec<String>,
    #[serde(default)]
    pub(crate) command_aliases: BTreeMap<String, CommandAlias>,
    #[serde(default)]
    pub(crate) pending_setting: Option<PendingSetting>,
}

impl UserSessionState {
    /// Canonical "fresh user" constructor. Currently only reached through
    /// `Default`-ish literals in `session/store.rs`; kept as the single source
    /// of truth for the initial field values so those literals can be folded
    /// into it.
    #[allow(dead_code)]
    pub(crate) fn new(default_workspace_dir: PathBuf) -> Self {
        Self {
            foreground: DialogState::new_temporary(default_workspace_dir),
            background: BTreeMap::new(),
            background_order: Vec::new(),
            settings: SessionSettings::default(),
            alias_seq: 0,
            last_projects_view: Vec::new(),
            last_sessions_view: Vec::new(),
            last_import_projects_view: Vec::new(),
            last_import_sessions_view: Vec::new(),
            saved_local_session_ids: Vec::new(),
            command_aliases: BTreeMap::new(),
            pending_setting: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct PersistedSessionState {
    #[serde(default)]
    pub(crate) users: BTreeMap<String, UserSessionState>,
    #[serde(default)]
    pub(crate) imported_profiles: BTreeMap<String, ImportedSessionProfile>,
    #[serde(default)]
    pub(crate) cron_jobs: BTreeMap<String, CronJob>,
}

#[cfg(test)]
mod tests {
    use super::ContextMode;

    #[test]
    fn context_window_above_standard_is_one_m() {
        assert_eq!(
            ContextMode::from_model_context_window(ContextMode::STANDARD_CONTEXT_WINDOW + 1),
            ContextMode::OneM
        );
        assert_eq!(
            ContextMode::from_model_context_window(950_000),
            ContextMode::OneM
        );
    }

    #[test]
    fn standard_context_window_stays_standard() {
        assert_eq!(
            ContextMode::from_model_context_window(ContextMode::STANDARD_CONTEXT_WINDOW),
            ContextMode::Standard
        );
        assert_eq!(
            ContextMode::from_model_context_window(128_000),
            ContextMode::Standard
        );
    }
}
