use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use codex_claw::{
    app::App,
    codex::{AppServerHandle, ClientInfo, CodexExecutor, build_codex_path_env, config_snapshot},
    config::AppConfig,
    memory::store::MemoryStore,
    qq::{api::QqApiClient, gateway},
    scheduler,
    session::store::SessionStore,
    shadow::{ShadowConfig, ShadowWorker, SkillShadowConfig},
    skills::index::SkillIndex,
    util::{layout::DataLayout, path::home_dir},
};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args
        .first()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        print_usage();
        return Ok(());
    }
    let mut config = AppConfig::load()?;
    normalize_config_paths(&mut config).await?;
    // Startup self-check used by the self-update smoke test: exercises arg
    // handling + config load/normalization (the common startup-panic surface)
    // and exits 0 without starting the service. Placed after config load so a
    // bad config is caught before a freshly built binary is installed.
    if args.first().is_some_and(|arg| arg == "--smoke-test") {
        println!("codex-claw smoke test ok");
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "cron") {
        scheduler::cli::run(&args[1..], &config).await?;
        return Ok(());
    }
    run_bot(config).await
}

fn print_usage() {
    println!(
        "usage: codex-claw [cron <command>]\n\n\
         Without arguments, starts the QQ bot service.\n\
         Commands:\n\
         cron add|once|list|rm|pause|resume|run-now|tail"
    );
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

    // Launch the shared `codex app-server` child process. All QQ users' turns
    // are dispatched as independent threads over this single connection.
    let path_env = build_codex_path_env(
        std::env::var_os("PATH").as_ref(),
        std::env::var_os("HOME")
            .as_deref()
            .map(std::path::Path::new),
    );
    let client_info = ClientInfo {
        name: "codex-claw".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        title: None,
    };
    let codex_sqlite_home = config.general.codex_home_global.join("sqlite");
    tokio::fs::create_dir_all(&codex_sqlite_home).await?;
    let app_server = Arc::new(
        AppServerHandle::start(
            PathBuf::from(config.general.codex_binary.clone()),
            config.general.codex_home_global.clone(),
            codex_sqlite_home,
            path_env,
            client_info,
        )
        .await?,
    );
    let codex = Arc::new(CodexExecutor::new(
        config.general.codex_binary.clone(),
        config.general.data_dir.clone(),
        app_server,
    ));
    let layout = DataLayout::new(&config.general.data_dir);
    let memory = Arc::new(MemoryStore::new(layout.memory_dir()));
    let skills_root = config.general.codex_home_global.join("skills");
    tokio::fs::create_dir_all(&skills_root).await.ok();
    let skill_index = Arc::new(SkillIndex::new(skills_root.clone()));
    let shadow_workspace = layout.shadow_workspace_dir();
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
    let app = App::new(config, session, qq_client, codex, memory, shadow);
    scheduler::Scheduler::spawn(app.clone());
    gateway::spawn_gateway(app.clone());

    wait_for_shutdown_signal().await;
    tracing::info!("shutdown signal received, terminating app-server child");
    // Give the supervisor a chance to kill+reap the codex app-server child so it
    // is not orphaned against the shared CODEX_HOME (a second app-server started
    // after restart would corrupt the shared SQLite/rollout state).
    app.codex.handle().shutdown().await;
    Ok(())
}

/// Wait for SIGTERM (service stop / supervisor restart) or SIGINT (Ctrl-C).
/// On non-unix platforms this falls back to Ctrl-C only.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(sig) => sig,
            Err(err) => {
                tracing::warn!(%err, "failed to install SIGTERM handler; waiting on Ctrl-C only");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
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
        return home_dir();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    path
}
