//! Human-readable one-liners for the codex items streamed during a turn.
//!
//! These formatters define the QQ-facing wire text, so `app_server::events`
//! translates every app-server notification into a [`DisplayItem`] and dispatches
//! here rather than growing a second set of strings.
//!
//! This module is a leaf inside `codex/`: it depends on `codex::events` and
//! `util::text` only, never on the executor or the app-server.

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::util::text::{humanize_tool_label, short_json, truncate_with_marker};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DisplayItem {
    #[serde(rename = "type")]
    pub(crate) item_type: String,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) message: Option<String>,
    #[serde(default)]
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) query: Option<String>,
    #[serde(default)]
    pub(crate) action: Option<WebSearchAction>,
    #[serde(default)]
    pub(crate) changes: Vec<FileUpdateChange>,
    #[serde(default)]
    pub(crate) server: Option<String>,
    #[serde(default)]
    pub(crate) tool: Option<String>,
    #[serde(default)]
    pub(crate) arguments: Option<JsonValue>,
    #[serde(default)]
    pub(crate) result: Option<McpToolCallResult>,
    #[serde(default)]
    pub(crate) error: Option<McpToolCallError>,
    #[serde(default)]
    pub(crate) prompt: Option<String>,
    #[serde(default)]
    pub(crate) receiver_thread_ids: Vec<String>,
    #[serde(default)]
    pub(crate) items: Vec<TodoEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WebSearchAction {
    Search {
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        queries: Option<Vec<String>>,
    },
    OpenPage {
        #[serde(default)]
        url: Option<String>,
    },
    FindInPage {
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        pattern: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FileUpdateChange {
    pub(crate) path: String,
    pub(crate) kind: PatchChangeKind,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PatchChangeKind {
    Add,
    Delete,
    Update,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct McpToolCallResult {
    #[serde(default)]
    pub(crate) content: Vec<JsonValue>,
    #[serde(default)]
    pub(crate) structured_content: Option<JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct McpToolCallError {
    pub(crate) message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TodoEntry {
    pub(crate) text: String,
    pub(crate) completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolEventPhase {
    Started,
    Updated,
    Completed,
}

pub(crate) fn tool_display_for_item(item: &DisplayItem, phase: ToolEventPhase) -> Option<String> {
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

fn web_search_display_from_item(item: &DisplayItem) -> String {
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

fn format_patch_changes(changes: &[FileUpdateChange]) -> String {
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

pub(crate) fn format_todo_items(items: &[TodoEntry]) -> String {
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
    use crate::codex::display::{
        DisplayItem, TodoEntry, ToolEventPhase, WebSearchAction, format_todo_items,
        tool_display_for_item, web_search_action_detail, web_search_display_from_action,
        web_search_display_from_detail,
    };

    /// A `DisplayItem` with every optional field cleared; tests fill in only the
    /// ones the formatter under test reads.
    fn empty_item(item_type: &str) -> DisplayItem {
        DisplayItem {
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
            receiver_thread_ids: Vec::new(),
            items: Vec::new(),
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
}
