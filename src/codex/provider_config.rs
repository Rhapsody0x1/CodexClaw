//! Apply a non-OpenAI model provider (e.g. xAI Grok) into Codex `config.toml`.
//!
//! CodexClaw still drives the Codex App-Server harness; this module only rewrites
//! the isolated `CODEX_HOME` config so the harness talks to an OpenAI-compatible
//! backend such as `https://api.x.ai/v1`.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Spec written into Codex `config.toml` as `model_provider` + `[model_providers.<id>]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexProviderSpec {
    /// Provider id used for top-level `model_provider` and table key.
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub env_key: String,
    pub wire_api: String,
    /// When true, set top-level `model_provider` (and optional `model`).
    pub set_as_default: bool,
    pub default_model: Option<String>,
    /// Extra model ids surfaced in the `/model` picker when this provider is active.
    pub models: Vec<String>,
    /// Optional Codex provider flags (third-party relays often need these).
    pub requires_openai_auth: Option<bool>,
    pub preferred_auth_method: Option<String>,
}

impl CodexProviderSpec {
    /// Defaults for xAI Grok (OpenAI-compatible Responses API).
    pub fn xai_grok() -> Self {
        Self {
            id: "xai".to_string(),
            name: "xAI Grok".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            env_key: "XAI_API_KEY".to_string(),
            wire_api: "responses".to_string(),
            set_as_default: true,
            default_model: Some("grok-4".to_string()),
            models: default_grok_model_ids(),
            requires_openai_auth: None,
            preferred_auth_method: None,
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.id.trim().is_empty() && !self.base_url.trim().is_empty()
    }
}

/// Canonical Grok model ids for picker / catalog merge.
pub fn default_grok_model_ids() -> Vec<String> {
    vec![
        "grok-4".to_string(),
        "grok-4.5".to_string(),
        "grok-3".to_string(),
        "grok-3-mini".to_string(),
    ]
}

/// Pure transform: merge `provider` into an existing Codex `config.toml` body.
///
/// - Upserts `[model_providers.<id>]` with `name`, `base_url`, `env_key`, `wire_api`.
/// - When `set_as_default`, sets top-level `model_provider` and optional `model`.
/// - Leaves unrelated keys intact (parsed via `toml::Value`).
pub fn apply_model_provider_to_config(raw: &str, provider: &CodexProviderSpec) -> Result<String> {
    let mut root: toml::Value = if raw.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(raw).context("failed to parse Codex config.toml while applying provider")?
    };

    let table = root
        .as_table_mut()
        .context("Codex config.toml root must be a table")?;

    if provider.set_as_default {
        table.insert(
            "model_provider".to_string(),
            toml::Value::String(provider.id.clone()),
        );
        if let Some(model) = provider
            .default_model
            .as_ref()
            .map(|m| m.trim())
            .filter(|m| !m.is_empty())
        {
            table.insert("model".to_string(), toml::Value::String(model.to_string()));
        }
    }

    let providers = table
        .entry("model_providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let providers_table = providers
        .as_table_mut()
        .context("model_providers must be a table")?;

    let mut provider_table = toml::map::Map::new();
    provider_table.insert(
        "name".to_string(),
        toml::Value::String(provider.name.clone()),
    );
    provider_table.insert(
        "base_url".to_string(),
        toml::Value::String(provider.base_url.clone()),
    );
    provider_table.insert(
        "env_key".to_string(),
        toml::Value::String(provider.env_key.clone()),
    );
    provider_table.insert(
        "wire_api".to_string(),
        toml::Value::String(provider.wire_api.clone()),
    );
    if let Some(requires) = provider.requires_openai_auth {
        provider_table.insert(
            "requires_openai_auth".to_string(),
            toml::Value::Boolean(requires),
        );
    }
    if let Some(method) = provider
        .preferred_auth_method
        .as_ref()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
    {
        provider_table.insert(
            "preferred_auth_method".to_string(),
            toml::Value::String(method.to_string()),
        );
    }
    providers_table.insert(provider.id.clone(), toml::Value::Table(provider_table));

    // Prefer a stable, readable layout for the managed isolated home.
    toml::to_string_pretty(&root).context("failed to serialize Codex config.toml")
}

/// Read `config.toml` under `codex_home`, apply `provider`, write back.
pub fn apply_model_provider_to_codex_home(
    codex_home: &Path,
    provider: &CodexProviderSpec,
) -> Result<()> {
    let path = codex_home.join("config.toml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let updated = apply_model_provider_to_config(&raw, provider)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(&path, updated)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Which LLM backend the isolated Codex home is currently pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Built-in OpenAI / ChatGPT Codex path (no custom `model_provider`, or non-Grok id).
    Codex,
    /// Custom OpenAI-compatible provider configured for Grok / xAI-style backends.
    Grok,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Grok => "grok",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex (OpenAI)",
            Self::Grok => "Grok",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::Codex => Self::Grok,
            Self::Grok => Self::Codex,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "codex" | "openai" | "gpt" | "default" => Some(Self::Codex),
            "grok" | "xai" | "x-ai" => Some(Self::Grok),
            _ => None,
        }
    }
}

/// Detect backend from Codex `config.toml` body and optional Grok provider id.
pub fn detect_backend_from_config(raw: &str, grok_provider_id: &str) -> BackendKind {
    let Ok(value) = toml::from_str::<toml::Value>(raw) else {
        return BackendKind::Codex;
    };
    let table = match value.as_table() {
        Some(t) => t,
        None => return BackendKind::Codex,
    };
    if let Some(provider) = table
        .get("model_provider")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if provider.eq_ignore_ascii_case(grok_provider_id)
            || provider.eq_ignore_ascii_case("xai")
            || provider.eq_ignore_ascii_case("grok")
        {
            return BackendKind::Grok;
        }
        // Custom non-Grok provider still treated as Codex/OpenAI-side for toggle purposes.
        return BackendKind::Codex;
    }
    if let Some(model) = table.get("model").and_then(|v| v.as_str()) {
        if looks_like_grok_model(model) {
            return BackendKind::Grok;
        }
    }
    BackendKind::Codex
}

pub fn looks_like_grok_model(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.starts_with("grok") || m.contains("grok-") || m == "xai"
}

/// Result of applying a backend switch to Codex config.toml.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendSwitchResult {
    pub target: BackendKind,
    pub model: String,
    pub config_toml: String,
}

/// Switch isolated Codex config between Codex and Grok backends.
///
/// - **Grok**: upserts the Grok provider table and sets it as active.
/// - **Codex**: prefers `codex` (e.g. third-party mirror like `chat.soruxgpt.com/codex`);
///   if that is missing, re-activates any non-Grok provider already in the file;
///   only if neither exists, clears `model_provider` (built-in OpenAI — usually wrong
///   when the operator uses a third-party key).
///
/// Both provider tables are kept so toggling back and forth does not lose config.
pub fn switch_backend_in_config(
    raw: &str,
    target: BackendKind,
    grok: &CodexProviderSpec,
    codex: Option<&CodexProviderSpec>,
    codex_model_fallback: &str,
) -> Result<BackendSwitchResult> {
    match target {
        BackendKind::Grok => {
            // Ensure the Codex/mirror provider table remains present for later toggle-back.
            let mut working = raw.to_string();
            if let Some(codex_spec) = codex.filter(|s| s.is_configured()) {
                let mut keep = codex_spec.clone();
                keep.set_as_default = false;
                keep.default_model = None;
                working = apply_model_provider_to_config(&working, &keep)?;
            }

            let mut spec = grok.clone();
            spec.set_as_default = true;
            if spec
                .default_model
                .as_ref()
                .map(|m| m.trim().is_empty())
                .unwrap_or(true)
            {
                spec.default_model = Some("grok-4.5".to_string());
            }
            let config_toml = apply_model_provider_to_config(&working, &spec)?;
            let model = spec
                .default_model
                .clone()
                .unwrap_or_else(|| "grok-4.5".to_string());
            Ok(BackendSwitchResult {
                target,
                model,
                config_toml,
            })
        }
        BackendKind::Codex => {
            let fallback_model = {
                let trimmed = codex_model_fallback.trim();
                if trimmed.is_empty() || looks_like_grok_model(trimmed) {
                    "gpt-5.5".to_string()
                } else {
                    trimmed.to_string()
                }
            };

            if let Some(spec) = codex.filter(|s| s.is_configured()) {
                let mut spec = spec.clone();
                spec.set_as_default = true;
                if spec
                    .default_model
                    .as_ref()
                    .map(|m| m.trim().is_empty())
                    .unwrap_or(true)
                {
                    spec.default_model = Some(fallback_model.clone());
                }
                let config_toml = apply_model_provider_to_config(raw, &spec)?;
                let model = spec.default_model.unwrap_or(fallback_model);
                return Ok(BackendSwitchResult {
                    target,
                    model,
                    config_toml,
                });
            }

            if let Some(recovered) = recover_non_grok_provider(raw, grok.id.as_str()) {
                let mut recovered = recovered;
                recovered.set_as_default = true;
                if recovered
                    .default_model
                    .as_ref()
                    .map(|m| m.trim().is_empty())
                    .unwrap_or(true)
                {
                    recovered.default_model = Some(fallback_model.clone());
                }
                let config_toml = apply_model_provider_to_config(raw, &recovered)?;
                let model = recovered.default_model.unwrap_or(fallback_model);
                return Ok(BackendSwitchResult {
                    target,
                    model,
                    config_toml,
                });
            }

            // Last resort: built-in OpenAI path (often wrong for third-party keys).
            let config_toml = clear_active_model_provider(raw, Some(&fallback_model))?;
            Ok(BackendSwitchResult {
                target,
                model: fallback_model,
                config_toml,
            })
        }
    }
}

/// Rebuild a provider spec from an existing non-Grok `[model_providers.*]` entry.
pub fn recover_non_grok_provider(raw: &str, grok_provider_id: &str) -> Option<CodexProviderSpec> {
    let value: toml::Value = toml::from_str(raw).ok()?;
    let table = value.as_table()?;
    let providers = table.get("model_providers")?.as_table()?;
    let top_model = table
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // Prefer current model_provider if it is not Grok.
    if let Some(active) = table
        .get("model_provider")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !active.eq_ignore_ascii_case(grok_provider_id)
            && !active.eq_ignore_ascii_case("xai")
            && !active.eq_ignore_ascii_case("grok")
        {
            if let Some(spec) =
                provider_table_to_spec(active, providers.get(active)?, top_model.clone())
            {
                return Some(spec);
            }
        }
    }

    for (id, entry) in providers {
        if id.eq_ignore_ascii_case(grok_provider_id)
            || id.eq_ignore_ascii_case("xai")
            || id.eq_ignore_ascii_case("grok")
        {
            continue;
        }
        if let Some(spec) = provider_table_to_spec(id, entry, top_model.clone()) {
            return Some(spec);
        }
    }
    None
}

fn provider_table_to_spec(
    id: &str,
    entry: &toml::Value,
    default_model: Option<String>,
) -> Option<CodexProviderSpec> {
    let t = entry.as_table()?;
    let base_url = t.get("base_url")?.as_str()?.trim().to_string();
    if base_url.is_empty() {
        return None;
    }
    let name = t
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(id)
        .to_string();
    let env_key = t
        .get("env_key")
        .and_then(|v| v.as_str())
        .unwrap_or("OPENAI_API_KEY")
        .to_string();
    let wire_api = t
        .get("wire_api")
        .and_then(|v| v.as_str())
        .unwrap_or("responses")
        .to_string();
    let requires_openai_auth = t.get("requires_openai_auth").and_then(|v| v.as_bool());
    let preferred_auth_method = t
        .get("preferred_auth_method")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let model = default_model.filter(|m| !looks_like_grok_model(m));
    Some(CodexProviderSpec {
        id: id.to_string(),
        name,
        base_url,
        env_key,
        wire_api,
        set_as_default: true,
        default_model: model,
        models: Vec::new(),
        requires_openai_auth,
        preferred_auth_method,
    })
}

/// Remove top-level `model_provider` so Codex uses the built-in OpenAI path.
/// Optionally set top-level `model`. Keeps `[model_providers.*]` tables intact.
pub fn clear_active_model_provider(raw: &str, model: Option<&str>) -> Result<String> {
    let mut root: toml::Value = if raw.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(raw).context("failed to parse Codex config.toml while clearing provider")?
    };
    let table = root
        .as_table_mut()
        .context("Codex config.toml root must be a table")?;
    table.remove("model_provider");
    if let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) {
        table.insert("model".to_string(), toml::Value::String(model.to_string()));
    }
    toml::to_string_pretty(&root).context("failed to serialize Codex config.toml")
}

/// Write a backend switch into `codex_home/config.toml`.
pub fn switch_backend_in_codex_home(
    codex_home: &Path,
    target: BackendKind,
    grok: &CodexProviderSpec,
    codex: Option<&CodexProviderSpec>,
    codex_model_fallback: &str,
) -> Result<BackendSwitchResult> {
    let path = codex_home.join("config.toml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let result = switch_backend_in_config(&raw, target, grok, codex, codex_model_fallback)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(&path, &result.config_toml)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(result)
}

/// Model ids that should appear in the picker when this provider is enabled.
pub fn provider_model_ids(provider: &CodexProviderSpec) -> Vec<String> {
    fn push_unique(out: &mut Vec<String>, name: &str) {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return;
        }
        if !out
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(trimmed))
        {
            out.push(trimmed.to_string());
        }
    }

    let mut out = Vec::new();
    if let Some(model) = provider.default_model.as_deref() {
        push_unique(&mut out, model);
    }
    for model in &provider.models {
        push_unique(&mut out, model);
    }
    if out.is_empty() {
        for model in default_grok_model_ids() {
            push_unique(&mut out, &model);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn apply_writes_xai_provider_block_and_defaults() {
        let raw = r#"
model = "gpt-5.4"
service_tier = "flex"

[profiles.default]
model = "gpt-5.4-mini"
"#;
        let updated = apply_model_provider_to_config(raw, &CodexProviderSpec::xai_grok()).unwrap();
        let parsed: toml::Value = toml::from_str(&updated).unwrap();
        let table = parsed.as_table().unwrap();

        assert_eq!(
            table.get("model_provider").and_then(|v| v.as_str()),
            Some("xai")
        );
        assert_eq!(table.get("model").and_then(|v| v.as_str()), Some("grok-4"));
        // Unrelated keys preserved
        assert_eq!(
            table.get("service_tier").and_then(|v| v.as_str()),
            Some("flex")
        );
        assert!(table.contains_key("profiles"));

        let provider = table
            .get("model_providers")
            .and_then(|v| v.get("xai"))
            .and_then(|v| v.as_table())
            .expect("xai provider table");
        assert_eq!(
            provider.get("base_url").and_then(|v| v.as_str()),
            Some("https://api.x.ai/v1")
        );
        assert_eq!(
            provider.get("env_key").and_then(|v| v.as_str()),
            Some("XAI_API_KEY")
        );
        assert_eq!(
            provider.get("wire_api").and_then(|v| v.as_str()),
            Some("responses")
        );
        assert_eq!(
            provider.get("name").and_then(|v| v.as_str()),
            Some("xAI Grok")
        );
    }

    #[test]
    fn apply_without_set_as_default_only_upserts_provider_table() {
        let raw = "model = \"gpt-5.4\"\n";
        let mut spec = CodexProviderSpec::xai_grok();
        spec.set_as_default = false;
        let updated = apply_model_provider_to_config(raw, &spec).unwrap();
        let parsed: toml::Value = toml::from_str(&updated).unwrap();
        let table = parsed.as_table().unwrap();
        assert!(table.get("model_provider").is_none());
        assert_eq!(table.get("model").and_then(|v| v.as_str()), Some("gpt-5.4"));
        assert!(
            table
                .get("model_providers")
                .and_then(|v| v.get("xai"))
                .is_some()
        );
    }

    #[test]
    fn apply_to_codex_home_creates_config_toml() {
        let dir = tempdir().unwrap();
        apply_model_provider_to_codex_home(dir.path(), &CodexProviderSpec::xai_grok()).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(raw.contains("api.x.ai"));
        assert!(raw.contains("XAI_API_KEY"));
        assert!(raw.contains("model_provider"));
    }

    #[test]
    fn provider_model_ids_includes_default_and_extras() {
        let mut spec = CodexProviderSpec::xai_grok();
        spec.models = vec!["grok-2".into(), "grok-4".into()];
        let ids = provider_model_ids(&spec);
        assert!(ids.iter().any(|m| m == "grok-4"));
        assert!(ids.iter().any(|m| m == "grok-2"));
        assert_eq!(ids.iter().filter(|m| *m == "grok-4").count(), 1);
    }

    #[test]
    fn openai_path_unchanged_when_provider_not_applied() {
        // Sanity: empty apply is not called; original OpenAI-oriented config stays as-is.
        let raw = "model = \"gpt-5.4\"\n";
        let parsed: toml::Value = toml::from_str(raw).unwrap();
        assert_eq!(
            parsed.get("model").and_then(|v| v.as_str()),
            Some("gpt-5.4")
        );
        assert!(parsed.get("model_providers").is_none());
    }

    #[test]
    fn switch_toggles_between_codex_mirror_and_grok() {
        let mut grok = CodexProviderSpec::xai_grok();
        grok.default_model = Some("grok-4.5".into());
        grok.base_url = "https://api.x.ai/v1".into();

        let mirror = CodexProviderSpec {
            id: "mirror".into(),
            name: "mirror".into(),
            base_url: "https://codex-mirror.example/v1".into(),
            env_key: "OPENAI_API_KEY".into(),
            wire_api: "responses".into(),
            set_as_default: true,
            default_model: Some("gpt-5.5".into()),
            models: vec![],
            requires_openai_auth: Some(false),
            preferred_auth_method: Some("apikey".into()),
        };

        let start = r#"
model = "gpt-5.5"
model_provider = "mirror"

[model_providers.mirror]
name = "mirror"
base_url = "https://codex-mirror.example/v1"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
requires_openai_auth = false
preferred_auth_method = "apikey"
"#;

        let to_grok =
            switch_backend_in_config(start, BackendKind::Grok, &grok, Some(&mirror), "gpt-5.5")
                .unwrap();
        assert_eq!(to_grok.target, BackendKind::Grok);
        assert_eq!(to_grok.model, "grok-4.5");
        let parsed: toml::Value = toml::from_str(&to_grok.config_toml).unwrap();
        assert_eq!(
            parsed.get("model_provider").and_then(|v| v.as_str()),
            Some("xai")
        );
        // Mirror table must survive so switch-back still works.
        assert!(
            parsed
                .get("model_providers")
                .and_then(|v| v.get("mirror"))
                .is_some()
        );

        let to_codex = switch_backend_in_config(
            &to_grok.config_toml,
            BackendKind::Codex,
            &grok,
            Some(&mirror),
            "gpt-5.5",
        )
        .unwrap();
        assert_eq!(to_codex.target, BackendKind::Codex);
        assert_eq!(to_codex.model, "gpt-5.5");
        let parsed: toml::Value = toml::from_str(&to_codex.config_toml).unwrap();
        assert_eq!(
            parsed.get("model_provider").and_then(|v| v.as_str()),
            Some("mirror")
        );
        assert_eq!(
            parsed
                .get("model_providers")
                .and_then(|v| v.get("mirror"))
                .and_then(|v| v.get("base_url"))
                .and_then(|v| v.as_str()),
            Some("https://codex-mirror.example/v1")
        );
        assert!(!to_codex.config_toml.contains("api.openai.com"));
    }

    #[test]
    fn switch_to_codex_recovers_mirror_from_file_without_explicit_spec() {
        let grok = CodexProviderSpec::xai_grok();
        let raw = r#"
model = "grok-4.5"
model_provider = "xai"

[model_providers.xai]
base_url = "https://api.x.ai/v1"
env_key = "XAI_API_KEY"
wire_api = "responses"

[model_providers.mirror]
name = "mirror"
base_url = "https://codex-mirror.example/v1"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
"#;
        let to_codex =
            switch_backend_in_config(raw, BackendKind::Codex, &grok, None, "gpt-5.5").unwrap();
        let parsed: toml::Value = toml::from_str(&to_codex.config_toml).unwrap();
        assert_eq!(
            parsed.get("model_provider").and_then(|v| v.as_str()),
            Some("mirror")
        );
    }

    #[test]
    fn detect_backend_from_grok_model_name_without_provider() {
        assert_eq!(
            detect_backend_from_config("model = \"grok-4.5\"\n", "xai"),
            BackendKind::Grok
        );
        assert_eq!(
            detect_backend_from_config("model = \"gpt-5.5\"\n", "xai"),
            BackendKind::Codex
        );
    }
}
