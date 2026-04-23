use std::{future::pending, path::PathBuf, sync::Arc};

use anyhow::Result;
use codex_claw::{
    app::App,
    codex::{config_snapshot, executor::CodexExecutor},
    config::AppConfig,
    memory::store::MemoryStore,
    qq::{api::QqApiClient, gateway},
    session::store::SessionStore,
    shadow::{ShadowConfig, ShadowWorker, SkillShadowConfig},
    skills::index::SkillIndex,
};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let mut config = AppConfig::load()?;
    normalize_config_paths(&mut config).await?;
    run_bot(config).await
}

async fn run_bot(config: AppConfig) -> Result<()> {
    tokio::fs::create_dir_all(&config.general.data_dir).await?;
    tokio::fs::create_dir_all(&config.general.codex_home_global).await?;
    config_snapshot::bootstrap_codex_home(
        &config.general.codex_home_global,
        &config.general.system_codex_home,
    )
    .await?;

    let session = Arc::new(
        SessionStore::load_or_init(
            &config.general.data_dir,
            &config.general.codex_home_global,
            &config.general.system_codex_home,
            &config.general.default_workspace_dir,
        )
        .await?,
    );
    let imported_count = session
        .import_sessions_for_workspace(&config.general.self_repo_dir)
        .await?;
    tracing::info!(
        imported_count,
        workspace = %config.general.self_repo_dir.display(),
        "auto-imported self repo sessions from system codex home"
    );
    let qq_client = Arc::new(QqApiClient::new(config.qq.clone())?);
    let codex = Arc::new(CodexExecutor::new(
        config.general.codex_binary.clone(),
        config.general.data_dir.clone(),
    ));
    let memory = Arc::new(MemoryStore::new(config.general.data_dir.join("memory")));
    let skills_root = config.general.codex_home_global.join("skills");
    tokio::fs::create_dir_all(&skills_root).await.ok();
    let skill_index = Arc::new(SkillIndex::new(skills_root.clone()));
    let shadow_workspace = config.general.data_dir.join("shadow-workspace");
    tokio::fs::create_dir_all(&shadow_workspace).await.ok();
    let shadow = if config.shadow.enabled {
        let memory_model = if config.shadow.memory_model.trim().is_empty() {
            None
        } else {
            Some(config.shadow.memory_model.clone())
        };
        let memory_cfg = ShadowConfig {
            min_user_msg_chars: config.shadow.memory_min_user_chars,
            model_override: memory_model,
            reasoning: config.shadow.memory_reasoning.clone(),
            deadline: std::time::Duration::from_secs(config.shadow.memory_deadline_secs),
        };
        let skill_cfg = SkillShadowConfig {
            files_threshold: config.shadow.skill_files_threshold,
            tool_threshold: config.shadow.skill_tool_threshold,
        };
        Some(Arc::new(ShadowWorker::new(
            memory.clone(),
            skill_index.clone(),
            skills_root,
            config.general.codex_binary.clone(),
            config.general.codex_home_global.clone(),
            shadow_workspace,
            memory_cfg,
            skill_cfg,
        )))
    } else {
        None
    };
    let app = Arc::new(App::new(config, session, qq_client, codex, memory, shadow));
    gateway::spawn_gateway(app.clone());
    pending::<()>().await;
    Ok(())
}

async fn normalize_config_paths(config: &mut AppConfig) -> Result<()> {
    config.general.data_dir = normalize_path(config.general.data_dir.clone()).await?;
    config.general.codex_home_global =
        normalize_path(config.general.codex_home_global.clone()).await?;
    config.general.system_codex_home =
        normalize_path(config.general.system_codex_home.clone()).await?;
    config.general.default_workspace_dir =
        normalize_path(config.general.default_workspace_dir.clone()).await?;
    config.general.self_repo_dir = normalize_path(config.general.self_repo_dir.clone()).await?;
    config.general.self_binary_path = if config.general.self_binary_path.is_absolute() {
        config.general.self_binary_path.clone()
    } else {
        normalize_path(
            config
                .general
                .self_repo_dir
                .join(&config.general.self_binary_path),
        )
        .await?
    };
    Ok(())
}

async fn normalize_path(path: PathBuf) -> Result<PathBuf> {
    let expanded = expand_tilde(path);
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()?.join(expanded)
    };
    tokio::fs::create_dir_all(
        absolute
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".")),
    )
    .await
    .ok();
    std::fs::canonicalize(&absolute).or(Ok(absolute))
}

fn expand_tilde(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/root"));
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/root"));
        return home.join(rest);
    }
    path
}
