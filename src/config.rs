use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    codex::provider_config::{CodexProviderSpec, default_grok_model_ids},
    session::state::ReasoningEffort,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    pub qq: QqConfig,
    #[serde(default)]
    pub shadow: ShadowSection,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    /// Alternate backend for Grok / xAI-style OpenAI-compatible APIs.
    /// Used at bootstrap when `enabled`, and by `/switch_model` when toggling to Grok.
    #[serde(default)]
    pub codex_provider: CodexProviderConfig,
    /// Preferred backend when `/switch_model` selects Codex (usually a third-party
    /// OpenAI-compatible mirror, NOT api.openai.com). When unset, switch tries to
    /// re-activate a non-Grok provider already present in isolated config.toml.
    #[serde(default)]
    pub openai_provider: OpenAiProviderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowSection {
    #[serde(default = "default_shadow_enabled")]
    pub enabled: bool,
    #[serde(default = "default_shadow_min_user_chars")]
    pub memory_min_user_chars: usize,
    #[serde(default = "default_shadow_reasoning")]
    pub memory_reasoning: String,
    #[serde(default)]
    pub memory_model: String,
    #[serde(default = "default_shadow_deadline_secs")]
    pub memory_deadline_secs: u64,
    #[serde(default = "default_shadow_files_threshold")]
    pub skill_files_threshold: usize,
    #[serde(default = "default_shadow_tool_threshold")]
    pub skill_tool_threshold: usize,
}

impl Default for ShadowSection {
    fn default() -> Self {
        Self {
            enabled: default_shadow_enabled(),
            memory_min_user_chars: default_shadow_min_user_chars(),
            memory_reasoning: default_shadow_reasoning(),
            memory_model: String::new(),
            memory_deadline_secs: default_shadow_deadline_secs(),
            skill_files_threshold: default_shadow_files_threshold(),
            skill_tool_threshold: default_shadow_tool_threshold(),
        }
    }
}

fn default_shadow_enabled() -> bool {
    true
}
fn default_shadow_min_user_chars() -> usize {
    40
}
fn default_shadow_reasoning() -> String {
    "low".to_string()
}
fn default_shadow_deadline_secs() -> u64 {
    120
}
fn default_shadow_files_threshold() -> usize {
    2
}
fn default_shadow_tool_threshold() -> usize {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    #[serde(default = "default_scheduler_enabled")]
    pub enabled: bool,
    #[serde(default = "default_scheduler_tick_secs")]
    pub tick_secs: u64,
    #[serde(default = "default_scheduler_default_tz")]
    pub default_tz: String,
    #[serde(default = "default_scheduler_max_concurrent_jobs")]
    pub max_concurrent_jobs: usize,
    #[serde(default = "default_scheduler_max_turn_secs")]
    pub max_turn_secs: u64,
    #[serde(default = "default_scheduler_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_scheduler_retry_backoff_secs")]
    pub retry_backoff_secs: u64,
    #[serde(default = "default_scheduler_circuit_breaker_threshold")]
    pub circuit_breaker_threshold: u32,
    #[serde(default = "default_scheduler_runs_retention")]
    pub runs_retention: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: default_scheduler_enabled(),
            tick_secs: default_scheduler_tick_secs(),
            default_tz: default_scheduler_default_tz(),
            max_concurrent_jobs: default_scheduler_max_concurrent_jobs(),
            max_turn_secs: default_scheduler_max_turn_secs(),
            max_attempts: default_scheduler_max_attempts(),
            retry_backoff_secs: default_scheduler_retry_backoff_secs(),
            circuit_breaker_threshold: default_scheduler_circuit_breaker_threshold(),
            runs_retention: default_scheduler_runs_retention(),
        }
    }
}

fn default_scheduler_enabled() -> bool {
    true
}
fn default_scheduler_tick_secs() -> u64 {
    30
}
fn default_scheduler_default_tz() -> String {
    "Asia/Shanghai".to_string()
}
fn default_scheduler_max_concurrent_jobs() -> usize {
    4
}
fn default_scheduler_max_turn_secs() -> u64 {
    600
}
fn default_scheduler_max_attempts() -> u32 {
    3
}
fn default_scheduler_retry_backoff_secs() -> u64 {
    30
}
fn default_scheduler_circuit_breaker_threshold() -> u32 {
    5
}
fn default_scheduler_runs_retention() -> usize {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default = "default_system_codex_home")]
    pub system_codex_home: PathBuf,
    #[serde(default = "default_global_codex_home")]
    pub codex_home_global: PathBuf,
    #[serde(default = "default_workspace_dir")]
    pub default_workspace_dir: PathBuf,
    #[serde(default = "default_codex_binary")]
    pub codex_binary: String,
    #[serde(default = "default_model")]
    pub default_model: String,
    #[serde(default)]
    pub default_reasoning_effort: ReasoningEffort,
    #[serde(default = "default_self_repo_dir")]
    pub self_repo_dir: PathBuf,
    #[serde(default = "default_self_build_command")]
    pub self_build_command: String,
    #[serde(default = "default_self_binary_path")]
    pub self_binary_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QqConfig {
    pub app_id: String,
    pub app_secret: String,
    #[serde(default = "default_api_base_url")]
    pub api_base_url: String,
    #[serde(default = "default_token_url")]
    pub token_url: String,
}

/// Optional rewrite of the isolated Codex home `config.toml` so App-Server
/// can use xAI Grok (or another OpenAI-compatible provider) instead of only
/// the default OpenAI/Codex provider.
///
/// Defaults target xAI (`https://api.x.ai/v1`, `XAI_API_KEY`, `wire_api = "responses"`).
/// Enable with `enabled = true` after exporting `XAI_API_KEY`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexProviderConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_codex_provider_id")]
    pub id: String,
    #[serde(default = "default_codex_provider_name")]
    pub name: String,
    #[serde(default = "default_codex_provider_base_url")]
    pub base_url: String,
    #[serde(default = "default_codex_provider_env_key")]
    pub env_key: String,
    #[serde(default = "default_codex_provider_wire_api")]
    pub wire_api: String,
    /// When true, write top-level `model_provider` (and optional `model`).
    #[serde(default = "default_codex_provider_set_as_default")]
    pub set_as_default: bool,
    /// Top-level `model` written into Codex config when `set_as_default` is true.
    #[serde(default = "default_codex_provider_default_model")]
    pub default_model: Option<String>,
    /// Extra model ids merged into the `/model` picker when this provider is enabled.
    #[serde(default = "default_codex_provider_models")]
    pub models: Vec<String>,
}

impl Default for CodexProviderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            id: default_codex_provider_id(),
            name: default_codex_provider_name(),
            base_url: default_codex_provider_base_url(),
            env_key: default_codex_provider_env_key(),
            wire_api: default_codex_provider_wire_api(),
            set_as_default: default_codex_provider_set_as_default(),
            default_model: default_codex_provider_default_model(),
            models: default_codex_provider_models(),
        }
    }
}

impl CodexProviderConfig {
    pub fn to_spec(&self) -> CodexProviderSpec {
        CodexProviderSpec {
            id: self.id.clone(),
            name: self.name.clone(),
            base_url: self.base_url.clone(),
            env_key: self.env_key.clone(),
            wire_api: self.wire_api.clone(),
            set_as_default: self.set_as_default,
            default_model: self.default_model.clone(),
            models: self.models.clone(),
            requires_openai_auth: None,
            preferred_auth_method: None,
        }
    }

    pub fn enabled_spec(&self) -> Option<CodexProviderSpec> {
        if self.enabled {
            Some(self.to_spec())
        } else {
            None
        }
    }
}

/// Third-party (or official) OpenAI-compatible provider used for the **Codex** side
/// of `/switch_model`. Typical example: a relay with `base_url` like
/// `https://chat.soruxgpt.com/codex` and `env_key = "OPENAI_API_KEY"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiProviderConfig {
    /// When true (and base_url/id set), `/switch_model` → Codex applies this provider
    /// instead of clearing `model_provider` (which would hit official api.openai.com).
    #[serde(default = "default_openai_provider_enabled")]
    pub enabled: bool,
    #[serde(default = "default_openai_provider_id")]
    pub id: String,
    #[serde(default = "default_openai_provider_name")]
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default = "default_openai_provider_env_key")]
    pub env_key: String,
    #[serde(default = "default_openai_provider_wire_api")]
    pub wire_api: String,
    #[serde(default = "default_openai_provider_default_model")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub requires_openai_auth: Option<bool>,
    #[serde(default)]
    pub preferred_auth_method: Option<String>,
}

impl Default for OpenAiProviderConfig {
    fn default() -> Self {
        Self {
            enabled: default_openai_provider_enabled(),
            id: default_openai_provider_id(),
            name: default_openai_provider_name(),
            base_url: String::new(),
            env_key: default_openai_provider_env_key(),
            wire_api: default_openai_provider_wire_api(),
            default_model: default_openai_provider_default_model(),
            requires_openai_auth: None,
            preferred_auth_method: None,
        }
    }
}

impl OpenAiProviderConfig {
    pub fn to_spec(&self) -> Option<CodexProviderSpec> {
        if !self.enabled || self.base_url.trim().is_empty() || self.id.trim().is_empty() {
            return None;
        }
        Some(CodexProviderSpec {
            id: self.id.clone(),
            name: if self.name.trim().is_empty() {
                self.id.clone()
            } else {
                self.name.clone()
            },
            base_url: self.base_url.clone(),
            env_key: self.env_key.clone(),
            wire_api: self.wire_api.clone(),
            set_as_default: true,
            default_model: self.default_model.clone(),
            models: Vec::new(),
            requires_openai_auth: self.requires_openai_auth,
            preferred_auth_method: self.preferred_auth_method.clone(),
        })
    }
}

fn default_openai_provider_enabled() -> bool {
    // Enabled by default only when base_url is later filled; empty base_url → to_spec() is None.
    true
}
fn default_openai_provider_id() -> String {
    "mirror".to_string()
}
fn default_openai_provider_name() -> String {
    "mirror".to_string()
}
fn default_openai_provider_env_key() -> String {
    "OPENAI_API_KEY".to_string()
}
fn default_openai_provider_wire_api() -> String {
    "responses".to_string()
}
fn default_openai_provider_default_model() -> Option<String> {
    Some("gpt-5.5".to_string())
}

fn default_codex_provider_id() -> String {
    "xai".to_string()
}
fn default_codex_provider_name() -> String {
    "xAI Grok".to_string()
}
fn default_codex_provider_base_url() -> String {
    "https://api.x.ai/v1".to_string()
}
fn default_codex_provider_env_key() -> String {
    "XAI_API_KEY".to_string()
}
fn default_codex_provider_wire_api() -> String {
    "responses".to_string()
}
fn default_codex_provider_set_as_default() -> bool {
    true
}
fn default_codex_provider_default_model() -> Option<String> {
    Some("grok-4".to_string())
}
fn default_codex_provider_models() -> Vec<String> {
    default_grok_model_ids()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            qq: QqConfig {
                app_id: String::new(),
                app_secret: String::new(),
                api_base_url: default_api_base_url(),
                token_url: default_token_url(),
            },
            shadow: ShadowSection::default(),
            scheduler: SchedulerConfig::default(),
            codex_provider: CodexProviderConfig::default(),
            openai_provider: OpenAiProviderConfig::default(),
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            system_codex_home: default_system_codex_home(),
            codex_home_global: default_global_codex_home(),
            default_workspace_dir: default_workspace_dir(),
            codex_binary: default_codex_binary(),
            default_model: default_model(),
            default_reasoning_effort: ReasoningEffort::default(),
            self_repo_dir: default_self_repo_dir(),
            self_build_command: default_self_build_command(),
            self_binary_path: default_self_binary_path(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let path = std::env::var("CODEX_CLAW_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("codexclaw.toml"));
        if !path.exists() && path == PathBuf::from("codexclaw.toml") {
            let fallback = default_codex_claw_root().join("codexclaw.toml");
            if fallback.exists() {
                return Self::load_from_path(&fallback);
            }
        }
        Self::load_from_path(&path)
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file at {}", path.display()))?;
        let config = toml::from_str::<Self>(&raw)
            .with_context(|| format!("failed to parse TOML config at {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.qq.app_id.trim().is_empty(),
            "qq.app_id must not be empty"
        );
        anyhow::ensure!(
            !self.qq.app_secret.trim().is_empty(),
            "qq.app_secret must not be empty"
        );
        anyhow::ensure!(
            !self.general.self_build_command.trim().is_empty(),
            "general.self_build_command must not be empty"
        );
        if self.codex_provider.enabled {
            anyhow::ensure!(
                !self.codex_provider.id.trim().is_empty(),
                "codex_provider.id must not be empty when codex_provider.enabled is true"
            );
            anyhow::ensure!(
                !self.codex_provider.base_url.trim().is_empty(),
                "codex_provider.base_url must not be empty when codex_provider.enabled is true"
            );
            anyhow::ensure!(
                !self.codex_provider.env_key.trim().is_empty(),
                "codex_provider.env_key must not be empty when codex_provider.enabled is true"
            );
            anyhow::ensure!(
                !self.codex_provider.wire_api.trim().is_empty(),
                "codex_provider.wire_api must not be empty when codex_provider.enabled is true"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn load_parses_codex_provider_grok_block() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            r#"
[qq]
app_id = "app"
app_secret = "secret"

[codex_provider]
enabled = true
id = "xai"
base_url = "https://api.x.ai/v1"
env_key = "XAI_API_KEY"
wire_api = "responses"
default_model = "grok-4"
models = ["grok-4", "grok-3-mini"]
"#,
        )
        .unwrap();

        let config = AppConfig::load_from_path(tmp.path()).unwrap();
        assert!(config.codex_provider.enabled);
        assert_eq!(config.codex_provider.id, "xai");
        assert_eq!(config.codex_provider.base_url, "https://api.x.ai/v1");
        assert_eq!(config.codex_provider.env_key, "XAI_API_KEY");
        let spec = config.codex_provider.enabled_spec().unwrap();
        assert_eq!(spec.default_model.as_deref(), Some("grok-4"));
        assert!(spec.models.iter().any(|m| m == "grok-3-mini"));
    }

    #[test]
    fn load_without_codex_provider_keeps_openai_defaults() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            r#"
[qq]
app_id = "app"
app_secret = "secret"
"#,
        )
        .unwrap();

        let config = AppConfig::load_from_path(tmp.path()).unwrap();
        assert!(!config.codex_provider.enabled);
        assert!(config.codex_provider.enabled_spec().is_none());
        // Defaults remain xAI-shaped but inactive until enabled.
        assert_eq!(config.codex_provider.base_url, "https://api.x.ai/v1");
        assert_eq!(config.general.default_model, "gpt-5.4");
    }
}

fn default_data_dir() -> PathBuf {
    default_codex_claw_root().join("data")
}

fn default_global_codex_home() -> PathBuf {
    default_codex_claw_root().join(".codex")
}

fn default_system_codex_home() -> PathBuf {
    home_dir().join(".codex")
}

fn default_workspace_dir() -> PathBuf {
    default_data_dir().join("session").join("workspace")
}

fn default_codex_binary() -> String {
    "codex".to_string()
}

fn default_model() -> String {
    "gpt-5.4".to_string()
}

fn default_self_repo_dir() -> PathBuf {
    PathBuf::from(".")
}

fn default_self_build_command() -> String {
    "cargo build --release".to_string()
}

fn default_self_binary_path() -> PathBuf {
    PathBuf::from("./target/release/codex-claw")
}

fn default_api_base_url() -> String {
    "https://sandbox.api.sgroup.qq.com".to_string()
}

fn default_token_url() -> String {
    "https://bots.qq.com/app/getAppAccessToken".to_string()
}

fn default_codex_claw_root() -> PathBuf {
    home_dir().join(".codex-claw")
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/root"))
}
