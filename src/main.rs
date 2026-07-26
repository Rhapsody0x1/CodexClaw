use std::{future::Future, path::PathBuf, sync::Arc};

use anyhow::Result;
use codex_claw::{
    DataLayout,
    app::App,
    codex::{AppServerHandle, ClientInfo, CodexExecutor, build_codex_path_env, config_snapshot},
    config::AppConfig,
    memory::MemoryStore,
    qq::{C2CMessageEvent, QqApiClient, spawn_gateway},
    scheduler::{self, SchedulerCtx},
    session::SessionStore,
    shadow::{ShadowConfig, ShadowWorker},
};
use tokio::sync::mpsc;
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
    config.normalize_paths().await?;
    // Standalone startup self-check: exercises arg handling + config
    // load/normalization and exits 0 without starting the service. No longer
    // invoked by /self-update (older binaries treat unknown args as "start the
    // bot", which made the pre-replace smoke test hazardous on downgrades);
    // kept so a stray `--smoke-test` from an old script exits fast instead of
    // booting a second bot instance.
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
    let codex = Arc::new(CodexExecutor::new(app_server));
    let layout = DataLayout::new(&config.general.data_dir);
    let memory = Arc::new(MemoryStore::new(layout.memory_dir()));
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
        Some(Arc::new(ShadowWorker::new(
            memory.clone(),
            config.general.codex_binary.clone(),
            config.general.codex_home_global.clone(),
            shadow_workspace,
            memory_cfg,
        )))
    } else {
        None
    };
    // Composition root for the scheduler: build its slice of the app here and
    // hand the strong reference to the `App`; the tick loop only keeps a
    // `Weak`, so the scheduler parks itself once the `App` is dropped.
    let scheduler_ctx = Arc::new(SchedulerCtx {
        config: config.clone(),
        session: session.clone(),
        codex: codex.clone(),
        notifier: qq_client.clone(),
    });
    let app = App::new(
        config,
        session,
        qq_client,
        codex,
        memory,
        shadow,
        scheduler_ctx.clone(),
    );
    scheduler::Scheduler::spawn(scheduler_ctx);
    let (c2c_tx, c2c_rx) = mpsc::unbounded_channel();
    spawn_gateway(
        app.config.general.data_dir.clone(),
        app.qq_client.clone(),
        c2c_tx,
    );
    spawn_c2c_consumer(app.clone(), c2c_rx);

    wait_for_shutdown_signal().await;
    tracing::info!("shutdown signal received, terminating app-server child");
    // Give the supervisor a chance to kill+reap the codex app-server child so it
    // is not orphaned against the shared CODEX_HOME (a second app-server started
    // after restart would corrupt the shared SQLite/rollout state).
    app.codex.handle().shutdown().await;
    Ok(())
}

/// Drain C2C events produced by the QQ gateway into `App::handle_c2c_event`.
fn spawn_c2c_consumer(app: Arc<App>, events: mpsc::UnboundedReceiver<C2CMessageEvent>) {
    spawn_dispatch_loop(events, move |event| {
        let app = app.clone();
        async move {
            if let Err(err) = app.handle_c2c_event(event).await {
                tracing::warn!("failed to process c2c message from gateway: {err:#}");
            }
        }
    });
}

/// Receive from `events` and hand every item to `handle` on its own detached
/// task.
///
/// The per-item `tokio::spawn` is load-bearing, not incidental: the QQ gateway
/// used to spawn inline at the receive site, and handling a single message can
/// occupy a Codex turn for minutes. Awaiting `handle` in this loop instead
/// would let one slow message stall every message behind it.
fn spawn_dispatch_loop<T, F, Fut>(mut events: mpsc::UnboundedReceiver<T>, handle: F)
where
    T: Send + 'static,
    F: Fn(T) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        while let Some(item) = events.recv().await {
            tokio::spawn(handle(item));
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The dispatch loop must not serialize handlers: a handler that never
    /// finishes may not keep the items behind it from starting.
    #[tokio::test]
    async fn dispatch_loop_does_not_await_handlers_serially() {
        let started = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::unbounded_channel::<usize>();
        let (done_tx, mut done_rx) = mpsc::unbounded_channel::<usize>();

        let counter = started.clone();
        spawn_dispatch_loop(rx, move |item| {
            let counter = counter.clone();
            let done_tx = done_tx.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                if item == 0 {
                    // Stands in for a long-running turn.
                    std::future::pending::<()>().await;
                }
                let _ = done_tx.send(item);
            }
        });

        for item in 0..3 {
            tx.send(item).expect("consumer alive");
        }

        // The two items queued behind the stalled one still complete. A serial
        // `handle(item).await` loop would hang here instead, so bound the wait
        // to fail loudly rather than block the suite.
        let recv_two = async {
            vec![
                done_rx.recv().await.expect("second item ran"),
                done_rx.recv().await.expect("third item ran"),
            ]
        };
        let mut finished = tokio::time::timeout(std::time::Duration::from_secs(5), recv_two)
            .await
            .expect("handlers ran concurrently");
        finished.sort_unstable();
        assert_eq!(finished, vec![1, 2]);
        assert_eq!(started.load(Ordering::SeqCst), 3);
    }

    /// Dropping the receiver ends the loop instead of leaking a live task.
    #[tokio::test]
    async fn dispatch_loop_stops_when_sender_is_dropped() {
        let (tx, rx) = mpsc::unbounded_channel::<usize>();
        let (done_tx, mut done_rx) = mpsc::unbounded_channel::<usize>();
        spawn_dispatch_loop(rx, move |item| {
            let done_tx = done_tx.clone();
            async move {
                let _ = done_tx.send(item);
            }
        });

        tx.send(7).expect("consumer alive");
        assert_eq!(done_rx.recv().await, Some(7));
        drop(tx);
        // The loop exits, dropping its clone of `done_tx` and closing the channel.
        assert_eq!(done_rx.recv().await, None);
    }
}
