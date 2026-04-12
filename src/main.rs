use std::{future::pending, path::PathBuf, sync::Arc};

use anyhow::Result;
use codex_claw::{
    app::App,
    codex::{config_snapshot, executor::CodexExecutor},
    config::AppConfig,
    launcher::{self, CHILD_ARG, ENV_LAUNCHER_ADDR, LAUNCHER_ARG},
    qq::{api::QqApiClient, gateway},
    session::store::SessionStore,
};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args = std::env::args().collect::<Vec<_>>();
    let mut config = AppConfig::load()?;
    normalize_config_paths(&mut config).await?;
    let run_as_child = args.iter().any(|arg| arg == CHILD_ARG);
    let run_as_launcher = args.iter().any(|arg| arg == LAUNCHER_ARG);

    if run_as_child {
        run_bot(config).await?;
        return Ok(());
    }
    if run_as_launcher || config.general.enable_launcher {
        launcher::run_launcher(&config).await?;
        return Ok(());
    }
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
    let app = Arc::new(App::new(config, session, qq_client, codex));
    if let Ok(addr) = std::env::var(ENV_LAUNCHER_ADDR)
        && let Err(err) = launcher::notify_ready(&addr).await
    {
        tracing::warn!("failed to notify launcher ready: {err:#}");
    }
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
