use std::{env, path::PathBuf};

use serde::Deserialize;

use crate::session::state::{ContextMode, ReasoningEffort, ServiceTier};

#[derive(Debug, Clone, Default)]
pub struct CodexRuntimeProfile {
    pub configured_model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub service_tier: Option<ServiceTier>,
    pub context_mode: Option<ContextMode>,
    pub model_provider: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    model: Option<String>,
    model_reasoning_effort: Option<ReasoningEffort>,
    service_tier: Option<ServiceTier>,
    model_context_window: Option<u64>,
    model_provider: Option<String>,
}

pub fn read_codex_runtime_profile() -> CodexRuntimeProfile {
    let codex_home = env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/root"))
                .join(".codex")
        });
    let config_path = codex_home.join("config.toml");
    let Ok(raw) = std::fs::read_to_string(config_path) else {
        return CodexRuntimeProfile::default();
    };
    let Ok(parsed) = toml::from_str::<RawConfig>(&raw) else {
        return CodexRuntimeProfile::default();
    };
    CodexRuntimeProfile {
        configured_model: parsed.model.filter(|value| !value.trim().is_empty()),
        reasoning_effort: parsed.model_reasoning_effort,
        service_tier: parsed.service_tier,
        context_mode: parsed.model_context_window.map(|value| {
            if value >= 1_000_000 {
                ContextMode::OneM
            } else {
                ContextMode::Standard
            }
        }),
        model_provider: parsed.model_provider.filter(|value| !value.trim().is_empty()),
    }
}
