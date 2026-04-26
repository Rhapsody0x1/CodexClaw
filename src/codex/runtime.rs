use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
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

pub fn write_service_tier_to_config_path(
    config_path: &Path,
    service_tier: Option<ServiceTier>,
) -> Result<()> {
    write_top_level_config_value(
        config_path,
        "service_tier",
        service_tier.map(|value| format!("\"{}\"", value.as_str())),
    )
}

pub fn write_model_to_config_path(config_path: &Path, model: Option<&str>) -> Result<()> {
    write_top_level_config_value(
        config_path,
        "model",
        model.map(|value| format!("\"{value}\"")),
    )
}

pub fn write_reasoning_effort_to_config_path(
    config_path: &Path,
    reasoning_effort: Option<ReasoningEffort>,
) -> Result<()> {
    write_top_level_config_value(
        config_path,
        "model_reasoning_effort",
        reasoning_effort.map(|value| format!("\"{}\"", value.as_str())),
    )
}

pub fn write_context_mode_to_config_path(
    config_path: &Path,
    context_mode: Option<ContextMode>,
) -> Result<()> {
    write_top_level_config_value(
        config_path,
        "model_context_window",
        context_mode.map(|value| match value {
            ContextMode::Standard => ContextMode::STANDARD_CONTEXT_WINDOW.to_string(),
            ContextMode::OneM => "1000000".to_string(),
        }),
    )
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

fn write_top_level_config_value(
    config_path: &Path,
    key: &str,
    value: Option<String>,
) -> Result<()> {
    let raw = match fs::read_to_string(config_path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", config_path.display()));
        }
    };
    let updated = rewrite_top_level_key(&raw, key, value.as_deref());
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    atomic_write(config_path, &updated)
}

fn rewrite_top_level_key(raw: &str, key: &str, value: Option<&str>) -> String {
    let replacement = value.map(|value| format!("{key} = {value}\n"));
    let mut lines = split_lines_preserve_newlines(raw);
    let table_start = lines
        .iter()
        .position(|line| line.trim_start().starts_with('['))
        .unwrap_or(lines.len());
    let first_match = lines
        .iter()
        .enumerate()
        .take(table_start)
        .find_map(|(idx, line)| is_top_level_key_line(line, key).then_some(idx));

    if let Some(idx) = first_match {
        if let Some(value) = replacement {
            lines[idx] = value;
            let mut out = String::new();
            for (line_idx, line) in lines.into_iter().enumerate() {
                if line_idx != idx && line_idx < table_start && is_top_level_key_line(&line, key) {
                    continue;
                }
                out.push_str(&line);
            }
            return out;
        }

        let mut out = String::new();
        for (line_idx, line) in lines.into_iter().enumerate() {
            if line_idx < table_start && is_top_level_key_line(&line, key) {
                continue;
            }
            out.push_str(&line);
        }
        return out;
    }

    let Some(value) = replacement else {
        return raw.to_string();
    };
    if table_start < lines.len() {
        lines.insert(table_start, value);
        return lines.concat();
    }
    if raw.is_empty() {
        return value;
    }
    if raw.ends_with('\n') {
        format!("{raw}{value}")
    } else {
        format!("{raw}\n{value}")
    }
}

fn split_lines_preserve_newlines(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in raw.char_indices() {
        if ch == '\n' {
            out.push(raw[start..=idx].to_string());
            start = idx + 1;
        }
    }
    if start < raw.len() {
        out.push(raw[start..].to_string());
    }
    out
}

fn is_top_level_key_line(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return false;
    }
    let Some(rest) = trimmed.strip_prefix(key) else {
        return false;
    };
    matches!(rest.chars().next(), Some(' ') | Some('\t') | Some('='))
}

fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config"),
        std::process::id(),
        nonce
    ));
    fs::write(&tmp, contents).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
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

    #[test]
    fn write_service_tier_updates_top_level_key_without_touching_tables() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            concat!(
                "model = \"gpt-5.4\"\n",
                "service_tier = \"flex\"\n",
                "\n",
                "[profiles.default]\n",
                "model = \"gpt-5.4-mini\"\n",
            ),
        )
        .unwrap();

        write_service_tier_to_config_path(tmp.path(), Some(ServiceTier::Fast)).unwrap();

        let raw = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(raw.contains("service_tier = \"fast\"\n"));
        assert!(raw.contains("[profiles.default]\n"));

        let profile = read_codex_runtime_profile_from_path(tmp.path());
        assert_eq!(profile.service_tier, Some(ServiceTier::Fast));
    }

    #[test]
    fn write_service_tier_removes_key_for_inherit() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            concat!(
                "model = \"gpt-5.4\"\n",
                "service_tier = \"fast\"\n",
                "\n",
                "[profiles.default]\n",
                "model = \"gpt-5.4-mini\"\n",
            ),
        )
        .unwrap();

        write_service_tier_to_config_path(tmp.path(), None).unwrap();

        let raw = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(!raw.contains("service_tier ="));

        let profile = read_codex_runtime_profile_from_path(tmp.path());
        assert_eq!(profile.service_tier, None);
    }

    #[test]
    fn write_model_and_context_updates_top_level_runtime_fields() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            concat!(
                "model = \"gpt-5.4\"\n",
                "model_context_window = 272000\n",
                "\n",
                "[profiles.default]\n",
                "model = \"gpt-5.4-mini\"\n",
            ),
        )
        .unwrap();

        write_model_to_config_path(tmp.path(), Some("gpt-5.5")).unwrap();
        write_context_mode_to_config_path(tmp.path(), Some(ContextMode::OneM)).unwrap();

        let profile = read_codex_runtime_profile_from_path(tmp.path());
        assert_eq!(profile.configured_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(profile.context_mode, Some(ContextMode::OneM));
    }
}
