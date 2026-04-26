//! Routes server-initiated approval / elicitation requests.
//!
//! This module listens for server → client requests (`commandExecution/request
//! Approval`, `fileChange/requestApproval`, `permissions/requestApproval`,
//! `mcpServer/elicitation/request`, `account/chatgptAuthTokens/refresh`) and
//! either responds automatically (when no interactive handler is installed)
//! or delegates to a channel-based handler (QQ prompt flow).

use std::sync::Arc;

use anyhow::Result;
use serde_json::Value as JsonValue;
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, info, warn};

use super::{
    client::{JsonRpcClient, ServerRequest},
    protocol::{
        ApprovalDecision, CommandApprovalParams, CommandApprovalResponse, ElicitationResponse,
        FileChangeApprovalParams, FileChangeApprovalResponse, JsonRpcError, McpElicitationParams,
        PermissionsApprovalParams, PermissionsApprovalResponse, SimpleDecision, method,
    },
    supervisor::AppServerSupervisor,
};

/// The decision a handler can return for a command / file-change / permissions
/// approval request.
#[derive(Debug, Clone, Copy)]
pub enum ApprovalOutcome {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

impl ApprovalOutcome {
    pub(crate) fn to_decision(self) -> ApprovalDecision {
        match self {
            ApprovalOutcome::Accept => ApprovalDecision::Simple(SimpleDecision::Accept),
            ApprovalOutcome::AcceptForSession => {
                ApprovalDecision::Simple(SimpleDecision::AcceptForSession)
            }
            ApprovalOutcome::Decline => ApprovalDecision::Simple(SimpleDecision::Decline),
            ApprovalOutcome::Cancel => ApprovalDecision::Simple(SimpleDecision::Cancel),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommandApprovalEvent {
    pub thread_id: String,
    pub item_id: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FileChangeApprovalEvent {
    pub thread_id: String,
    pub reason: Option<String>,
    pub grant_root: Option<String>,
    pub file_changes: JsonValue,
}

#[derive(Debug, Clone)]
pub struct PermissionsApprovalEvent {
    pub thread_id: String,
    pub reason: Option<String>,
    pub permissions: JsonValue,
}

#[derive(Debug, Clone)]
pub struct ElicitationEvent {
    pub thread_id: String,
    pub server: Option<String>,
    pub request: JsonValue,
}

/// Every approval request yields one of these envelopes — the handler fills
/// the oneshot to report its decision. If the handler drops the envelope
/// without responding, the broker replies `Decline` so the turn can progress.
pub enum ApprovalRequest {
    Command {
        event: CommandApprovalEvent,
        reply: oneshot::Sender<ApprovalOutcome>,
    },
    FileChange {
        event: FileChangeApprovalEvent,
        reply: oneshot::Sender<ApprovalOutcome>,
    },
    Permissions {
        event: PermissionsApprovalEvent,
        reply: oneshot::Sender<ApprovalOutcome>,
    },
    Elicitation {
        event: ElicitationEvent,
        /// `Some(content)` accepts, `None` declines.
        reply: oneshot::Sender<Option<JsonValue>>,
    },
}

pub struct ApprovalBroker {
    supervisor: Arc<AppServerSupervisor>,
    handler_tx: Mutex<Option<mpsc::Sender<ApprovalRequest>>>,
}

impl ApprovalBroker {
    pub fn new(supervisor: Arc<AppServerSupervisor>) -> Arc<Self> {
        Arc::new(Self {
            supervisor,
            handler_tx: Mutex::new(None),
        })
    }

    /// Install an interactive handler. Any request delivered while no handler
    /// is installed (or while the handler's channel is full/closed) is
    /// auto-declined.
    pub async fn install_handler(&self, tx: mpsc::Sender<ApprovalRequest>) {
        *self.handler_tx.lock().await = Some(tx);
    }

    /// Launch the background dispatcher. Must be called exactly once after
    /// the supervisor has started.
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        let mut rx = self
            .supervisor
            .take_server_request_receiver()
            .await
            .ok_or_else(|| anyhow::anyhow!("server request receiver already taken"))?;
        let this = self.clone();
        tokio::spawn(async move {
            while let Some(request) = rx.recv().await {
                let this = this.clone();
                tokio::spawn(async move {
                    this.dispatch(request).await;
                });
            }
            debug!("approval broker dispatcher ended");
        });
        Ok(())
    }

    async fn dispatch(&self, req: ServerRequest) {
        let client = match self.supervisor.client().await {
            Ok(c) => c,
            Err(err) => {
                warn!(error = %err, method = %req.method, "no client for approval response");
                return;
            }
        };
        match req.method.as_str() {
            method::COMMAND_EXECUTION_REQUEST_APPROVAL | method::EXEC_COMMAND_APPROVAL => {
                self.dispatch_command(&client, req).await;
            }
            method::FILE_CHANGE_REQUEST_APPROVAL | method::APPLY_PATCH_APPROVAL => {
                self.dispatch_file_change(&client, req).await;
            }
            method::PERMISSIONS_REQUEST_APPROVAL => {
                self.dispatch_permissions(&client, req).await;
            }
            method::MCP_SERVER_ELICITATION_REQUEST => {
                self.dispatch_elicitation(&client, req).await;
            }
            method::CHATGPT_AUTH_TOKENS_REFRESH => {
                let err = JsonRpcError {
                    code: -32001,
                    message: "Token refresh not supported by codex-claw".to_string(),
                    data: None,
                };
                let _ = client.respond_err(req.id, err).await;
                info!("declined chatgpt auth token refresh; user must re-login on host");
            }
            other => {
                warn!(method = %other, "unknown server-initiated request; declining");
                let err = JsonRpcError {
                    code: -32601,
                    message: format!("method not supported: {other}"),
                    data: None,
                };
                let _ = client.respond_err(req.id, err).await;
            }
        }
    }

    async fn dispatch_command(&self, client: &Arc<JsonRpcClient>, req: ServerRequest) {
        let params: CommandApprovalParams = match serde_json::from_value(req.params.clone()) {
            Ok(p) => p,
            Err(err) => {
                warn!(error = %err, "failed to parse command approval params");
                return respond_parse_err(client, req.id).await;
            }
        };
        let has_handler = self.handler_tx.lock().await.is_some();
        info!(
            thread_id = %params.thread_id,
            command = params.command.as_deref().unwrap_or("(missing)"),
            cwd = params.cwd.as_deref().unwrap_or(""),
            reason = params.reason.as_deref().unwrap_or(""),
            has_handler,
            "command approval requested"
        );
        let event = CommandApprovalEvent {
            thread_id: params.thread_id,
            item_id: params.item_id,
            command: params.command,
            cwd: params.cwd,
            reason: params.reason,
        };
        let outcome = self
            .ask_outcome(|tx| ApprovalRequest::Command { event, reply: tx })
            .await;
        let resp = CommandApprovalResponse {
            decision: outcome.to_decision(),
        };
        let _ = client.respond_ok(req.id, &resp).await;
    }

    async fn dispatch_file_change(&self, client: &Arc<JsonRpcClient>, req: ServerRequest) {
        let params: FileChangeApprovalParams = match serde_json::from_value(req.params.clone()) {
            Ok(p) => p,
            Err(err) => {
                warn!(error = %err, "failed to parse file-change approval params");
                return respond_parse_err(client, req.id).await;
            }
        };
        let event = FileChangeApprovalEvent {
            thread_id: params.thread_id,
            reason: params.reason,
            grant_root: params.grant_root,
            file_changes: params.file_changes,
        };
        let outcome = self
            .ask_outcome(|tx| ApprovalRequest::FileChange { event, reply: tx })
            .await;
        let resp = FileChangeApprovalResponse {
            decision: outcome.to_decision(),
        };
        let _ = client.respond_ok(req.id, &resp).await;
    }

    async fn dispatch_permissions(&self, client: &Arc<JsonRpcClient>, req: ServerRequest) {
        let params: PermissionsApprovalParams = match serde_json::from_value(req.params.clone()) {
            Ok(p) => p,
            Err(err) => {
                warn!(error = %err, "failed to parse permissions approval params");
                return respond_parse_err(client, req.id).await;
            }
        };
        let event = PermissionsApprovalEvent {
            thread_id: params.thread_id,
            reason: params.reason,
            permissions: params.permissions,
        };
        let outcome = self
            .ask_outcome(|tx| ApprovalRequest::Permissions { event, reply: tx })
            .await;
        let resp = PermissionsApprovalResponse {
            decision: outcome.to_decision(),
        };
        let _ = client.respond_ok(req.id, &resp).await;
    }

    async fn dispatch_elicitation(&self, client: &Arc<JsonRpcClient>, req: ServerRequest) {
        let params: McpElicitationParams = match serde_json::from_value(req.params.clone()) {
            Ok(p) => p,
            Err(err) => {
                warn!(error = %err, "failed to parse elicitation params");
                return respond_parse_err(client, req.id).await;
            }
        };
        let event = ElicitationEvent {
            thread_id: params.thread_id,
            server: params.server,
            request: params.request,
        };
        let (tx, rx) = oneshot::channel();
        let sent = {
            let guard = self.handler_tx.lock().await;
            if let Some(sender) = guard.as_ref() {
                sender
                    .send(ApprovalRequest::Elicitation { event, reply: tx })
                    .await
                    .is_ok()
            } else {
                false
            }
        };
        let resp = if sent {
            match rx.await {
                Ok(Some(content)) => ElicitationResponse::Accept { content },
                Ok(None) | Err(_) => ElicitationResponse::Decline,
            }
        } else {
            ElicitationResponse::Decline
        };
        let _ = client.respond_ok(req.id, &resp).await;
    }

    async fn ask_outcome<F>(&self, build: F) -> ApprovalOutcome
    where
        F: FnOnce(oneshot::Sender<ApprovalOutcome>) -> ApprovalRequest,
    {
        let (tx, rx) = oneshot::channel();
        let request = build(tx);
        let sent = {
            let guard = self.handler_tx.lock().await;
            if let Some(sender) = guard.as_ref() {
                sender.send(request).await.is_ok()
            } else {
                false
            }
        };
        if !sent {
            return ApprovalOutcome::Decline;
        }
        rx.await.unwrap_or(ApprovalOutcome::Decline)
    }
}

async fn respond_parse_err(client: &Arc<JsonRpcClient>, id: JsonValue) {
    let err = JsonRpcError {
        code: -32602,
        message: "invalid params".to_string(),
        data: None,
    };
    let _ = client.respond_err(id, err).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_converts_to_simple_decision() {
        let d = ApprovalOutcome::AcceptForSession.to_decision();
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v, serde_json::Value::String("acceptForSession".into()));
    }

    #[test]
    fn decline_converts_to_decline_string() {
        let d = ApprovalOutcome::Decline.to_decision();
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v, serde_json::Value::String("decline".into()));
    }
}
