use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::session::state::ReasoningEffort;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    pub qq: QqConfig,
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
    #[serde(default = "default_launcher_control_addr")]
    pub launcher_control_addr: String,
    #[serde(default = "default_enable_launcher")]
    pub enable_launcher: bool,
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
            launcher_control_addr: default_launcher_control_addr(),
            enable_launcher: default_enable_launcher(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let path = std::env::var("CODEX_CLAW_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("codexclaw.toml"));
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
        anyhow::ensure!(
            !self.general.launcher_control_addr.trim().is_empty(),
            "general.launcher_control_addr must not be empty"
        );
        Ok(())
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
    "gpt-5-codex".to_string()
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

fn default_launcher_control_addr() -> String {
    "127.0.0.1:8765".to_string()
}

fn default_enable_launcher() -> bool {
    true
}

fn default_api_base_url() -> String {
    "https://api.sgroup.qq.com".to_string()
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
