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
    #[serde(default = "default_codex_binary")]
    pub codex_binary: String,
    #[serde(default = "default_model")]
    pub default_model: String,
    #[serde(default)]
    pub default_reasoning_effort: ReasoningEffort,
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
            codex_binary: default_codex_binary(),
            default_model: default_model(),
            default_reasoning_effort: ReasoningEffort::default(),
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
        Ok(())
    }
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

fn default_codex_binary() -> String {
    "codex".to_string()
}

fn default_model() -> String {
    "gpt-5-codex".to_string()
}

fn default_api_base_url() -> String {
    "https://api.sgroup.qq.com".to_string()
}

fn default_token_url() -> String {
    "https://bots.qq.com/app/getAppAccessToken".to_string()
}
