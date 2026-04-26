//! Shared `codex app-server` stdio JSON-RPC backend.
//!
//! Replaces the per-turn `codex exec` subprocess with a single long-lived
//! `codex app-server` child process that serves every QQ user's conversations
//! as independent threads. Exposes [`AppServerHandle`] — the façade used by
//! `CodexExecutor` — which the existing call sites already speak to.

pub mod approvals;
pub mod client;
pub mod events;
pub mod protocol;
pub mod session;
pub mod supervisor;
pub mod transport;

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::codex::executor::{CompactRequest, ExecutionRequest, ExecutionResult, ExecutionUpdate};

pub use approvals::{
    ApprovalBroker, ApprovalOutcome, ApprovalRequest, CommandApprovalEvent, ElicitationEvent,
    FileChangeApprovalEvent, PermissionsApprovalEvent,
};
pub use protocol::{ApprovalPolicy, ClientInfo};
pub use session::{AppServerSession, RuntimeConfigSignature, TurnPolicy};
pub use supervisor::AppServerSupervisor;

/// Façade the rest of the project uses: holds the supervisor + broker and
/// exposes a single `execute` method mirroring the legacy executor contract.
#[derive(Clone)]
pub struct AppServerHandle {
    pub supervisor: Arc<AppServerSupervisor>,
    pub approvals: Arc<ApprovalBroker>,
    runtime_configs: Arc<Mutex<HashMap<String, RuntimeConfigSignature>>>,
}

impl AppServerHandle {
    pub async fn start(
        codex_binary: std::path::PathBuf,
        codex_home: std::path::PathBuf,
        sqlite_home: std::path::PathBuf,
        path_env: Option<std::ffi::OsString>,
        client_info: ClientInfo,
    ) -> Result<Self> {
        let supervisor =
            AppServerSupervisor::new(codex_binary, codex_home, sqlite_home, path_env, client_info);
        supervisor.start().await?;
        let approvals = ApprovalBroker::new(supervisor.clone());
        approvals.start().await?;
        Ok(Self {
            supervisor,
            approvals,
            runtime_configs: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn execute(
        &self,
        request: ExecutionRequest,
        policy: TurnPolicy,
        cancel_rx: Option<oneshot::Receiver<()>>,
        update_tx: Option<mpsc::UnboundedSender<ExecutionUpdate>>,
    ) -> Result<ExecutionResult> {
        AppServerSession::new(self.supervisor.clone(), self.runtime_configs.clone())
            .execute(request, policy, cancel_rx, update_tx)
            .await
    }

    pub async fn compact_thread(
        &self,
        request: CompactRequest,
        cancel_rx: Option<oneshot::Receiver<()>>,
    ) -> Result<()> {
        AppServerSession::new(self.supervisor.clone(), self.runtime_configs.clone())
            .compact_thread(request, cancel_rx)
            .await
    }

    pub async fn shutdown(&self) {
        self.supervisor.shutdown().await;
    }
}
