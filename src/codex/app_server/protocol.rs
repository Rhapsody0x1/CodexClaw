//! Hand-copied subset of the `codex app-server` JSON-RPC protocol.
//!
//! Sources (reference):
//! - `/tmp/openai-codex/codex-rs/app-server-protocol/src/protocol/{common,v1,v2}.rs`
//! - `/tmp/openai-codex/codex-rs/protocol/src/{config_types,plan_tool}.rs`
//!
//! Field names and enum tagging match the wire format emitted by the installed
//! `codex app-server` binary (verified empirically with `/tmp/codex-probe`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

// ---------------------------------------------------------------------------
// JSON-RPC envelope
// ---------------------------------------------------------------------------

/// The app-server omits the `jsonrpc` field from responses/notifications and
/// accepts messages with or without it, so we serialize it as optional.
#[derive(Debug, Clone)]
pub enum Message {
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
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
}

// ---------------------------------------------------------------------------
// initialize
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_info: ClientInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<InitializeCapabilities>,
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
pub struct InitializeCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental_api: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub codex_home: Option<String>,
    #[serde(default)]
    pub platform_family: Option<String>,
    #[serde(default)]
    pub platform_os: Option<String>,
}

// ---------------------------------------------------------------------------
// thread / turn
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_policy: Option<SandboxPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub config: HashMap<String, JsonValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_dirs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartResponse {
    pub thread: Thread,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub approval_policy: Option<ApprovalPolicy>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_policy: Option<SandboxPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub config: HashMap<String, JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeResponse {
    pub thread: Thread,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUnsubscribeParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUnsubscribeResponse {
    pub status: ThreadUnsubscribeStatus,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThreadUnsubscribeStatus {
    NotLoaded,
    NotSubscribed,
    Unsubscribed,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub ephemeral: bool,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    pub thread_id: String,
    pub input: Vec<TurnInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_policy: Option<SandboxPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<CollaborationMode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TurnInputItem {
    Text { text: String },
    LocalImage { path: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartResponse {
    pub turn: Turn,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub error: Option<JsonValue>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TurnInterruptResponse {}

// ---------------------------------------------------------------------------
// Approval & sandbox
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalPolicy {
    UnlessTrusted,
    OnFailure,
    OnRequest,
    Never,
}

impl ApprovalPolicy {
    pub fn as_wire_str(self) -> &'static str {
        match self {
            ApprovalPolicy::UnlessTrusted => "unless-trusted",
            ApprovalPolicy::OnFailure => "on-failure",
            ApprovalPolicy::OnRequest => "on-request",
            ApprovalPolicy::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SandboxPolicy {
    ReadOnly {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access: Option<ReadOnlyAccess>,
    },
    WorkspaceWrite {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        writable_roots: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        read_only_access: Option<ReadOnlyAccess>,
        #[serde(default)]
        network_access: bool,
        #[serde(default)]
        exclude_tmpdir_env: bool,
        #[serde(default)]
        exclude_slash_tmp: bool,
    },
    DangerFullAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ReadOnlyAccess {
    FullAccess,
    Restricted {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        readable_roots: Vec<String>,
    },
}

// ---------------------------------------------------------------------------
// Collaboration mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationMode {
    pub mode: ModeKind,
    pub settings: CollaborationSettings,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModeKind {
    Default,
    Plan,
    #[allow(dead_code)]
    Execute,
    #[allow(dead_code)]
    PairProgramming,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationSettings {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
}

// ---------------------------------------------------------------------------
// Server notifications
// ---------------------------------------------------------------------------

/// Notification method names we care about (compared as `&str` at dispatch
/// time so unknown methods don't crash the client).
pub mod method {
    pub const THREAD_COMPACT_START: &str = "thread/compact/start";
    pub const THREAD_STARTED: &str = "thread/started";
    pub const THREAD_STATUS_CHANGED: &str = "thread/status/changed";
    pub const THREAD_TOKEN_USAGE_UPDATED: &str = "thread/tokenUsage/updated";
    pub const THREAD_COMPACTED: &str = "thread/compacted";
    pub const THREAD_CLOSED: &str = "thread/closed";
    pub const TURN_STARTED: &str = "turn/started";
    pub const TURN_COMPLETED: &str = "turn/completed";
    pub const TURN_FAILED: &str = "turn/failed";
    pub const TURN_PLAN_UPDATED: &str = "turn/planUpdated";
    pub const ITEM_STARTED: &str = "item/started";
    pub const ITEM_UPDATED: &str = "item/updated";
    pub const ITEM_COMPLETED: &str = "item/completed";
    pub const ITEM_AGENT_MESSAGE_DELTA: &str = "item/agentMessage/delta";
    pub const ITEM_REASONING_TEXT_DELTA: &str = "item/reasoning/textDelta";
    pub const ITEM_REASONING_SUMMARY_TEXT_DELTA: &str = "item/reasoning/summaryTextDelta";
    pub const ITEM_COMMAND_EXECUTION_OUTPUT_DELTA: &str = "item/commandExecution/outputDelta";
    pub const ACCOUNT_UPDATED: &str = "account/updated";
    pub const ACCOUNT_RATE_LIMITS_UPDATED: &str = "account/rateLimits/updated";
    pub const CONFIG_WARNING: &str = "configWarning";
    pub const MODEL_REROUTED: &str = "model/rerouted";
    pub const ERROR: &str = "error";
    pub const INITIALIZED: &str = "initialized";

    // Server-initiated request methods (require a response).
    pub const COMMAND_EXECUTION_REQUEST_APPROVAL: &str = "item/commandExecution/requestApproval";
    pub const FILE_CHANGE_REQUEST_APPROVAL: &str = "item/fileChange/requestApproval";
    pub const PERMISSIONS_REQUEST_APPROVAL: &str = "item/permissions/requestApproval";
    pub const APPLY_PATCH_APPROVAL: &str = "applyPatchApproval";
    pub const EXEC_COMMAND_APPROVAL: &str = "execCommandApproval";
    pub const MCP_SERVER_ELICITATION_REQUEST: &str = "mcpServer/elicitation/request";
    pub const CHATGPT_AUTH_TOKENS_REFRESH: &str = "account/chatgptAuthTokens/refresh";
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedNotification {
    pub thread_id: String,
    pub turn: Turn,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletedNotification {
    pub thread_id: String,
    pub turn: TurnCompleted,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompleted {
    pub id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub error: Option<TurnError>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnError {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemNotification {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub item: ItemPayload,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ItemPayload {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub item_type: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub summary: Option<JsonValue>,
    #[serde(default)]
    pub content: Option<JsonValue>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub aggregated_output: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub action: Option<JsonValue>,
    #[serde(default)]
    pub changes: Vec<FileChange>,
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub arguments: Option<JsonValue>,
    #[serde(default)]
    pub result: Option<JsonValue>,
    #[serde(default)]
    pub error: Option<JsonValue>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub sender_thread_id: Option<String>,
    #[serde(default)]
    pub receiver_thread_ids: Vec<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileChange {
    #[serde(default)]
    pub path: String,
    pub kind: PatchChangeKindWire,
    #[serde(default)]
    pub diff: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PatchChangeKindWire {
    Legacy(String),
    Structured(PatchChangeKind),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PatchChangeKind {
    Add,
    Delete,
    Update {
        #[serde(default)]
        move_path: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    #[serde(default)]
    pub delta: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningDeltaNotification {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub delta: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutputDeltaNotification {
    pub thread_id: String,
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub chunk: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageUpdatedNotification {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub token_usage: TokenUsagePayload,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsagePayload {
    pub total: TokenCountBucket,
    pub last: TokenCountBucket,
    #[serde(default)]
    pub model_context_window: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TokenCountBucket {
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_output_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlanUpdatedNotification {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub plan: Vec<TurnPlanStep>,
    #[serde(default)]
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlanStep {
    #[serde(default)]
    pub step: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorNotification {
    #[serde(default)]
    pub error: JsonValue,
    #[serde(default)]
    pub will_retry: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReroutedNotification {
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub from_model: Option<String>,
    #[serde(default)]
    pub to_model: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactedNotification {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCompactStartParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ThreadCompactStartResponse {}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigWarningNotification {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub detail: Option<JsonValue>,
}

// ---------------------------------------------------------------------------
// Server-initiated requests (approvals & elicitations)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandApprovalParams {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub command_actions: Vec<JsonValue>,
    #[serde(default)]
    pub proposed_execpolicy_amendment: Option<JsonValue>,
    #[serde(default)]
    pub available_decisions: Vec<JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeApprovalParams {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub grant_root: Option<String>,
    #[serde(default)]
    pub file_changes: JsonValue,
    #[serde(default)]
    pub available_decisions: Vec<JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsApprovalParams {
    pub thread_id: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub permissions: JsonValue,
    #[serde(default)]
    pub available_decisions: Vec<JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpElicitationParams {
    pub thread_id: String,
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub request: JsonValue,
}

/// The decision variants the server accepts. Derived from the installed
/// app-server binary's serde error: "expected one of accept, acceptForSession,
/// acceptWithExecpolicyAmendment, applyNetworkPolicyAmendment, decline, cancel".
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ApprovalDecision {
    Simple(SimpleDecision),
    WithAmendment(AmendedDecision),
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SimpleDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum AmendedDecision {
    #[serde(rename = "acceptWithExecpolicyAmendment")]
    AcceptWithExecpolicyAmendment { execpolicy_amendment: Vec<String> },
    #[serde(rename = "applyNetworkPolicyAmendment")]
    ApplyNetworkPolicyAmendment {
        #[serde(default)]
        detail: Option<JsonValue>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandApprovalResponse {
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileChangeApprovalResponse {
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionsApprovalResponse {
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum ElicitationResponse {
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
        assert_eq!(
            ApprovalPolicy::UnlessTrusted.as_wire_str(),
            "unless-trusted"
        );
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
    fn sandbox_policy_read_only_roundtrips() {
        let p = SandboxPolicy::ReadOnly {
            access: Some(ReadOnlyAccess::FullAccess),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["type"], "readOnly");
        assert_eq!(v["access"]["type"], "fullAccess");
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
            sandbox_policy: None,
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
