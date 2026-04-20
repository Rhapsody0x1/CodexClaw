use serde::Deserialize;
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum CodexEvent {
    #[serde(rename = "thread.started")]
    ThreadStarted { thread_id: String },
    #[serde(rename = "turn.started")]
    TurnStarted,
    #[serde(rename = "item.started")]
    ItemStarted { item: CodexItem },
    #[serde(rename = "item.updated")]
    ItemUpdated { item: CodexItem },
    #[serde(rename = "item.completed")]
    ItemCompleted { item: CodexItem },
    #[serde(rename = "turn.completed")]
    TurnCompleted {
        #[serde(default)]
        usage: Option<TokenUsage>,
    },
    #[serde(rename = "turn.failed")]
    TurnFailed { error: serde_json::Value },
    #[serde(rename = "response_item")]
    ResponseItem { payload: ResponseItemPayload },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseItemPayload {
    WebSearchCall {
        #[serde(default)]
        status: Option<String>,
        action: WebSearchAction,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CodexItem {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub item_type: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub action: Option<WebSearchAction>,
    #[serde(default)]
    pub changes: Vec<FileUpdateChange>,
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub arguments: Option<JsonValue>,
    #[serde(default)]
    pub result: Option<McpToolCallResult>,
    #[serde(default)]
    pub error: Option<McpToolCallError>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub sender_thread_id: Option<String>,
    #[serde(default)]
    pub receiver_thread_ids: Vec<String>,
    #[serde(default)]
    pub items: Vec<TodoEntry>,
    #[serde(default)]
    pub aggregated_output: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebSearchAction {
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
pub struct FileUpdateChange {
    pub path: String,
    pub kind: PatchChangeKind,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchChangeKind {
    Add,
    Delete,
    Update,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpToolCallResult {
    #[serde(default)]
    pub content: Vec<JsonValue>,
    #[serde(default)]
    pub structured_content: Option<JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpToolCallError {
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TodoEntry {
    pub text: String,
    pub completed: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::{CodexEvent, ResponseItemPayload, WebSearchAction};

    #[test]
    fn parses_command_execution_started_event() {
        let event: CodexEvent = serde_json::from_str(
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc pwd","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#,
        )
        .unwrap();
        match event {
            CodexEvent::ItemStarted { item } => {
                assert_eq!(item.item_type, "command_execution");
                assert_eq!(item.command.as_deref(), Some("/bin/zsh -lc pwd"));
                assert_eq!(item.status.as_deref(), Some("in_progress"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parses_response_item_web_search_call() {
        let event: CodexEvent = serde_json::from_str(
            r#"{"type":"response_item","payload":{"type":"web_search_call","status":"completed","action":{"type":"search","query":"openai codex github","queries":["openai codex github"]}}}"#,
        )
        .unwrap();
        match event {
            CodexEvent::ResponseItem { payload } => match payload {
                ResponseItemPayload::WebSearchCall { status, action } => {
                    assert_eq!(status.as_deref(), Some("completed"));
                    match action {
                        WebSearchAction::Search { query, .. } => {
                            assert_eq!(query.as_deref(), Some("openai codex github"));
                        }
                        other => panic!("unexpected action: {other:?}"),
                    }
                }
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
