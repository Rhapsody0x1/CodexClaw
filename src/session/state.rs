use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

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
    pub fn parse(input: &str) -> Option<Self> {
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

    pub fn parse_supported(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            _ => None,
        }
    }

    pub fn normalized(self) -> Self {
        match self {
            Self::None | Self::Minimal => Self::Low,
            other => other,
        }
    }

    pub fn as_str(self) -> &'static str {
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
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "fast" | "on" => Some(Self::Fast),
            "flex" | "off" => Some(Self::Flex),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Flex => "flex",
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
    pub const STANDARD_CONTEXT_WINDOW: u64 = 272_000;

    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "standard" | "272k" => Some(Self::Standard),
            "1m" => Some(Self::OneM),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::OneM => "1m",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "272K",
            Self::OneM => "1M",
        }
    }

    pub fn from_model_context_window(window: u64) -> Self {
        if window > Self::STANDARD_CONTEXT_WINDOW {
            Self::OneM
        } else {
            Self::Standard
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSettings {
    pub model_override: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub service_tier: Option<ServiceTier>,
    pub context_mode: Option<ContextMode>,
    #[serde(default)]
    pub verbose: bool,
    #[serde(default)]
    pub plan_mode: bool,
    #[serde(default = "default_language")]
    pub language: String,
}

fn default_language() -> String {
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
            language: default_language(),
        }
    }
}

impl SessionSettings {
    pub fn merged_with_profile(&self, profile: Option<&DialogProfile>) -> Self {
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
        if profile.context_mode.is_some() {
            merged.context_mode = profile.context_mode;
        }
        merged
    }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionState {
    pub session_id: Option<String>,
    #[serde(default)]
    pub settings: SessionSettings,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DialogOrigin {
    #[default]
    Local,
    Global,
}

impl DialogOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "claw",
            Self::Global => "native",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DialogProfile {
    pub model_override: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub service_tier: Option<ServiceTier>,
    pub context_mode: Option<ContextMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ImportedSessionProfile {
    pub workspace_dir: PathBuf,
    pub model_override: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub service_tier: Option<ServiceTier>,
    pub context_mode: Option<ContextMode>,
}

impl ImportedSessionProfile {
    pub fn dialog_profile(&self) -> DialogProfile {
        DialogProfile {
            model_override: self.model_override.clone(),
            reasoning_effort: self.reasoning_effort,
            service_tier: self.service_tier,
            context_mode: self.context_mode,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsageSnapshot {
    pub total_tokens: u64,
    pub window: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TokenUsageSnapshot {
    pub fn context_tokens(&self) -> Option<u64> {
        if self.window > 0 && self.total_tokens > self.window {
            return None;
        }
        Some(self.total_tokens)
    }

    pub fn percent_remaining(&self) -> Option<u64> {
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

    pub fn percent_used(&self) -> Option<u64> {
        self.percent_remaining()
            .map(|value| 100_u64.saturating_sub(value))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DialogState {
    pub session_id: Option<String>,
    #[serde(default)]
    pub origin: DialogOrigin,
    pub workspace_dir: PathBuf,
    #[serde(default)]
    pub saved: bool,
    #[serde(default)]
    pub profile: Option<DialogProfile>,
    #[serde(default)]
    pub last_usage: Option<TokenUsageSnapshot>,
}

impl DialogState {
    pub fn new_temporary(workspace_dir: PathBuf) -> Self {
        Self {
            session_id: None,
            origin: DialogOrigin::Local,
            workspace_dir,
            saved: false,
            profile: None,
            last_usage: None,
        }
    }

    pub fn is_temporary(&self) -> bool {
        self.session_id.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandAlias {
    pub name: String,
    pub commands: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingSetting {
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
}

impl PendingSetting {
    pub fn command_name(&self, locale: &str) -> &'static str {
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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserSessionState {
    pub foreground: DialogState,
    #[serde(default)]
    pub background: BTreeMap<String, DialogState>,
    #[serde(default)]
    pub background_order: Vec<String>,
    #[serde(default)]
    pub settings: SessionSettings,
    #[serde(default)]
    pub alias_seq: u64,
    #[serde(default)]
    pub last_projects_view: Vec<String>,
    #[serde(default)]
    pub last_sessions_view: Vec<String>,
    #[serde(default)]
    pub last_import_projects_view: Vec<String>,
    #[serde(default)]
    pub last_import_sessions_view: Vec<String>,
    #[serde(default)]
    pub saved_local_session_ids: Vec<String>,
    #[serde(default)]
    pub command_aliases: BTreeMap<String, CommandAlias>,
    #[serde(default)]
    pub pending_setting: Option<PendingSetting>,
}

impl UserSessionState {
    pub fn new(default_workspace_dir: PathBuf) -> Self {
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
pub struct PersistedSessionState {
    #[serde(default)]
    pub users: BTreeMap<String, UserSessionState>,
    #[serde(default)]
    pub imported_profiles: BTreeMap<String, ImportedSessionProfile>,
}

#[cfg(test)]
mod token_usage_tests {
    use super::TokenUsageSnapshot;

    #[test]
    fn percent_remaining_returns_none_for_implausible_legacy_cumulative_usage() {
        let snapshot = TokenUsageSnapshot {
            total_tokens: 19_668_612,
            window: 1_000_000,
            input_tokens: 19_568_077,
            cached_input_tokens: 18_968_448,
            output_tokens: 100_535,
            updated_at: chrono::Utc::now(),
        };

        assert_eq!(snapshot.context_tokens(), None);
        assert_eq!(snapshot.percent_remaining(), None);
        assert_eq!(snapshot.percent_used(), None);
    }
}
