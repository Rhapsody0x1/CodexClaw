//! Translate app-server `ServerNotification` payloads into the existing
//! [`ExecutionUpdate`] stream consumed by `PassiveTurnEmitter`.
//!
//! The aim is **byte-for-byte parity** with the current `codex exec --json`
//! output: to achieve that, we convert each app-server `ItemPayload` into the
//! pre-existing [`CodexItem`] shape and dispatch to
//! [`crate::codex::executor::tool_display_for_item_public`] (the existing
//! formatter). Only events that don't fit the legacy shape get new output
//! paths (e.g. `[Model rerouted -> ...]`).

use serde_json::Value as JsonValue;
use tracing::trace;

use crate::codex::{
    events::{CodexItem, FileUpdateChange, PatchChangeKind, TodoEntry, WebSearchAction},
    executor::{
        ExecutionUpdate, ToolEventPhasePublic, format_todo_items_public,
        tool_display_for_item_public,
    },
};

use super::protocol::{
    AgentMessageDeltaNotification, CommandOutputDeltaNotification, ItemNotification, ItemPayload,
    ReasoningDeltaNotification, TokenUsagePayload, TurnCompleted, TurnPlanStep,
    TurnPlanUpdatedNotification,
};

/// State accumulated across a single turn so we can reconstruct aggregated
/// outputs and decide what to emit.
#[derive(Debug, Default)]
pub struct TurnState {
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub agent_text_parts: Vec<String>,
    pub changed_files: Vec<std::path::PathBuf>,
    pub token_usage: Option<TokenUsagePayload>,
    pub last_plan_signature: Option<String>,
}

/// What happened on a single translation step.
#[derive(Debug, Clone, Default)]
pub struct TranslationOutcome {
    pub updates: Vec<ExecutionUpdate>,
    /// `Some(true)` = turn completed successfully, `Some(false)` = failed,
    /// `None` = still in progress.
    pub turn_finished: Option<TurnOutcome>,
}

#[derive(Debug, Clone)]
pub enum TurnOutcome {
    Completed,
    Failed(String),
    Interrupted,
}

// ---------------------------------------------------------------------------
// Public translator entry points
// ---------------------------------------------------------------------------

pub fn translate_item_started(
    state: &mut TurnState,
    notif: &ItemNotification,
) -> Vec<ExecutionUpdate> {
    if state.thread_id.is_none() {
        state.thread_id = Some(notif.thread_id.clone());
    }
    if state.turn_id.is_none() {
        state.turn_id = notif.turn_id.clone();
    }
    let item = to_codex_item(&notif.item);
    trace!(item_type = %item.item_type, "item/started");
    match tool_display_for_item_public(&item, ToolEventPhasePublic::Started) {
        Some(display) => vec![ExecutionUpdate::ToolCall { display }],
        None => Vec::new(),
    }
}

pub fn translate_item_updated(
    _state: &mut TurnState,
    notif: &ItemNotification,
) -> Vec<ExecutionUpdate> {
    let item = to_codex_item(&notif.item);
    trace!(item_type = %item.item_type, "item/updated");
    match tool_display_for_item_public(&item, ToolEventPhasePublic::Updated) {
        Some(display) => vec![ExecutionUpdate::ToolCall { display }],
        None => Vec::new(),
    }
}

pub fn translate_item_completed(
    state: &mut TurnState,
    notif: &ItemNotification,
) -> Vec<ExecutionUpdate> {
    let item = to_codex_item(&notif.item);
    trace!(item_type = %item.item_type, "item/completed");
    let item_type = item.item_type.as_str();
    if item_type == "agent_message" {
        if let Some(text) = item.text.clone() {
            if !text.is_empty() {
                state.agent_text_parts.push(text.clone());
                return vec![ExecutionUpdate::AgentMessage { text }];
            }
        }
        return Vec::new();
    }
    if item_type == "file_change" {
        for change in &item.changes {
            state
                .changed_files
                .push(std::path::PathBuf::from(change.path.clone()));
        }
    }
    match tool_display_for_item_public(&item, ToolEventPhasePublic::Completed) {
        Some(display) => vec![ExecutionUpdate::ToolCall { display }],
        None => Vec::new(),
    }
}

pub fn translate_turn_plan_updated(
    state: &mut TurnState,
    notif: &TurnPlanUpdatedNotification,
) -> Vec<ExecutionUpdate> {
    let entries: Vec<TodoEntry> = notif.plan.iter().filter_map(step_to_entry).collect();
    if entries.is_empty() {
        return Vec::new();
    }
    let detail = format_todo_items_public(&entries);
    if detail.is_empty() {
        return Vec::new();
    }
    // Dedup so repeated identical plans don't flood QQ.
    let signature = format!("{}|{}", entries.len(), detail);
    if state.last_plan_signature.as_deref() == Some(signature.as_str()) {
        return Vec::new();
    }
    state.last_plan_signature = Some(signature);
    vec![ExecutionUpdate::ToolCall {
        display: format!("[Todo]\n{detail}"),
    }]
}

pub fn translate_token_usage(
    state: &mut TurnState,
    notif: &super::protocol::TokenUsageUpdatedNotification,
) {
    state.token_usage = Some(notif.token_usage.clone());
}

pub fn translate_turn_completed(_state: &mut TurnState, turn: &TurnCompleted) -> TurnOutcome {
    let status = turn.status.as_str();
    match status {
        "completed" => TurnOutcome::Completed,
        "interrupted" | "cancelled" => TurnOutcome::Interrupted,
        _ => {
            let message = turn
                .error
                .as_ref()
                .and_then(|e| e.message.clone())
                .unwrap_or_else(|| format!("turn {status}"));
            TurnOutcome::Failed(message)
        }
    }
}

pub fn translate_error_notification(error: &JsonValue, will_retry: bool) -> TurnOutcome {
    if will_retry {
        return TurnOutcome::Completed;
    }
    let message = error
        .get("message")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string());
    TurnOutcome::Failed(message)
}

pub fn translate_model_rerouted(
    notif: &super::protocol::ModelReroutedNotification,
) -> Option<ExecutionUpdate> {
    let to_model = notif.to_model.as_deref()?;
    Some(ExecutionUpdate::ToolCall {
        display: format!("[Model rerouted -> {to_model}]"),
    })
}

pub fn translate_compacted(
    _state: &mut TurnState,
    _notif: &super::protocol::CompactedNotification,
) -> ExecutionUpdate {
    ExecutionUpdate::ToolCall {
        display: "[Context Compacted]".to_string(),
    }
}

/// Translate a streaming assistant text delta. We do NOT emit a QQ message per
/// delta (that would flood the channel); deltas are accumulated until the
/// matching `item/completed agent_message` fires, which emits the full text.
/// We keep this function so callers can easily opt into delta handling later.
pub fn accumulate_agent_delta(
    _state: &mut TurnState,
    _notif: &AgentMessageDeltaNotification,
) -> Vec<ExecutionUpdate> {
    Vec::new()
}

pub fn accumulate_reasoning_delta(
    _state: &mut TurnState,
    _notif: &ReasoningDeltaNotification,
) -> Vec<ExecutionUpdate> {
    Vec::new()
}

pub fn accumulate_command_output(
    _state: &mut TurnState,
    _notif: &CommandOutputDeltaNotification,
) -> Vec<ExecutionUpdate> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// ItemPayload → CodexItem converter
// ---------------------------------------------------------------------------

fn to_codex_item(p: &ItemPayload) -> CodexItem {
    let item_type = map_item_type(&p.item_type);
    let (result, error) = parse_mcp_result_error(&p.result, &p.error);
    let action = parse_web_search_action(&p.action);
    let changes = p
        .changes
        .iter()
        .filter_map(map_file_change)
        .collect::<Vec<_>>();
    let items = parse_todo_items(&p.content);
    let text = derive_item_text(p);
    let message = p.message.clone().or_else(|| {
        if item_type == "error" {
            p.text.clone()
        } else {
            None
        }
    });

    CodexItem {
        id: p.id.clone(),
        item_type,
        text,
        message,
        command: p.command.clone(),
        query: p.query.clone(),
        action,
        changes,
        server: p.server.clone(),
        tool: p.tool.clone(),
        arguments: p.arguments.clone(),
        result,
        error,
        prompt: p.prompt.clone(),
        sender_thread_id: p.sender_thread_id.clone(),
        receiver_thread_ids: p.receiver_thread_ids.clone(),
        items,
        aggregated_output: p.aggregated_output.clone(),
        exit_code: p.exit_code,
        status: p.status.clone(),
    }
}

fn map_item_type(wire: &str) -> String {
    match wire {
        "commandExecution" => "command_execution".to_string(),
        "agentMessage" => "agent_message".to_string(),
        "userMessage" => "user_message".to_string(),
        "fileChange" => "file_change".to_string(),
        "webSearch" => "web_search".to_string(),
        "mcpToolCall" => "mcp_tool_call".to_string(),
        "collabAgentToolCall" => "collab_tool_call".to_string(),
        // Pass-through (already snake_case or matches legacy):
        other => other.to_string(),
    }
}

fn map_file_change(fc: &super::protocol::FileChange) -> Option<FileUpdateChange> {
    let kind = match &fc.kind {
        super::protocol::PatchChangeKindWire::Legacy(kind) => match kind.as_str() {
            "add" | "Add" => PatchChangeKind::Add,
            "delete" | "Delete" => PatchChangeKind::Delete,
            "update" | "Update" | "modify" | "Modify" | "edit" | "Edit" => PatchChangeKind::Update,
            _ => return None,
        },
        super::protocol::PatchChangeKindWire::Structured(kind) => match kind {
            super::protocol::PatchChangeKind::Add => PatchChangeKind::Add,
            super::protocol::PatchChangeKind::Delete => PatchChangeKind::Delete,
            super::protocol::PatchChangeKind::Update { .. } => PatchChangeKind::Update,
        },
    };
    if fc.path.is_empty() {
        return None;
    }
    Some(FileUpdateChange {
        path: fc.path.clone(),
        kind,
    })
}

fn parse_web_search_action(raw: &Option<JsonValue>) -> Option<WebSearchAction> {
    let value = raw.as_ref()?;
    let obj = value.as_object()?;
    let tag = obj.get("type")?.as_str()?;
    match tag {
        "search" => {
            let query = obj.get("query").and_then(|v| v.as_str()).map(str::to_owned);
            let queries = obj.get("queries").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect::<Vec<String>>()
            });
            Some(WebSearchAction::Search { query, queries })
        }
        "openPage" | "open_page" => {
            let url = obj.get("url").and_then(|v| v.as_str()).map(str::to_owned);
            Some(WebSearchAction::OpenPage { url })
        }
        "findInPage" | "find_in_page" => {
            let url = obj.get("url").and_then(|v| v.as_str()).map(str::to_owned);
            let pattern = obj
                .get("pattern")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            Some(WebSearchAction::FindInPage { url, pattern })
        }
        _ => Some(WebSearchAction::Other),
    }
}

fn parse_mcp_result_error(
    result: &Option<JsonValue>,
    error: &Option<JsonValue>,
) -> (
    Option<crate::codex::events::McpToolCallResult>,
    Option<crate::codex::events::McpToolCallError>,
) {
    let parsed_result = result.as_ref().and_then(|v| {
        serde_json::from_value::<crate::codex::events::McpToolCallResult>(v.clone()).ok()
    });
    let parsed_error = error.as_ref().and_then(|v| {
        // Error may be shaped as {"message":"..."} or {"error":{"message":"..."}}.
        if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
            Some(crate::codex::events::McpToolCallError {
                message: msg.to_string(),
            })
        } else {
            v.as_str()
                .map(|msg| crate::codex::events::McpToolCallError {
                    message: msg.to_string(),
                })
        }
    });
    (parsed_result, parsed_error)
}

fn parse_todo_items(content: &Option<JsonValue>) -> Vec<TodoEntry> {
    let arr = match content.as_ref().and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|v| {
            let obj = v.as_object()?;
            let text = obj.get("text").and_then(|v| v.as_str())?.to_string();
            let completed = obj
                .get("completed")
                .and_then(|v| v.as_bool())
                .or_else(|| {
                    obj.get("status").and_then(|s| s.as_str()).map(|s| {
                        matches!(
                            s,
                            "completed" | "done" | "finished" | "complete" | "Completed"
                        )
                    })
                })
                .unwrap_or(false);
            Some(TodoEntry { text, completed })
        })
        .collect()
}

fn step_to_entry(step: &TurnPlanStep) -> Option<TodoEntry> {
    let text = step.step.clone().unwrap_or_default();
    if text.is_empty() {
        return None;
    }
    let completed = matches!(
        step.status.as_deref(),
        Some("completed") | Some("done") | Some("Completed")
    );
    Some(TodoEntry { text, completed })
}

fn derive_item_text(p: &ItemPayload) -> Option<String> {
    if let Some(t) = p.text.clone() {
        if !t.is_empty() {
            return Some(t);
        }
    }
    // For reasoning items, text may be in `content` / `summary`.
    if p.item_type == "reasoning" {
        if let Some(content) = p.content.as_ref().and_then(|v| {
            if let Some(arr) = v.as_array() {
                Some(
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            } else {
                v.as_str().map(str::to_owned)
            }
        }) {
            if !content.is_empty() {
                return Some(content);
            }
        }
        if let Some(summary) = p.summary.as_ref().and_then(|v| {
            if let Some(arr) = v.as_array() {
                Some(
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            } else {
                v.as_str().map(str::to_owned)
            }
        }) {
            if !summary.is_empty() {
                return Some(summary);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_item(raw: serde_json::Value) -> ItemNotification {
        serde_json::from_value(json!({
            "threadId":"t",
            "turnId":"tu",
            "item": raw,
        }))
        .unwrap()
    }

    #[test]
    fn command_execution_started_matches_legacy_bash_display() {
        let item = make_item(json!({
            "id":"c","type":"commandExecution","command":"/bin/zsh -lc pwd","cwd":"/tmp","status":"inProgress"
        }));
        let mut state = TurnState::default();
        let updates = translate_item_started(&mut state, &item);
        assert_eq!(updates.len(), 1);
        match &updates[0] {
            ExecutionUpdate::ToolCall { display } => {
                assert_eq!(display, "[Tool: Bash]\n```shell\n/bin/zsh -lc pwd\n```");
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn agent_message_completed_emits_agent_message_update() {
        let item = make_item(json!({
            "id":"m","type":"agentMessage","text":"Hello world","phase":"final"
        }));
        let mut state = TurnState::default();
        let updates = translate_item_completed(&mut state, &item);
        assert_eq!(updates.len(), 1);
        match &updates[0] {
            ExecutionUpdate::AgentMessage { text } => assert_eq!(text, "Hello world"),
            _ => panic!("expected AgentMessage"),
        }
        assert_eq!(state.agent_text_parts, vec!["Hello world"]);
    }

    #[test]
    fn web_search_open_page_completed_matches_legacy() {
        let item = make_item(json!({
            "id":"w","type":"webSearch",
            "query":"https://example.com",
            "action":{"type":"openPage","url":"https://example.com"}
        }));
        let mut state = TurnState::default();
        let updates = translate_item_completed(&mut state, &item);
        match &updates[0] {
            ExecutionUpdate::ToolCall { display } => {
                assert_eq!(display, "[Tool: Web Open] https://example.com");
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn file_change_completed_with_structured_kind_lists_paths_and_records_state() {
        let item = make_item(json!({
            "id":"f","type":"fileChange",
            "status":"completed",
            "changes":[{
                "path":"src/main.rs",
                "kind":{"type":"update","movePath":null},
                "diff":"@@ -1 +1 @@"
            }]
        }));
        let mut state = TurnState::default();
        let updates = translate_item_completed(&mut state, &item);
        match &updates[0] {
            ExecutionUpdate::ToolCall { display } => {
                assert_eq!(display, "[Tool: Patch] src/main.rs (update)");
            }
            _ => panic!("expected ToolCall"),
        }
        assert_eq!(state.changed_files.len(), 1);
    }

    #[test]
    fn file_change_completed_accepts_legacy_string_kind() {
        let item = make_item(json!({
            "id":"f","type":"fileChange",
            "changes":[{"path":"src/lib.rs","kind":"update"}]
        }));
        let mut state = TurnState::default();
        let updates = translate_item_completed(&mut state, &item);
        match &updates[0] {
            ExecutionUpdate::ToolCall { display } => {
                assert_eq!(display, "[Tool: Patch] src/lib.rs (update)");
            }
            _ => panic!("expected ToolCall"),
        }
        assert_eq!(state.changed_files.len(), 1);
    }

    #[test]
    fn reasoning_completed_uses_existing_thinking_format() {
        let item = make_item(json!({
            "id":"r","type":"reasoning",
            "text":"先检查当前目录，再决定下一步。"
        }));
        let mut state = TurnState::default();
        let updates = translate_item_completed(&mut state, &item);
        match &updates[0] {
            ExecutionUpdate::ToolCall { display } => {
                assert_eq!(display, "[Thinking]\n先检查当前目录，再决定下一步。");
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn turn_plan_updated_emits_todo_block_once_per_unique_plan() {
        let notif = TurnPlanUpdatedNotification {
            thread_id: "t".into(),
            turn_id: Some("tu".into()),
            plan: vec![
                TurnPlanStep {
                    step: Some("first".into()),
                    status: Some("completed".into()),
                },
                TurnPlanStep {
                    step: Some("second".into()),
                    status: Some("in_progress".into()),
                },
            ],
            explanation: None,
        };
        let mut state = TurnState::default();
        let first = translate_turn_plan_updated(&mut state, &notif);
        assert_eq!(first.len(), 1);
        // Repeated identical plans are suppressed.
        let second = translate_turn_plan_updated(&mut state, &notif);
        assert_eq!(second.len(), 0);
    }
}
