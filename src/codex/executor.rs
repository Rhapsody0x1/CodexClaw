use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use tokio::sync::{mpsc, oneshot};

use crate::{
    codex::{
        app_server::{
            AppServerHandle, TurnPolicy,
            protocol::{ApprovalPolicy, ApprovalsReviewer},
        },
        events::{CodexItem, PatchChangeKind, TokenUsageInfo, WebSearchAction},
    },
    session::state::{
        ApprovalPolicySetting, ContextMode, ReasoningEffort, ServiceTier, SessionState,
    },
    util::{
        layout::DataLayout,
        path::search_path_dirs,
        text::{humanize_tool_label, short_json, truncate_with_marker},
    },
};

#[derive(Clone)]
pub struct CodexExecutor {
    pub binary: PathBuf,
    pub sqlite_home: PathBuf,
    handle: Arc<AppServerHandle>,
}

#[derive(Debug, Clone)]
pub struct ExecutionRequest {
    pub prompt: String,
    pub workspace_dir: PathBuf,
    pub codex_home: PathBuf,
    pub config_overrides: Vec<String>,
    pub add_dirs: Vec<PathBuf>,
    pub session_state: SessionState,
    pub model: Option<String>,
    pub service_tier: Option<ServiceTier>,
    pub context_mode: Option<ContextMode>,
    pub reasoning_effort: ReasoningEffort,
    pub image_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct CompactRequest {
    pub session_id: String,
    pub workspace_dir: PathBuf,
    pub config_overrides: Vec<String>,
    pub add_dirs: Vec<PathBuf>,
    pub model: Option<String>,
    pub service_tier: Option<ServiceTier>,
    pub context_mode: Option<ContextMode>,
    pub reasoning_effort: ReasoningEffort,
}

#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub session_id: Option<String>,
    pub text: String,
    pub changed_files: Vec<PathBuf>,
    pub token_usage_info: Option<TokenUsageInfo>,
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionUpdate {
    /// Emitted once, as soon as the turn's thread is established, so the caller
    /// learns the thread id even if the turn is later interrupted or fails
    /// before producing an ExecutionResult.
    SessionStarted {
        session_id: String,
    },
    AgentMessage {
        text: String,
    },
    ToolCall {
        display: String,
    },
}

impl CodexExecutor {
    pub fn new(binary: String, data_dir: PathBuf, handle: Arc<AppServerHandle>) -> Self {
        Self {
            binary: PathBuf::from(binary),
            sqlite_home: DataLayout::new(data_dir).codex_sqlite_dir(),
            handle,
        }
    }

    pub fn handle(&self) -> Arc<AppServerHandle> {
        self.handle.clone()
    }

    /// Execute one turn against the shared app-server. Chooses a per-turn
    /// [`TurnPolicy`] from the current session settings (plan mode + approval
    /// policy override).
    pub async fn execute(
        &self,
        request: ExecutionRequest,
        cancel_rx: Option<oneshot::Receiver<()>>,
        update_tx: Option<mpsc::UnboundedSender<ExecutionUpdate>>,
    ) -> Result<ExecutionResult> {
        let policy = build_turn_policy(&request);
        self.handle
            .execute(request, policy, cancel_rx, update_tx)
            .await
    }

    pub async fn compact_session(
        &self,
        request: CompactRequest,
        cancel_rx: Option<oneshot::Receiver<()>>,
    ) -> Result<()> {
        self.handle.compact_thread(request, cancel_rx).await
    }
}

/// Given an [`ExecutionRequest`], pick the appropriate turn policy.
///
/// We avoid overriding anything we don't need to: approval_policy and
/// sandbox_policy default to `None` which means the app-server uses whatever
/// is in `~/.codex-claw/.codex/config.toml` (`sandbox_mode`, `approval_policy`,
/// and the `sandbox_workspace_write.*` knobs like `network_access`,
/// `exclude_slash_tmp`, `writable_roots`, etc.).
///
/// We only override when:
/// - plan mode is active → force `ReadOnly` + `Never` approvals + Plan collab;
/// - the user explicitly set an approval override via `/approvals`.
pub fn build_turn_policy(request: &ExecutionRequest) -> TurnPolicy {
    if request.session_state.settings.plan_mode {
        return TurnPolicy::plan_mode();
    }
    if let Some(setting) = request.session_state.settings.approval_policy_override {
        return match setting {
            ApprovalPolicySetting::GuardianSubagent => {
                TurnPolicy::with_approvals_reviewer(ApprovalsReviewer::GuardianSubagent)
            }
            _ => TurnPolicy::with_approval_policy(approval_setting_to_protocol(setting)),
        };
    }
    TurnPolicy::inherit_from_config()
}

fn approval_setting_to_protocol(setting: ApprovalPolicySetting) -> ApprovalPolicy {
    match setting {
        ApprovalPolicySetting::UnlessTrusted => ApprovalPolicy::UnlessTrusted,
        ApprovalPolicySetting::OnRequest => ApprovalPolicy::OnRequest,
        ApprovalPolicySetting::Never => ApprovalPolicy::Never,
        ApprovalPolicySetting::GuardianSubagent => ApprovalPolicy::OnRequest,
    }
}

/// Directories a codex turn should be able to find binaries in, on top of the
/// inherited `PATH`. Wider than the self-update search list on purpose: a turn
/// may shell out to anything the user has installed.
const CODEX_HOME_BIN_DIRS: &[&str] = &[".cargo/bin", ".local/bin"];
const CODEX_SYSTEM_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

pub fn build_codex_path_env(current: Option<&OsString>, home: Option<&Path>) -> Option<OsString> {
    let dirs = search_path_dirs(current, home, CODEX_HOME_BIN_DIRS, CODEX_SYSTEM_BIN_DIRS);
    if dirs.is_empty() {
        return None;
    }
    env::join_paths(dirs).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolEventPhase {
    Started,
    Updated,
    Completed,
}

/// Public mirror of `ToolEventPhase` for cross-module callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEventPhasePublic {
    Started,
    Updated,
    Completed,
}

impl From<ToolEventPhasePublic> for ToolEventPhase {
    fn from(v: ToolEventPhasePublic) -> Self {
        match v {
            ToolEventPhasePublic::Started => ToolEventPhase::Started,
            ToolEventPhasePublic::Updated => ToolEventPhase::Updated,
            ToolEventPhasePublic::Completed => ToolEventPhase::Completed,
        }
    }
}

/// Public wrapper around the internal formatter — used by
/// `src/codex/app_server/events.rs` for QQ parity.
pub fn tool_display_for_item_public(
    item: &CodexItem,
    phase: ToolEventPhasePublic,
) -> Option<String> {
    tool_display_for_item(item, phase.into())
}

pub fn format_todo_items_public(items: &[crate::codex::events::TodoEntry]) -> String {
    format_todo_items(items)
}

fn tool_display_for_item(item: &CodexItem, phase: ToolEventPhase) -> Option<String> {
    match item.item_type.as_str() {
        "command_execution" if phase == ToolEventPhase::Started => item
            .command
            .as_deref()
            .map(|command| format!("[Tool: Bash]\n```shell\n{}\n```", truncate(command, 180))),
        "web_search" if phase == ToolEventPhase::Completed => {
            Some(web_search_display_from_item(item))
        }
        "reasoning" if phase == ToolEventPhase::Completed => item
            .text
            .as_deref()
            .map(|text| format!("[Thinking]\n{}", truncate(text.trim(), 500))),
        "todo_list" if matches!(phase, ToolEventPhase::Started | ToolEventPhase::Updated) => {
            let detail = format_todo_items(&item.items);
            if detail.is_empty() {
                None
            } else {
                Some(format!("[Todo]\n{detail}"))
            }
        }
        "file_change" if phase == ToolEventPhase::Completed => {
            let detail = format_patch_changes(&item.changes);
            Some(if detail.is_empty() {
                "[Tool: Patch]".to_string()
            } else {
                format!("[Tool: Patch] {}", truncate(&detail, 220))
            })
        }
        "mcp_tool_call" if phase == ToolEventPhase::Started => {
            let server = item.server.as_deref().unwrap_or("unknown");
            let tool = item.tool.as_deref().unwrap_or("tool");
            let args = item
                .arguments
                .as_ref()
                .map(short_json)
                .filter(|value| !value.is_empty())
                .map(|value| format!(" {}", truncate(&value, 160)))
                .unwrap_or_default();
            Some(format!("[Tool: MCP {server}:{tool}]{args}"))
        }
        "mcp_tool_call" if phase == ToolEventPhase::Completed => {
            let server = item.server.as_deref().unwrap_or("unknown");
            let tool = item.tool.as_deref().unwrap_or("tool");
            let summary = if let Some(error) = item.error.as_ref() {
                format!(" failed: {}", truncate(error.message.trim(), 180))
            } else if let Some(result) = item.result.as_ref() {
                result
                    .structured_content
                    .as_ref()
                    .map(short_json)
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        result
                            .content
                            .first()
                            .and_then(|value| serde_json::to_string(value).ok())
                    })
                    .map(|value| format!(" {}", truncate(&value, 180)))
                    .unwrap_or_else(|| " completed".to_string())
            } else {
                " completed".to_string()
            };
            Some(format!("[Tool: MCP {server}:{tool}]{summary}"))
        }
        "collab_tool_call" if phase == ToolEventPhase::Started => {
            let label = humanize_tool_label(&item.tool.clone().unwrap_or_else(|| "collab".into()));
            let detail = item
                .receiver_thread_ids
                .first()
                .map(|thread_id| format!(" -> {thread_id}"))
                .or_else(|| {
                    item.prompt
                        .as_deref()
                        .filter(|prompt| !prompt.trim().is_empty())
                        .map(|prompt| format!(" {}", truncate(prompt.trim(), 120)))
                })
                .unwrap_or_default();
            Some(format!("[Tool: {label}]{detail}"))
        }
        "error" if phase == ToolEventPhase::Completed => item
            .message
            .as_deref()
            .map(|message| format!("[Error] {}", truncate(message.trim(), 220))),
        _ => None,
    }
}

fn web_search_action_detail(action: &WebSearchAction) -> String {
    match action {
        WebSearchAction::Search { query, queries } => query
            .clone()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                queries.as_ref().and_then(|values| {
                    if values.is_empty() {
                        None
                    } else if values.len() == 1 {
                        Some(values[0].clone())
                    } else {
                        Some(format!("{} ...", values[0]))
                    }
                })
            })
            .unwrap_or_default(),
        WebSearchAction::OpenPage { url } => url.clone().unwrap_or_default(),
        WebSearchAction::FindInPage { url, pattern } => match (pattern, url) {
            (Some(pattern), Some(url)) => format!("'{pattern}' in {url}"),
            (Some(pattern), None) => pattern.clone(),
            (None, Some(url)) => url.clone(),
            (None, None) => String::new(),
        },
        WebSearchAction::Other => String::new(),
    }
}

fn web_search_display_from_item(item: &CodexItem) -> String {
    let detail = item.query.clone().unwrap_or_default();
    match item.action.as_ref() {
        Some(WebSearchAction::Other) | None => web_search_display_from_detail(&detail),
        Some(action) => web_search_display_from_action(action),
    }
}

fn web_search_display_from_action(action: &WebSearchAction) -> String {
    let prefix = match action {
        WebSearchAction::Search { .. } => "[Tool: Web Search]",
        WebSearchAction::OpenPage { .. } => "[Tool: Web Open]",
        WebSearchAction::FindInPage { .. } => "[Tool: Web Find]",
        WebSearchAction::Other => "[Tool: Web Search]",
    };
    let detail = web_search_action_detail(action);
    if detail.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix} {}", truncate(&detail, 220))
    }
}

fn web_search_display_from_detail(detail: &str) -> String {
    let trimmed = detail.trim();
    let prefix = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        "[Tool: Web Open]"
    } else {
        "[Tool: Web Search]"
    };
    if trimmed.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix} {}", truncate(trimmed, 220))
    }
}

fn format_patch_changes(changes: &[crate::codex::events::FileUpdateChange]) -> String {
    changes
        .iter()
        .take(4)
        .map(|change| {
            let kind = match change.kind {
                PatchChangeKind::Add => "add",
                PatchChangeKind::Delete => "delete",
                PatchChangeKind::Update => "update",
            };
            format!("{} ({kind})", change.path)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_todo_items(items: &[crate::codex::events::TodoEntry]) -> String {
    items
        .iter()
        .take(6)
        .map(|item| {
            let mark = if item.completed { "x" } else { " " };
            format!("- [{}] {}", mark, item.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate(value: &str, max_chars: usize) -> String {
    truncate_with_marker(value, max_chars, "...")
}

#[cfg(test)]
mod tests {
    use std::{env, ffi::OsString};

    use tempfile::tempdir;

    use crate::codex::{
        events::{CodexItem, TodoEntry, WebSearchAction},
        executor::{
            ToolEventPhase, build_codex_path_env, format_todo_items, tool_display_for_item,
            web_search_action_detail, web_search_display_from_action,
            web_search_display_from_detail,
        },
    };

    /// A `CodexItem` with every optional field cleared; tests fill in only the
    /// ones the formatter under test reads.
    fn empty_item(item_type: &str) -> CodexItem {
        CodexItem {
            id: None,
            item_type: item_type.to_string(),
            text: None,
            message: None,
            command: None,
            query: None,
            action: None,
            changes: Vec::new(),
            server: None,
            tool: None,
            arguments: None,
            result: None,
            error: None,
            prompt: None,
            sender_thread_id: None,
            receiver_thread_ids: Vec::new(),
            items: Vec::new(),
            aggregated_output: None,
            exit_code: None,
            status: None,
        }
    }

    #[test]
    fn bash_tool_display_is_none_without_a_command() {
        // The `command: Some(..)` branch is covered end-to-end by
        // app_server::events::tests::command_execution_started_matches_legacy_bash_display.
        let item = empty_item("command_execution");
        assert!(tool_display_for_item(&item, ToolEventPhase::Started).is_none());
    }

    #[test]
    fn formats_web_search_response_item_display() {
        let action = WebSearchAction::Search {
            query: Some("openai codex github".to_string()),
            queries: Some(vec!["openai codex github".to_string()]),
        };
        assert_eq!(web_search_action_detail(&action), "openai codex github");
        assert_eq!(
            web_search_display_from_action(&action),
            "[Tool: Web Search] openai codex github"
        );
    }

    #[test]
    fn formats_web_find_response_item_display() {
        let action = WebSearchAction::FindInPage {
            url: Some("https://example.com".to_string()),
            pattern: Some("Codex".to_string()),
        };
        assert_eq!(
            web_search_display_from_action(&action),
            "[Tool: Web Find] 'Codex' in https://example.com"
        );
    }

    #[test]
    fn infers_web_open_from_url_query() {
        assert_eq!(
            web_search_display_from_detail("https://rhapsody0x1.github.io/"),
            "[Tool: Web Open] https://rhapsody0x1.github.io/"
        );
    }

    #[test]
    fn formats_todo_items_block() {
        let detail = format_todo_items(&[
            TodoEntry {
                text: "first".to_string(),
                completed: true,
            },
            TodoEntry {
                text: "second".to_string(),
                completed: false,
            },
        ]);
        assert_eq!(detail, "- [x] first\n- [ ] second");
    }

    #[test]
    fn path_env_includes_home_bin_fallbacks() {
        let home = tempdir().unwrap();
        let joined =
            build_codex_path_env(Some(&OsString::from("/usr/bin")), Some(home.path())).unwrap();
        let paths = env::split_paths(&joined).collect::<Vec<_>>();
        assert!(paths.contains(&home.path().join(".cargo").join("bin")));
        assert!(paths.contains(&home.path().join(".local").join("bin")));
    }
}
