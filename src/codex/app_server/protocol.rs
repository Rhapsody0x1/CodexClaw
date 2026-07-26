//! Hand-copied subset of the `codex app-server` JSON-RPC protocol.
//!
//! Sources (reference):
//! - `/tmp/openai-codex/codex-rs/app-server-protocol/src/protocol/{common,v1,v2}.rs`
//! - `/tmp/openai-codex/codex-rs/protocol/src/{config_types,plan_tool}.rs`
//!
//! Field names and enum tagging match the wire format emitted by the installed
//! `codex app-server` binary (verified empirically with `/tmp/codex-probe`).
//!
//! `dead_code` is allowed for the whole module on purpose. These types are a
//! transcription of somebody else's schema, so their shape is dictated by the
//! wire format rather than by what this crate happens to read today: response
//! fields we do not consume still document what the server sends, and request
//! fields must stay even when nothing sets them, because dropping one changes
//! the JSON we emit. Types that are *entirely* unreferenced are still deleted —
//! the allow covers reserved fields and variants, not orphaned definitions.
#![allow(dead_code)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

// ---------------------------------------------------------------------------
// JSON-RPC envelope
// ---------------------------------------------------------------------------

/// The app-server omits the `jsonrpc` field from responses/notifications and
/// accepts messages with or without it, so we serialize it as optional.
#[derive(Debug, Clone)]
pub(crate) enum Message {
    /// Client → server (or server → client) request expecting a response.
    Request {
        id: JsonValue,
        method: String,
        params: JsonValue,
    },
    /// Response to a prior request.
    Response {
        id: JsonValue,
        outcome: Result<JsonValue, JsonRpcError>,
    },
    /// One-way notification.
    Notification { method: String, params: JsonValue },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JsonRpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<JsonValue>,
}

// ---------------------------------------------------------------------------
// initialize
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeParams {
    pub(crate) client_info: ClientInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) capabilities: Option<InitializeCapabilities>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) experimental_api: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeResponse {
    #[serde(default)]
    pub(crate) user_agent: Option<String>,
    #[serde(default)]
    pub(crate) codex_home: Option<String>,
    #[serde(default)]
    pub(crate) platform_family: Option<String>,
    #[serde(default)]
    pub(crate) platform_os: Option<String>,
}

// ---------------------------------------------------------------------------
// thread / turn
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadStartParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) approvals_reviewer: Option<ApprovalsReviewer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sandbox: Option<SandboxMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) permissions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) runtime_workspace_roots: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) service_tier: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) config: HashMap<String, JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadStartResponse {
    pub(crate) thread: Thread,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) approval_policy: Option<ApprovalPolicy>,
    #[serde(default)]
    pub(crate) cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadResumeParams {
    pub(crate) thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) approvals_reviewer: Option<ApprovalsReviewer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sandbox: Option<SandboxMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) permissions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) runtime_workspace_roots: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) service_tier: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) config: HashMap<String, JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadResumeResponse {
    pub(crate) thread: Thread,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadUnsubscribeParams {
    pub(crate) thread_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadUnsubscribeResponse {
    pub(crate) status: ThreadUnsubscribeStatus,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ThreadUnsubscribeStatus {
    NotLoaded,
    NotSubscribed,
    Unsubscribed,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Thread {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) cwd: Option<String>,
    #[serde(default)]
    pub(crate) ephemeral: bool,
    #[serde(default)]
    pub(crate) path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnStartParams {
    pub(crate) thread_id: String,
    pub(crate) input: Vec<TurnInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) approvals_reviewer: Option<ApprovalsReviewer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sandbox_policy: Option<SandboxPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) permissions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) runtime_workspace_roots: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) service_tier: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) collaboration_mode: Option<CollaborationMode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum TurnInputItem {
    Text { text: String },
    LocalImage { path: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnStartResponse {
    pub(crate) turn: Turn,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Turn {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(default)]
    pub(crate) error: Option<JsonValue>,
    #[serde(default)]
    pub(crate) duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnInterruptParams {
    pub(crate) thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) turn_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct TurnInterruptResponse {}

// ---------------------------------------------------------------------------
// Approval & sandbox
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ApprovalPolicy {
    #[serde(rename = "untrusted")]
    UnlessTrusted,
    OnFailure,
    OnRequest,
    Never,
}

impl ApprovalPolicy {
    fn as_wire_str(self) -> &'static str {
        match self {
            ApprovalPolicy::UnlessTrusted => "untrusted",
            ApprovalPolicy::OnFailure => "on-failure",
            ApprovalPolicy::OnRequest => "on-request",
            ApprovalPolicy::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApprovalsReviewer {
    User,
    AutoReview,
    GuardianSubagent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum SandboxPolicy {
    ReadOnly {
        #[serde(rename = "networkAccess")]
        #[serde(default)]
        network_access: bool,
    },
    WorkspaceWrite {
        #[serde(rename = "writableRoots")]
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        writable_roots: Vec<String>,
        #[serde(rename = "networkAccess")]
        #[serde(default)]
        network_access: bool,
        #[serde(default)]
        #[serde(rename = "excludeTmpdirEnvVar")]
        exclude_tmpdir_env_var: bool,
        #[serde(rename = "excludeSlashTmp")]
        #[serde(default)]
        exclude_slash_tmp: bool,
    },
    DangerFullAccess,
}

// ---------------------------------------------------------------------------
// Collaboration mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollaborationMode {
    pub(crate) mode: ModeKind,
    pub(crate) settings: CollaborationSettings,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ModeKind {
    Default,
    Plan,
    Execute,
    PairProgramming,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollaborationSettings {
    pub(crate) model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) developer_instructions: Option<String>,
}

// ---------------------------------------------------------------------------
// Server notifications
// ---------------------------------------------------------------------------

/// Notification method names we care about (compared as `&str` at dispatch
/// time so unknown methods don't crash the client).
pub(crate) mod method {
    pub(crate) const THREAD_COMPACT_START: &str = "thread/compact/start";
    pub(crate) const THREAD_TOKEN_USAGE_UPDATED: &str = "thread/tokenUsage/updated";
    pub(crate) const THREAD_COMPACTED: &str = "thread/compacted";
    pub(crate) const THREAD_CLOSED: &str = "thread/closed";
    pub(crate) const TURN_COMPLETED: &str = "turn/completed";
    pub(crate) const TURN_FAILED: &str = "turn/failed";
    pub(crate) const TURN_PLAN_UPDATED: &str = "turn/planUpdated";
    pub(crate) const ITEM_STARTED: &str = "item/started";
    pub(crate) const ITEM_UPDATED: &str = "item/updated";
    pub(crate) const ITEM_COMPLETED: &str = "item/completed";
    pub(crate) const MODEL_REROUTED: &str = "model/rerouted";
    pub(crate) const ERROR: &str = "error";
    pub(crate) const INITIALIZED: &str = "initialized";

    /// Internal, codex-claw-synthesized notification broadcast by the
    /// supervisor when the app-server child exits, so in-flight turns can abort
    /// promptly instead of waiting out the output-idle timeout. Namespaced so it
    /// can never collide with a real app-server method.
    pub(crate) const BACKEND_DISCONNECTED: &str = "codexclaw/internal/backendDisconnected";

    // Server-initiated request methods (require a response).
    pub(crate) const COMMAND_EXECUTION_REQUEST_APPROVAL: &str =
        "item/commandExecution/requestApproval";
    pub(crate) const FILE_CHANGE_REQUEST_APPROVAL: &str = "item/fileChange/requestApproval";
    pub(crate) const PERMISSIONS_REQUEST_APPROVAL: &str = "item/permissions/requestApproval";
    pub(crate) const APPLY_PATCH_APPROVAL: &str = "applyPatchApproval";
    pub(crate) const EXEC_COMMAND_APPROVAL: &str = "execCommandApproval";
    pub(crate) const MCP_SERVER_ELICITATION_REQUEST: &str = "mcpServer/elicitation/request";
    pub(crate) const CHATGPT_AUTH_TOKENS_REFRESH: &str = "account/chatgptAuthTokens/refresh";
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnCompletedNotification {
    pub(crate) thread_id: String,
    pub(crate) turn: TurnCompleted,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnCompleted {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) error: Option<TurnError>,
    #[serde(default)]
    pub(crate) duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnError {
    #[serde(default)]
    pub(crate) message: Option<String>,
    #[serde(default)]
    #[serde(rename = "type")]
    pub(crate) kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ItemNotification {
    pub(crate) thread_id: String,
    #[serde(default)]
    pub(crate) turn_id: Option<String>,
    pub(crate) item: ItemPayload,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ItemPayload {
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(rename = "type", default)]
    pub(crate) item_type: String,
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) phase: Option<String>,
    #[serde(default)]
    pub(crate) summary: Option<JsonValue>,
    #[serde(default)]
    pub(crate) content: Option<JsonValue>,
    #[serde(default)]
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) cwd: Option<String>,
    #[serde(default)]
    pub(crate) aggregated_output: Option<String>,
    #[serde(default)]
    pub(crate) exit_code: Option<i32>,
    #[serde(default)]
    pub(crate) query: Option<String>,
    #[serde(default)]
    pub(crate) action: Option<JsonValue>,
    #[serde(default)]
    pub(crate) changes: Vec<FileChange>,
    #[serde(default)]
    pub(crate) server: Option<String>,
    #[serde(default)]
    pub(crate) tool: Option<String>,
    #[serde(default)]
    pub(crate) arguments: Option<JsonValue>,
    #[serde(default)]
    pub(crate) result: Option<JsonValue>,
    #[serde(default)]
    pub(crate) error: Option<JsonValue>,
    #[serde(default)]
    pub(crate) prompt: Option<String>,
    #[serde(default)]
    pub(crate) sender_thread_id: Option<String>,
    #[serde(default)]
    pub(crate) receiver_thread_ids: Vec<String>,
    #[serde(default)]
    pub(crate) message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FileChange {
    #[serde(default)]
    pub(crate) path: String,
    pub(crate) kind: PatchChangeKindWire,
    #[serde(default)]
    pub(crate) diff: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum PatchChangeKindWire {
    Legacy(String),
    Structured(PatchChangeKind),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum PatchChangeKind {
    Add,
    Delete,
    Update {
        #[serde(default)]
        move_path: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TokenUsageUpdatedNotification {
    pub(crate) thread_id: String,
    #[serde(default)]
    pub(crate) turn_id: Option<String>,
    pub(crate) token_usage: TokenUsagePayload,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TokenUsagePayload {
    pub(crate) total: TokenCountBucket,
    pub(crate) last: TokenCountBucket,
    #[serde(default)]
    pub(crate) model_context_window: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TokenCountBucket {
    #[serde(default)]
    pub(crate) total_tokens: u64,
    #[serde(default)]
    pub(crate) input_tokens: u64,
    #[serde(default)]
    pub(crate) cached_input_tokens: u64,
    #[serde(default)]
    pub(crate) output_tokens: u64,
    #[serde(default)]
    pub(crate) reasoning_output_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnPlanUpdatedNotification {
    pub(crate) thread_id: String,
    #[serde(default)]
    pub(crate) turn_id: Option<String>,
    #[serde(default)]
    pub(crate) plan: Vec<TurnPlanStep>,
    #[serde(default)]
    pub(crate) explanation: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnPlanStep {
    #[serde(default)]
    pub(crate) step: Option<String>,
    #[serde(default)]
    pub(crate) status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelReroutedNotification {
    #[serde(default)]
    pub(crate) thread_id: Option<String>,
    #[serde(default)]
    pub(crate) from_model: Option<String>,
    #[serde(default)]
    pub(crate) to_model: Option<String>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompactedNotification {
    pub(crate) thread_id: String,
    #[serde(default)]
    pub(crate) turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadCompactStartParams {
    pub(crate) thread_id: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct ThreadCompactStartResponse {}

// ---------------------------------------------------------------------------
// Server-initiated requests (approvals & elicitations)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandApprovalParams {
    pub(crate) thread_id: String,
    #[serde(default)]
    pub(crate) turn_id: Option<String>,
    #[serde(default)]
    pub(crate) item_id: Option<String>,
    #[serde(default)]
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) cwd: Option<String>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) command_actions: Vec<JsonValue>,
    #[serde(default)]
    pub(crate) proposed_execpolicy_amendment: Option<JsonValue>,
    #[serde(default)]
    pub(crate) available_decisions: Vec<JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileChangeApprovalParams {
    pub(crate) thread_id: String,
    #[serde(default)]
    pub(crate) turn_id: Option<String>,
    #[serde(default)]
    pub(crate) item_id: Option<String>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) grant_root: Option<String>,
    #[serde(default)]
    pub(crate) file_changes: JsonValue,
    #[serde(default)]
    pub(crate) available_decisions: Vec<JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PermissionsApprovalParams {
    pub(crate) thread_id: String,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) permissions: JsonValue,
    #[serde(default)]
    pub(crate) available_decisions: Vec<JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpElicitationParams {
    pub(crate) thread_id: String,
    #[serde(default)]
    pub(crate) server: Option<String>,
    #[serde(default)]
    pub(crate) request: JsonValue,
}

/// The decision variants the server accepts. Derived from the installed
/// app-server binary's serde error: "expected one of accept, acceptForSession,
/// acceptWithExecpolicyAmendment, applyNetworkPolicyAmendment, decline, cancel".
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub(crate) enum ApprovalDecision {
    Simple(SimpleDecision),
    WithAmendment(AmendedDecision),
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SimpleDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) enum AmendedDecision {
    #[serde(rename = "acceptWithExecpolicyAmendment")]
    AcceptWithExecpolicyAmendment { execpolicy_amendment: Vec<String> },
    #[serde(rename = "applyNetworkPolicyAmendment")]
    ApplyNetworkPolicyAmendment {
        #[serde(default)]
        detail: Option<JsonValue>,
    },
}

/// Response body shared by command / file-change / permissions approvals —
/// each replies with the same single `decision` field.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ApprovalDecisionResponse {
    pub(crate) decision: ApprovalDecision,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub(crate) enum ElicitationResponse {
    Accept { content: JsonValue },
    Decline,
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_params_serializes_camel_case() {
        let p = InitializeParams {
            client_info: ClientInfo {
                name: "codex-claw".into(),
                version: "0.1.0".into(),
                title: None,
            },
            capabilities: Some(InitializeCapabilities {
                experimental_api: Some(true),
            }),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["clientInfo"]["name"], "codex-claw");
        assert_eq!(v["capabilities"]["experimentalApi"], true);
    }

    #[test]
    fn approval_policy_wire_strs_match_server() {
        assert_eq!(ApprovalPolicy::OnRequest.as_wire_str(), "on-request");
        assert_eq!(ApprovalPolicy::Never.as_wire_str(), "never");
        assert_eq!(ApprovalPolicy::UnlessTrusted.as_wire_str(), "untrusted");
    }

    #[test]
    fn approval_decision_serializes_as_bare_string() {
        let d = ApprovalDecision::Simple(SimpleDecision::Decline);
        let v = serde_json::to_string(&d).unwrap();
        assert_eq!(v, "\"decline\"");

        let d = ApprovalDecision::Simple(SimpleDecision::AcceptForSession);
        let v = serde_json::to_string(&d).unwrap();
        assert_eq!(v, "\"acceptForSession\"");
    }

    #[test]
    fn approval_decision_amended_serializes_as_object() {
        let d = ApprovalDecision::WithAmendment(AmendedDecision::AcceptWithExecpolicyAmendment {
            execpolicy_amendment: vec!["bash".into(), "-lc".into(), "ls".into()],
        });
        let v = serde_json::to_value(&d).unwrap();
        assert!(v["acceptWithExecpolicyAmendment"].is_object());
    }

    #[test]
    fn sandbox_policy_read_only_serializes_tagged() {
        let p = SandboxPolicy::ReadOnly {
            network_access: false,
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["type"], "readOnly");
        assert_eq!(v["networkAccess"], false);
    }

    #[test]
    fn thread_start_uses_v2_sandbox_and_runtime_roots() {
        let p = ThreadStartParams {
            sandbox: Some(SandboxMode::WorkspaceWrite),
            permissions: Some(":workspace".into()),
            runtime_workspace_roots: Some(vec!["/tmp/inbox".into()]),
            approvals_reviewer: Some(ApprovalsReviewer::AutoReview),
            ..ThreadStartParams::default()
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["sandbox"], "workspace-write");
        assert_eq!(v["permissions"], ":workspace");
        assert_eq!(v["runtimeWorkspaceRoots"][0], "/tmp/inbox");
        assert_eq!(v["approvalsReviewer"], "auto_review");
        assert!(v.get("sandboxPolicy").is_none());
        assert!(v.get("addDirs").is_none());
    }

    #[test]
    fn thread_resume_uses_v2_sandbox_and_runtime_roots() {
        let p = ThreadResumeParams {
            thread_id: "thread-1".into(),
            model: None,
            cwd: None,
            approval_policy: None,
            approvals_reviewer: None,
            sandbox: Some(SandboxMode::ReadOnly),
            permissions: Some(":workspace".into()),
            runtime_workspace_roots: Some(vec!["/tmp/inbox".into()]),
            service_tier: None,
            config: HashMap::new(),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["sandbox"], "read-only");
        assert_eq!(v["permissions"], ":workspace");
        assert_eq!(v["runtimeWorkspaceRoots"][0], "/tmp/inbox");
        assert!(v.get("sandboxPolicy").is_none());
        assert!(v.get("addDirs").is_none());
    }

    #[test]
    fn turn_input_text_serializes() {
        let item = TurnInputItem::Text {
            text: "hello".into(),
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "hello");
    }

    #[test]
    fn thread_start_serializes_explicit_null_service_tier() {
        let p = ThreadStartParams {
            service_tier: Some(None),
            ..ThreadStartParams::default()
        };
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("serviceTier").unwrap().is_null());
    }

    #[test]
    fn turn_start_serializes_nested_service_tier() {
        let p = TurnStartParams {
            thread_id: "t".into(),
            input: Vec::new(),
            approval_policy: None,
            approvals_reviewer: None,
            sandbox_policy: None,
            permissions: None,
            runtime_workspace_roots: None,
            model: None,
            effort: None,
            service_tier: Some(Some("fast".into())),
            collaboration_mode: None,
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["serviceTier"], "fast");
    }

    #[test]
    fn thread_compact_start_serializes_thread_id() {
        let p = ThreadCompactStartParams {
            thread_id: "thread-1".into(),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["threadId"], "thread-1");
    }

    #[test]
    fn collaboration_mode_plan_shape() {
        let m = CollaborationMode {
            mode: ModeKind::Plan,
            settings: CollaborationSettings {
                model: "gpt-5".into(),
                reasoning_effort: Some("medium".into()),
                developer_instructions: None,
            },
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["mode"], "plan");
        assert_eq!(v["settings"]["model"], "gpt-5");
        assert_eq!(v["settings"]["reasoningEffort"], "medium");
        assert!(v["settings"].get("developerInstructions").is_none());
    }

    #[test]
    fn token_usage_payload_parses() {
        let raw = r#"{
            "total": {"totalTokens":100,"inputTokens":80,"cachedInputTokens":10,"outputTokens":20,"reasoningOutputTokens":0},
            "last": {"totalTokens":50,"inputTokens":40,"cachedInputTokens":5,"outputTokens":10,"reasoningOutputTokens":0},
            "modelContextWindow": 200000
        }"#;
        let p: TokenUsagePayload = serde_json::from_str(raw).unwrap();
        assert_eq!(p.total.total_tokens, 100);
        assert_eq!(p.last.total_tokens, 50);
        assert_eq!(p.model_context_window, Some(200_000));
    }

    #[test]
    fn item_payload_parses_command_execution() {
        let raw = r#"{"id":"call_X","type":"commandExecution","command":"/bin/zsh -lc pwd","cwd":"/tmp","status":"inProgress","aggregatedOutput":null,"exitCode":null}"#;
        let p: ItemPayload = serde_json::from_str(raw).unwrap();
        assert_eq!(p.item_type, "commandExecution");
        assert_eq!(p.command.as_deref(), Some("/bin/zsh -lc pwd"));
        assert_eq!(p.status.as_deref(), Some("inProgress"));
    }

    #[test]
    fn item_payload_parses_agent_message() {
        let raw = r#"{"id":"msg","type":"agentMessage","text":"hello","phase":"commentary","memoryCitation":null}"#;
        let p: ItemPayload = serde_json::from_str(raw).unwrap();
        assert_eq!(p.item_type, "agentMessage");
        assert_eq!(p.text.as_deref(), Some("hello"));
        assert_eq!(p.phase.as_deref(), Some("commentary"));
    }

    #[test]
    fn command_approval_params_parse() {
        let raw = r#"{"threadId":"t","turnId":"tu","itemId":"i","reason":"root","command":"/bin/zsh -lc \"...\"","cwd":"/tmp","commandActions":[],"proposedExecpolicyAmendment":["bash","-lc","x"],"availableDecisions":["accept",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["bash"]}},"cancel"]}"#;
        let p: CommandApprovalParams = serde_json::from_str(raw).unwrap();
        assert_eq!(p.thread_id, "t");
        assert_eq!(p.command.unwrap(), "/bin/zsh -lc \"...\"");
        assert_eq!(p.available_decisions.len(), 3);
    }
}
