use std::{future::pending, path::PathBuf, sync::Arc};

use anyhow::Result;
use codex_claw::{
    app::App,
    codex::executor::CodexExecutor,
    config::AppConfig,
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

    let mut config = AppConfig::load()?;
    let data_dir = if config.general.data_dir.is_absolute() {
        config.general.data_dir.clone()
    } else {
        std::env::current_dir()?.join(&config.general.data_dir)
    };
    tokio::fs::create_dir_all(&data_dir).await?;
    config.general.data_dir = normalize_path(data_dir)?;
    let session = Arc::new(SessionStore::load_or_init(&config.general.data_dir).await?);
    let qq_client = Arc::new(QqApiClient::new(config.qq.clone())?);
    let codex = Arc::new(CodexExecutor::new(
        config.general.codex_binary.clone(),
        config.general.data_dir.clone(),
    ));
    let app = Arc::new(App::new(config, session, qq_client, codex));
    gateway::spawn_gateway(app.clone());
    pending::<()>().await;
    Ok(())
}

fn normalize_path(path: PathBuf) -> Result<PathBuf> {
    std::fs::canonicalize(&path).or(Ok(path))
}
