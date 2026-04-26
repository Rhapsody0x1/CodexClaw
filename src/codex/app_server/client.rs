//! Typed JSON-RPC client wrapping [`StdioTransport`].
//!
//! Responsibilities:
//! - correlate outbound requests with incoming responses via an auto-
//!   incrementing `id` + oneshot map;
//! - fan out server notifications to subscribers (via `tokio::sync::broadcast`);
//! - route server-initiated requests (approvals, elicitations) to a single
//!   consumer via an mpsc channel;
//! - signal when the reader task exits so the supervisor can respawn the child.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

use anyhow::{Context, Result, anyhow};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value as JsonValue;
use tokio::{
    sync::{Mutex, broadcast, mpsc, oneshot},
    task::JoinHandle,
};
use tracing::{debug, warn};

use super::{
    protocol::{JsonRpcError, Message},
    transport::{self, StdioTransport},
};

type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<JsonValue, JsonRpcError>>>>>;

#[derive(Debug, Clone)]
pub struct Notification {
    pub method: String,
    pub params: JsonValue,
}

#[derive(Debug)]
pub struct ServerRequest {
    pub id: JsonValue,
    pub method: String,
    pub params: JsonValue,
}

pub struct JsonRpcClient {
    transport: Arc<StdioTransport>,
    pending: PendingMap,
    next_id: AtomicI64,
    notifications_tx: broadcast::Sender<Notification>,
    reader_handle: Mutex<Option<JoinHandle<()>>>,
    stderr_handle: Mutex<Option<JoinHandle<()>>>,
}

impl JsonRpcClient {
    /// Construct a client and start its reader. Returns the client plus a
    /// receiver that fires when the reader task exits (child EOF).
    pub async fn start(
        transport: Arc<StdioTransport>,
        notifications_tx: broadcast::Sender<Notification>,
        server_requests_tx: mpsc::Sender<ServerRequest>,
    ) -> Result<(Arc<Self>, oneshot::Receiver<()>)> {
        let reader = transport
            .take_stdout()
            .await
            .context("transport stdout already taken")?;
        let stderr = transport.take_stderr().await;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let pending_for_reader = pending.clone();
        let notifications_for_reader = notifications_tx.clone();

        let (exit_tx, exit_rx) = oneshot::channel();
        let raw_handle = tokio::spawn({
            let pending = pending_for_reader;
            let notifications = notifications_for_reader;
            let server_requests = server_requests_tx.clone();
            async move {
                // Drive the reader loop. `transport::spawn_reader` returns a
                // JoinHandle that completes on EOF; we await that here so we
                // can fire `exit_tx` when it finishes.
                let inner = transport::spawn_reader(reader, move |msg| match msg {
                    Message::Response { id, outcome } => {
                        let id = match id.as_i64() {
                            Some(v) => v,
                            None => {
                                warn!(?id, "response with non-integer id; dropping");
                                return;
                            }
                        };
                        let pending = pending.clone();
                        tokio::spawn(async move {
                            let slot = pending.lock().await.remove(&id);
                            if let Some(tx) = slot {
                                let _ = tx.send(outcome);
                            } else {
                                warn!(id, "unmatched response id");
                            }
                        });
                    }
                    Message::Notification { method, params } => {
                        let _ = notifications.send(Notification { method, params });
                    }
                    Message::Request { id, method, params } => {
                        let tx = server_requests.clone();
                        tokio::spawn(async move {
                            if let Err(err) = tx.send(ServerRequest { id, method, params }).await {
                                warn!(error = %err, "server request channel closed");
                            }
                        });
                    }
                });
                let _ = inner.await;
                let _ = exit_tx.send(());
            }
        });

        let stderr_handle = stderr.map(transport::spawn_stderr_logger);

        let client = Arc::new(Self {
            transport,
            pending,
            next_id: AtomicI64::new(1),
            notifications_tx,
            reader_handle: Mutex::new(Some(raw_handle)),
            stderr_handle: Mutex::new(stderr_handle),
        });
        Ok((client, exit_rx))
    }

    pub async fn request<P, R>(&self, method: &str, params: &P) -> Result<R>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let params_value = serde_json::to_value(params).context("serialize params")?;
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params_value,
        });
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        if let Err(err) = self.transport.write_message(message).await {
            self.pending.lock().await.remove(&id);
            return Err(err);
        }
        let outcome = rx
            .await
            .map_err(|_| anyhow!("app-server transport dropped before response to {method}"))?;
        match outcome {
            Ok(value) => serde_json::from_value(value)
                .with_context(|| format!("deserialize response for {method}")),
            Err(err) => Err(anyhow!(
                "app-server request `{}` failed: {} ({})",
                method,
                err.message,
                err.code
            )),
        }
    }

    pub async fn notify<P: Serialize>(&self, method: &str, params: &P) -> Result<()> {
        let params_value = serde_json::to_value(params).context("serialize params")?;
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params_value,
        });
        self.transport.write_message(msg).await
    }

    /// Fire-and-forget: parameter-less notification (e.g. `initialized`).
    pub async fn notify_empty(&self, method: &str) -> Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        self.transport.write_message(msg).await
    }

    pub async fn respond_ok<R: Serialize>(&self, id: JsonValue, result: &R) -> Result<()> {
        let result = serde_json::to_value(result).context("serialize response result")?;
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        });
        self.transport.write_message(msg).await
    }

    pub async fn respond_err(&self, id: JsonValue, err: JsonRpcError) -> Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": err,
        });
        self.transport.write_message(msg).await
    }

    pub fn subscribe_notifications(&self) -> broadcast::Receiver<Notification> {
        self.notifications_tx.subscribe()
    }

    /// Wake every outstanding request with a disconnect error.
    pub async fn drain_pending_with_disconnect(&self, reason: &str) {
        let mut pending = self.pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(JsonRpcError {
                code: -32000,
                message: format!("app-server disconnected: {reason}"),
                data: None,
            }));
        }
    }

    pub async fn shutdown(&self, reason: &str) {
        debug!(reason, "shutting down JsonRpcClient");
        self.drain_pending_with_disconnect(reason).await;
        if let Some(handle) = self.reader_handle.lock().await.take() {
            handle.abort();
        }
        if let Some(handle) = self.stderr_handle.lock().await.take() {
            handle.abort();
        }
        if let Some(mut child) = self.transport.take_child().await {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::protocol::{ClientInfo, InitializeCapabilities, InitializeParams};
    use serde_json::json;

    #[test]
    fn initialize_request_body_shape() {
        let params = InitializeParams {
            client_info: ClientInfo {
                name: "codex-claw".into(),
                version: "0.0.1".into(),
                title: None,
            },
            capabilities: Some(InitializeCapabilities {
                experimental_api: Some(true),
            }),
        };
        let v = serde_json::to_value(&params).unwrap();
        assert_eq!(v["clientInfo"]["name"], json!("codex-claw"));
        assert_eq!(v["capabilities"]["experimentalApi"], json!(true));
    }
}
