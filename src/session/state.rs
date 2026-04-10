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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionState {
    pub session_id: Option<String>,
    #[serde(default)]
    pub settings: SessionSettings,
}
