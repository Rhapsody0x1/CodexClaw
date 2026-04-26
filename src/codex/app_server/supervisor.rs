//! Process lifecycle for a long-lived `codex app-server` child.
//!
//! Ensures:
//! - exactly one running child at a time (file lock on the data dir);
//! - automatic respawn with exponential backoff on crash;
//! - `initialize` handshake rerun after each spawn;
//! - atomic client swap so callers always see a live connection.

use std::{ffi::OsString, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use tokio::{
    sync::{Mutex, RwLock, broadcast, mpsc, oneshot},
    time::sleep,
};
use tracing::{error, info, warn};

use super::{
    client::{JsonRpcClient, Notification, ServerRequest},
    protocol::{ClientInfo, InitializeCapabilities, InitializeParams, InitializeResponse, method},
    transport::StdioTransport,
};

const CHANNEL_CAP: usize = 1024;
const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

pub struct AppServerSupervisor {
    codex_binary: PathBuf,
    codex_home: PathBuf,
    sqlite_home: PathBuf,
    path_env: Option<OsString>,
    client_info: ClientInfo,
    notifications_tx: broadcast::Sender<Notification>,
    server_requests_tx: mpsc::Sender<ServerRequest>,
    server_requests_rx: Mutex<Option<mpsc::Receiver<ServerRequest>>>,
    client: RwLock<Option<Arc<JsonRpcClient>>>,
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
}

impl AppServerSupervisor {
    pub fn new(
        codex_binary: PathBuf,
        codex_home: PathBuf,
        sqlite_home: PathBuf,
        path_env: Option<OsString>,
        client_info: ClientInfo,
    ) -> Arc<Self> {
        let (notifications_tx, _) = broadcast::channel(CHANNEL_CAP);
        let (server_requests_tx, server_requests_rx) = mpsc::channel(64);
        Arc::new(Self {
            codex_binary,
            codex_home,
            sqlite_home,
            path_env,
            client_info,
            notifications_tx,
            server_requests_tx,
            server_requests_rx: Mutex::new(Some(server_requests_rx)),
            client: RwLock::new(None),
            shutdown_tx: Mutex::new(None),
        })
    }

    /// Launch the supervisor loop. Spawns the child, performs `initialize`,
    /// and kicks off a background task that respawns on EOF.
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        let (client, exit_rx) = self.spawn_and_initialize().await?;
        *self.client.write().await = Some(client);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        let sup = self.clone();
        tokio::spawn(async move {
            sup.run_respawn_loop(exit_rx, shutdown_rx).await;
        });
        Ok(())
    }

    async fn run_respawn_loop(
        self: Arc<Self>,
        mut exit_rx: oneshot::Receiver<()>,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) {
        let mut backoff = BACKOFF_MIN;
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    info!("supervisor shutdown requested");
                    if let Some(client) = self.client.write().await.take() {
                        client.shutdown("supervisor stopped").await;
                    }
                    return;
                }
                _ = &mut exit_rx => {
                    warn!("app-server stdout closed; respawning");
                    if let Some(client) = self.client.write().await.take() {
                        client.drain_pending_with_disconnect("app-server exited").await;
                        // Kill child if still around.
                        client.shutdown("respawn").await;
                    }
                    loop {
                        sleep(backoff).await;
                        match self.spawn_and_initialize().await {
                            Ok((client, new_exit_rx)) => {
                                *self.client.write().await = Some(client);
                                exit_rx = new_exit_rx;
                                backoff = BACKOFF_MIN;
                                info!("app-server respawned");
                                break;
                            }
                            Err(err) => {
                                error!(error = %err, "failed to respawn app-server");
                                backoff = (backoff * 2).min(BACKOFF_MAX);
                            }
                        }
                    }
                }
            }
        }
    }

    async fn spawn_and_initialize(
        self: &Arc<Self>,
    ) -> Result<(Arc<JsonRpcClient>, oneshot::Receiver<()>)> {
        let transport = StdioTransport::spawn(
            &self.codex_binary,
            &self.codex_home,
            &self.sqlite_home,
            self.path_env.as_deref(),
        )
        .context("spawn app-server")?;
        let transport = Arc::new(transport);
        let (client, exit_rx) = JsonRpcClient::start(
            transport,
            self.notifications_tx.clone(),
            self.server_requests_tx.clone(),
        )
        .await
        .context("start JSON-RPC client")?;

        let params = InitializeParams {
            client_info: self.client_info.clone(),
            capabilities: Some(InitializeCapabilities {
                experimental_api: Some(true),
            }),
        };
        let resp: InitializeResponse = client
            .request("initialize", &params)
            .await
            .context("initialize handshake")?;
        if let Some(home) = resp.codex_home.as_deref() {
            let expected = self.codex_home.to_string_lossy();
            if home != expected {
                warn!(
                    server_codex_home = home,
                    expected = %expected,
                    "app-server reported different CODEX_HOME than requested"
                );
            }
        }
        client
            .notify_empty(method::INITIALIZED)
            .await
            .context("initialized notification")?;
        Ok((client, exit_rx))
    }

    pub async fn client(&self) -> Result<Arc<JsonRpcClient>> {
        self.client
            .read()
            .await
            .clone()
            .context("app-server client not ready")
    }

    pub fn subscribe_notifications(&self) -> broadcast::Receiver<Notification> {
        self.notifications_tx.subscribe()
    }

    /// Take the single receiver for server-initiated requests. Should only be
    /// called by the `ApprovalBroker` at startup.
    pub async fn take_server_request_receiver(&self) -> Option<mpsc::Receiver<ServerRequest>> {
        self.server_requests_rx.lock().await.take()
    }

    pub async fn shutdown(&self) {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
        // Also actively drop the client so anyone holding a reference sees None.
        if let Some(client) = self.client.write().await.take() {
            client.shutdown("supervisor shutdown").await;
        }
    }
}
