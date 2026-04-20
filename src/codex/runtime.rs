use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
};

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
    #[serde(default)]
    profiles: Option<BTreeMap<String, RawProfile>>,
}

#[derive(Debug, Deserialize)]
struct RawProfile {
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawModelList {
    #[serde(default)]
    canonical: Vec<String>,
}

const CODEX_MODELS_TOML: &str = include_str!("../../config/codex_models.toml");

pub fn read_codex_runtime_profile() -> CodexRuntimeProfile {
    read_codex_runtime_profile_from_path(&codex_config_path())
}

pub fn read_codex_runtime_profile_from_path(config_path: &Path) -> CodexRuntimeProfile {
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
        context_mode: parsed
            .model_context_window
            .map(ContextMode::from_model_context_window),
        model_provider: parsed
            .model_provider
            .filter(|value| !value.trim().is_empty()),
    }
}

/// Return the deduplicated list of model names available to the user.
///
/// Merges, in order:
///   1. Canonical upstream list from `config/codex_models.toml`.
///   2. `runtime_profile.configured_model` (the top-level `model = …` in
///      `~/.codex/config.toml`).
///   3. `model` values defined under `[profiles.*]` in that same file.
///   4. An optional `extra` slice for per-session overrides the caller wants
///      surfaced (e.g. the user's `model_override`).
pub fn list_codex_models(runtime_profile: &CodexRuntimeProfile, extra: &[String]) -> Vec<String> {
    list_codex_models_with_path(runtime_profile, extra, &codex_config_path())
}

pub fn list_codex_models_with_path(
    runtime_profile: &CodexRuntimeProfile,
    extra: &[String],
    config_path: &Path,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |name: &str| {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return;
        }
        if !out.iter().any(|existing| existing == trimmed) {
            out.push(trimmed.to_string());
        }
    };

    if let Ok(list) = toml::from_str::<RawModelList>(CODEX_MODELS_TOML) {
        for model in list.canonical {
            push(&model);
        }
    }

    if let Some(model) = runtime_profile.configured_model.as_deref() {
        push(model);
    }

    if let Ok(raw) = std::fs::read_to_string(config_path)
        && let Ok(parsed) = toml::from_str::<RawConfig>(&raw)
        && let Some(profiles) = parsed.profiles
    {
        for profile in profiles.values() {
            if let Some(model) = profile.model.as_deref() {
                push(model);
            }
        }
    }

    for value in extra {
        push(value);
    }

    out
}

fn codex_config_path() -> PathBuf {
    let codex_home = env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/root"))
                .join(".codex")
        });
    codex_home.join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn canonical_list_is_non_empty() {
        let list = list_codex_models_with_path(
            &CodexRuntimeProfile::default(),
            &[],
            Path::new("/dev/null"),
        );
        assert!(!list.is_empty(), "canonical list must not be empty");
        assert!(list.iter().any(|m| m.starts_with("gpt-")));
    }

    #[test]
    fn merges_profiles_and_extras_without_duplicates() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            r#"
model = "gpt-configured"

[profiles.team]
model = "gpt-team"

[profiles.solo]
model = "gpt-5.4"   # already canonical, must dedupe
"#,
        )
        .unwrap();
        let profile = CodexRuntimeProfile {
            configured_model: Some("gpt-configured".into()),
            ..CodexRuntimeProfile::default()
        };
        let list = list_codex_models_with_path(
            &profile,
            &["gpt-user-override".into(), "gpt-configured".into()],
            tmp.path(),
        );
        assert!(list.iter().filter(|m| *m == "gpt-5.4").count() == 1);
        assert!(list.contains(&"gpt-configured".to_string()));
        assert!(list.contains(&"gpt-team".to_string()));
        assert!(list.contains(&"gpt-user-override".to_string()));
    }
}
