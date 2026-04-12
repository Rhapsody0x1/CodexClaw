use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{mpsc, oneshot},
};

use crate::{
    codex::events::{CodexEvent, CodexItem, PatchChangeKind, ResponseItemPayload, WebSearchAction},
    session::state::{ContextMode, ReasoningEffort, ServiceTier, SessionState},
};

const OUTPUT_IDLE_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone)]
pub struct CodexExecutor {
    pub binary: PathBuf,
    pub sqlite_home: PathBuf,
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
pub struct ExecutionResult {
    pub session_id: Option<String>,
    pub text: String,
    pub changed_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionUpdate {
    AgentMessage { text: String },
    ToolCall { display: String },
}

impl CodexExecutor {
    pub fn new(binary: String, data_dir: PathBuf) -> Self {
        Self {
            binary: PathBuf::from(binary),
            sqlite_home: data_dir.join("codex-sqlite"),
        }
    }

    pub async fn execute(
        &self,
        request: ExecutionRequest,
        cancel_rx: Option<oneshot::Receiver<()>>,
        update_tx: Option<mpsc::UnboundedSender<ExecutionUpdate>>,
    ) -> Result<ExecutionResult> {
        let sqlite_home = request.codex_home.join("sqlite");
        if tokio::fs::create_dir_all(&sqlite_home).await.is_err() {
            tokio::fs::create_dir_all(&self.sqlite_home).await?;
        }
        let sqlite_home_env = if sqlite_home.exists() {
            sqlite_home.clone()
        } else {
            self.sqlite_home.clone()
        };
        let mut command = Command::new(&self.binary);
        command.arg("exec");
        if let Some(session_id) = request.session_state.session_id.clone() {
            command.arg("-C").arg(&request.workspace_dir);
            for add_dir in &request.add_dirs {
                command.arg("--add-dir").arg(add_dir);
            }
            command
                .arg("resume")
                .arg("--json")
                .arg("--full-auto")
                .arg("--skip-git-repo-check")
                .arg("-c")
                .arg(format!(
                    "model_reasoning_effort=\"{}\"",
                    request.reasoning_effort.as_str()
                ));
            for override_arg in &request.config_overrides {
                command.arg("-c").arg(override_arg);
            }
            if let Some(model) = &request.model {
                command.arg("--model").arg(model);
            }
            append_runtime_overrides(&mut command, request.service_tier, request.context_mode);
            for image in &request.image_paths {
                command.arg("--image").arg(image);
            }
            command.arg(session_id).arg("--").arg(&request.prompt);
        } else {
            command
                .arg("--json")
                .arg("--full-auto")
                .arg("--skip-git-repo-check")
                .arg("-C")
                .arg(&request.workspace_dir)
                .arg("-c")
                .arg(format!(
                    "model_reasoning_effort=\"{}\"",
                    request.reasoning_effort.as_str()
                ));
            for override_arg in &request.config_overrides {
                command.arg("-c").arg(override_arg);
            }
            if let Some(model) = &request.model {
                command.arg("--model").arg(model);
            }
            append_runtime_overrides(&mut command, request.service_tier, request.context_mode);
            for image in &request.image_paths {
                command.arg("--image").arg(image);
            }
            for add_dir in &request.add_dirs {
                command.arg("--add-dir").arg(add_dir);
            }
            command.arg("--").arg(&request.prompt);
        }
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("CODEX_HOME", &request.codex_home)
            .env("CODEX_SQLITE_HOME", sqlite_home_env)
            .current_dir(&request.workspace_dir);
        if let Some(path_env) = build_codex_path_env(
            env::var_os("PATH").as_ref(),
            env::var_os("HOME").as_deref().map(Path::new),
        ) {
            command.env("PATH", path_env);
        }

        let mut child = command.spawn().context("failed to spawn codex process")?;
        let stdout = child.stdout.take().context("missing codex stdout")?;
        let stderr = child.stderr.take().context("missing codex stderr")?;

        let stderr_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            let mut lines = Vec::new();
            while let Ok(Some(line)) = reader.next_line().await {
                lines.push(line);
            }
            lines
        });

        let mut reader = BufReader::new(stdout).lines();
        let mut current_session_id = request.session_state.session_id.clone();
        let mut text_parts = Vec::new();
        let mut changed_files = Vec::new();
        let mut cancel_rx = cancel_rx;
        loop {
            let line = if let Some(cancel) = cancel_rx.as_mut() {
                tokio::select! {
                    line = tokio::time::timeout(OUTPUT_IDLE_TIMEOUT, reader.next_line()) => {
                        match line {
                            Ok(result) => result?,
                            Err(_) => {
                                let _ = child.kill().await;
                                let _ = child.wait().await;
                                return Err(anyhow!(
                                    "codex 输出超时（{} 秒无新事件）",
                                    OUTPUT_IDLE_TIMEOUT.as_secs()
                                ));
                            }
                        }
                    }
                    _ = cancel => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        return Err(anyhow!("codex turn aborted by user"));
                    }
                }
            } else {
                match tokio::time::timeout(OUTPUT_IDLE_TIMEOUT, reader.next_line()).await {
                    Ok(result) => result?,
                    Err(_) => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        return Err(anyhow!(
                            "codex 输出超时（{} 秒无新事件）",
                            OUTPUT_IDLE_TIMEOUT.as_secs()
                        ));
                    }
                }
            };
            let Some(line) = line else {
                break;
            };
            if let Some(tx) = update_tx.as_ref()
                && let Some(update) = parse_update_from_raw_json(&line)
            {
                let _ = tx.send(update);
            }
            if let Ok(event) = serde_json::from_str::<CodexEvent>(&line) {
                match event {
                    CodexEvent::ThreadStarted { thread_id } => current_session_id = Some(thread_id),
                    CodexEvent::ItemStarted { item } => {
                        if let Some(display) = tool_display_for_item(&item, ToolEventPhase::Started)
                        {
                            if let Some(tx) = update_tx.as_ref() {
                                let _ = tx.send(ExecutionUpdate::ToolCall { display });
                            }
                        }
                    }
                    CodexEvent::ItemUpdated { item } => {
                        if let Some(display) = tool_display_for_item(&item, ToolEventPhase::Updated)
                        {
                            if let Some(tx) = update_tx.as_ref() {
                                let _ = tx.send(ExecutionUpdate::ToolCall { display });
                            }
                        }
                    }
                    CodexEvent::ItemCompleted { item } => {
                        if item.item_type == "file_change" {
                            for change in &item.changes {
                                changed_files.push(PathBuf::from(change.path.clone()));
                            }
                        }
                        if item.item_type == "agent_message" {
                            if let Some(text) = item.text {
                                if let Some(tx) = update_tx.as_ref() {
                                    let _ = tx
                                        .send(ExecutionUpdate::AgentMessage { text: text.clone() });
                                }
                                text_parts.push(text);
                            }
                            continue;
                        }
                        if let Some(display) =
                            tool_display_for_item(&item, ToolEventPhase::Completed)
                        {
                            if let Some(tx) = update_tx.as_ref() {
                                let _ = tx.send(ExecutionUpdate::ToolCall { display });
                            }
                        }
                    }
                    CodexEvent::ResponseItem {
                        payload: ResponseItemPayload::Unknown,
                    } => {}
                    CodexEvent::ResponseItem { .. } => {}
                    CodexEvent::TurnFailed { error } => {
                        return Err(anyhow!("codex turn failed: {}", error));
                    }
                    _ => {}
                }
            }
        }

        let status = child.wait().await?;
        let stderr_lines = stderr_handle.await.unwrap_or_default();
        anyhow::ensure!(
            status.success(),
            "codex exited with status {}{}",
            status,
            if stderr_lines.is_empty() {
                String::new()
            } else {
                format!(", stderr: {}", stderr_lines.join("\n"))
            }
        );
        Ok(ExecutionResult {
            session_id: current_session_id,
            text: text_parts.join("\n\n").trim().to_string(),
            changed_files,
        })
    }
}

fn build_codex_path_env(current: Option<&OsString>, home: Option<&Path>) -> Option<OsString> {
    let mut dirs = Vec::new();
    if let Some(current) = current {
        for dir in env::split_paths(current) {
            push_unique_dir(&mut dirs, dir);
        }
    }
    if let Some(home) = home {
        push_unique_dir(&mut dirs, home.join(".cargo").join("bin"));
        push_unique_dir(&mut dirs, home.join(".local").join("bin"));
    }
    for dir in [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/local/sbin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ] {
        push_unique_dir(&mut dirs, dir);
    }
    if dirs.is_empty() {
        return None;
    }
    env::join_paths(dirs).ok()
}

fn push_unique_dir(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dirs.iter().any(|existing| existing == &dir) {
        dirs.push(dir);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolEventPhase {
    Started,
    Updated,
    Completed,
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
            Some(format!("[Tool: MCP {}:{}]{}", server, tool, args))
        }
        "mcp_tool_call" if phase == ToolEventPhase::Completed => {
            let server = item.server.as_deref().unwrap_or("unknown");
            let tool = item.tool.as_deref().unwrap_or("tool");
            let summary = if let Some(error) = item.error.as_ref() {
                format!(" failed: {}", truncate(error.message.trim(), 180))
            } else if let Some(result) = item.result.as_ref() {
                let detail = result
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
                    .unwrap_or_else(|| " completed".to_string());
                detail
            } else {
                " completed".to_string()
            };
            Some(format!("[Tool: MCP {}:{}]{}", server, tool, summary))
        }
        "collab_tool_call" if phase == ToolEventPhase::Started => {
            let label = humanize_tool_label(&item.tool.clone().unwrap_or_else(|| "collab".into()));
            let detail = item
                .receiver_thread_ids
                .first()
                .map(|thread_id| format!(" -> {}", thread_id))
                .or_else(|| {
                    item.prompt
                        .as_deref()
                        .filter(|prompt| !prompt.trim().is_empty())
                        .map(|prompt| format!(" {}", truncate(prompt.trim(), 120)))
                })
                .unwrap_or_default();
            Some(format!("[Tool: {}]{}", label, detail))
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
            (Some(pattern), Some(url)) => format!("'{}' in {}", pattern, url),
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

fn short_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn humanize_tool_label(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut text = first.to_uppercase().collect::<String>();
                    text.push_str(chars.as_str());
                    text
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_update_from_raw_json(line: &str) -> Option<ExecutionUpdate> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    match value.get("type")?.as_str()? {
        "response_item" => {
            let payload = value.get("payload")?;
            if payload.get("type")?.as_str()? != "web_search_call" {
                return None;
            }
            if payload.get("status").and_then(|value| value.as_str()) != Some("completed") {
                return None;
            }
            let action =
                serde_json::from_value::<WebSearchAction>(payload.get("action")?.clone()).ok()?;
            Some(ExecutionUpdate::ToolCall {
                display: web_search_display_from_action(&action),
            })
        }
        "item.completed" => {
            let item = value.get("item")?;
            if item.get("type")?.as_str()? != "web_search" {
                return None;
            }
            let action =
                serde_json::from_value::<WebSearchAction>(item.get("action")?.clone()).ok()?;
            let query = item
                .get("query")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let display = match action {
                WebSearchAction::Other => web_search_display_from_detail(query),
                _ => web_search_display_from_action(&action),
            };
            Some(ExecutionUpdate::ToolCall { display })
        }
        _ => None,
    }
}

fn append_runtime_overrides(
    command: &mut Command,
    service_tier: Option<ServiceTier>,
    context_mode: Option<ContextMode>,
) {
    if let Some(service_tier) = service_tier {
        command
            .arg("-c")
            .arg(format!("service_tier=\"{}\"", service_tier.as_str()));
    }
    match context_mode {
        Some(ContextMode::OneM) => {
            command.arg("-c").arg("model_context_window=1000000");
            command
                .arg("-c")
                .arg("model_auto_compact_token_limit=900000");
        }
        Some(ContextMode::Standard) => {
            command.arg("-c").arg("model_context_window=272000");
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use std::{env, ffi::OsString};

    use tempfile::tempdir;

    use crate::codex::{
        events::{CodexItem, PatchChangeKind, TodoEntry, WebSearchAction},
        executor::{
            ExecutionUpdate, ToolEventPhase, build_codex_path_env, format_todo_items,
            humanize_tool_label, parse_update_from_raw_json, tool_display_for_item,
            web_search_action_detail, web_search_display_from_action,
            web_search_display_from_detail,
        },
    };

    #[test]
    fn formats_bash_tool_display() {
        let item = CodexItem {
            id: None,
            item_type: "command_execution".to_string(),
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
        };
        assert!(tool_display_for_item(&item, ToolEventPhase::Started).is_none());
        let item = CodexItem {
            command: Some("/bin/zsh -lc pwd".to_string()),
            ..item
        };
        assert_eq!(
            tool_display_for_item(&item, ToolEventPhase::Started).as_deref(),
            Some("[Tool: Bash]\n```shell\n/bin/zsh -lc pwd\n```")
        );
    }

    #[test]
    fn humanizes_unknown_tool_names() {
        assert_eq!(humanize_tool_label("file_search"), "File Search");
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
    fn formats_web_open_response_item_display() {
        let action = WebSearchAction::OpenPage {
            url: Some("https://rhapsody0x1.github.io/".to_string()),
        };
        assert_eq!(
            web_search_display_from_action(&action),
            "[Tool: Web Open] https://rhapsody0x1.github.io/"
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
    fn parses_raw_web_search_response_item() {
        let update = parse_update_from_raw_json(
            r#"{"type":"response_item","payload":{"type":"web_search_call","status":"completed","action":{"type":"search","query":"Anthropic Claude Code official repository GitHub","queries":["Anthropic Claude Code official repository GitHub"]}}}"#,
        );
        assert_eq!(
            update,
            Some(ExecutionUpdate::ToolCall {
                display: "[Tool: Web Search] Anthropic Claude Code official repository GitHub"
                    .to_string()
            })
        );
    }

    #[test]
    fn parses_raw_item_completed_web_search() {
        let update = parse_update_from_raw_json(
            r#"{"type":"item.completed","item":{"id":"item_1","type":"web_search","id":"ws_abc","query":"Claude Code official repository Anthropic GitHub","action":{"type":"search","query":"Claude Code official repository Anthropic GitHub","queries":["Claude Code official repository Anthropic GitHub"]}}}"#,
        );
        assert_eq!(
            update,
            Some(ExecutionUpdate::ToolCall {
                display: "[Tool: Web Search] Claude Code official repository Anthropic GitHub"
                    .to_string()
            })
        );
    }

    #[test]
    fn infers_web_open_from_url_query_when_action_is_other() {
        assert_eq!(
            web_search_display_from_detail("https://rhapsody0x1.github.io/"),
            "[Tool: Web Open] https://rhapsody0x1.github.io/"
        );
        let update = parse_update_from_raw_json(
            r#"{"type":"item.completed","item":{"id":"item_1","type":"web_search","id":"ws_abc","query":"https://rhapsody0x1.github.io/","action":{"type":"other"}}}"#,
        );
        assert_eq!(
            update,
            Some(ExecutionUpdate::ToolCall {
                display: "[Tool: Web Open] https://rhapsody0x1.github.io/".to_string()
            })
        );
    }

    #[test]
    fn formats_patch_changes() {
        let item = CodexItem {
            id: None,
            item_type: "file_change".to_string(),
            text: None,
            message: None,
            command: None,
            query: None,
            action: None,
            changes: vec![crate::codex::events::FileUpdateChange {
                path: "src/main.rs".to_string(),
                kind: PatchChangeKind::Update,
            }],
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
        };
        assert_eq!(
            tool_display_for_item(&item, ToolEventPhase::Completed).as_deref(),
            Some("[Tool: Patch] src/main.rs (update)")
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
    fn formats_reasoning_block() {
        let item = CodexItem {
            id: None,
            item_type: "reasoning".to_string(),
            text: Some("先检查当前目录，再决定下一步。".to_string()),
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
        };
        assert_eq!(
            tool_display_for_item(&item, ToolEventPhase::Completed).as_deref(),
            Some("[Thinking]\n先检查当前目录，再决定下一步。")
        );
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
