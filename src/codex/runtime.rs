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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexModelEntry {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: Option<String>,
    pub description_zh: Option<String>,
    pub description_en: Option<String>,
}

impl CodexModelEntry {
    pub fn description_for_locale(&self, locale: &str) -> Option<&str> {
        if locale.eq_ignore_ascii_case("zh") {
            self.description_zh
                .as_deref()
                .or(self.description.as_deref())
                .or(self.description_en.as_deref())
        } else {
            self.description_en
                .as_deref()
                .or(self.description.as_deref())
                .or(self.description_zh.as_deref())
        }
    }
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
    #[serde(default)]
    models: Vec<RawModelEntry>,
}

#[derive(Debug, Deserialize)]
struct RawModelEntry {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    description: Option<String>,
    description_zh: Option<String>,
    description_en: Option<String>,
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
    list_codex_model_entries(runtime_profile, extra)
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

pub fn list_codex_model_entries(
    runtime_profile: &CodexRuntimeProfile,
    extra: &[String],
) -> Vec<CodexModelEntry> {
    list_codex_model_entries_with_path(runtime_profile, extra, &codex_config_path())
}

pub fn list_codex_model_entries_with_path(
    runtime_profile: &CodexRuntimeProfile,
    extra: &[String],
    config_path: &Path,
) -> Vec<CodexModelEntry> {
    let mut out: Vec<CodexModelEntry> = Vec::new();
    let mut push = |entry: RawCodexModelEntry<'_>| {
        let trimmed = entry.name.trim();
        if trimmed.is_empty() {
            return;
        }
        let normalized_aliases = entry
            .aliases
            .iter()
            .filter_map(|alias| {
                let trimmed_alias = alias.trim();
                if trimmed_alias.is_empty() || trimmed_alias.eq_ignore_ascii_case(trimmed) {
                    None
                } else {
                    Some(trimmed_alias.to_string())
                }
            })
            .fold(Vec::<String>::new(), |mut acc, alias| {
                if !acc
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(&alias))
                {
                    acc.push(alias);
                }
                acc
            });
        if let Some(existing) = out.iter_mut().find(|existing| existing.name == trimmed) {
            for alias in normalized_aliases {
                if !existing
                    .aliases
                    .iter()
                    .any(|current| current.eq_ignore_ascii_case(&alias))
                {
                    existing.aliases.push(alias);
                }
            }
            if existing.description.is_none() {
                existing.description = entry.description.map(str::to_string);
            }
            if existing.description_zh.is_none() {
                existing.description_zh = entry.description_zh.map(str::to_string);
            }
            if existing.description_en.is_none() {
                existing.description_en = entry.description_en.map(str::to_string);
            }
        } else {
            out.push(CodexModelEntry {
                name: trimmed.to_string(),
                aliases: normalized_aliases,
                description: entry.description.map(str::to_string),
                description_zh: entry.description_zh.map(str::to_string),
                description_en: entry.description_en.map(str::to_string),
            });
        }
    };

    if let Ok(list) = toml::from_str::<RawModelList>(CODEX_MODELS_TOML) {
        for model in list.canonical {
            push(RawCodexModelEntry {
                name: &model,
                aliases: &[],
                description: None,
                description_zh: None,
                description_en: None,
            });
        }
        for model in list.models {
            push(RawCodexModelEntry {
                name: &model.name,
                aliases: &model.aliases,
                description: model.description.as_deref(),
                description_zh: model.description_zh.as_deref(),
                description_en: model.description_en.as_deref(),
            });
        }
    }

    if let Some(model) = runtime_profile.configured_model.as_deref() {
        push(RawCodexModelEntry {
            name: model,
            aliases: &[],
            description: None,
            description_zh: None,
            description_en: None,
        });
    }

    if let Ok(raw) = std::fs::read_to_string(config_path)
        && let Ok(parsed) = toml::from_str::<RawConfig>(&raw)
        && let Some(profiles) = parsed.profiles
    {
        for profile in profiles.values() {
            if let Some(model) = profile.model.as_deref() {
                push(RawCodexModelEntry {
                    name: model,
                    aliases: &[],
                    description: None,
                    description_zh: None,
                    description_en: None,
                });
            }
        }
    }

    for value in extra {
        push(RawCodexModelEntry {
            name: value,
            aliases: &[],
            description: None,
            description_zh: None,
            description_en: None,
        });
    }

    out
}

pub fn list_codex_models_with_path(
    runtime_profile: &CodexRuntimeProfile,
    extra: &[String],
    config_path: &Path,
) -> Vec<String> {
    list_codex_model_entries_with_path(runtime_profile, extra, config_path)
        .into_iter()
        .map(|entry| entry.name)
        .collect()
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

struct RawCodexModelEntry<'a> {
    name: &'a str,
    aliases: &'a [String],
    description: Option<&'a str>,
    description_zh: Option<&'a str>,
    description_en: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn canonical_list_is_non_empty() {
        let list = list_codex_model_entries_with_path(
            &CodexRuntimeProfile::default(),
            &[],
            Path::new("/dev/null"),
        );
        assert!(!list.is_empty(), "canonical list must not be empty");
        assert!(list.iter().any(|m| m.name.starts_with("gpt-")));
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

    #[test]
    fn canonical_models_expose_aliases() {
        let list = list_codex_model_entries_with_path(
            &CodexRuntimeProfile::default(),
            &[],
            Path::new("/dev/null"),
        );
        let mini = list
            .iter()
            .find(|entry| entry.name == "gpt-5.4-mini")
            .unwrap();
        assert!(mini.aliases.iter().any(|alias| alias == "mini"));
        assert_eq!(
            mini.description_for_locale("zh"),
            Some("小巧快速的通用模型")
        );
    }
}
