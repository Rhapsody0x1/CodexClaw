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

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
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
pub struct DialogState {
    pub session_id: Option<String>,
    #[serde(default)]
    pub origin: DialogOrigin,
    pub workspace_dir: PathBuf,
    #[serde(default)]
    pub saved: bool,
    #[serde(default)]
    pub profile: Option<DialogProfile>,
}

impl DialogState {
    pub fn new_temporary(workspace_dir: PathBuf) -> Self {
        Self {
            session_id: None,
            origin: DialogOrigin::Local,
            workspace_dir,
            saved: false,
            profile: None,
        }
    }

    pub fn is_temporary(&self) -> bool {
        self.session_id.is_none()
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
