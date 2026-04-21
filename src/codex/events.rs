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
    #[serde(rename = "event_msg")]
    EventMsg { payload: EventMsgPayload },
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
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventMsgPayload {
    TokenCount {
        #[serde(default)]
        info: Option<TokenUsageInfo>,
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

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    pub fn tokens_in_context_window(&self) -> u64 {
        if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.total()
        }
    }

    pub fn percent_of_context_window_remaining(&self, context_window: u64) -> u64 {
        const BASELINE_TOKENS: u64 = 12_000;

        if context_window <= BASELINE_TOKENS {
            return 0;
        }

        let effective_window = context_window - BASELINE_TOKENS;
        let used = self
            .tokens_in_context_window()
            .saturating_sub(BASELINE_TOKENS);
        let remaining = effective_window.saturating_sub(used);
        ((remaining as f64 / effective_window as f64) * 100.0)
            .clamp(0.0, 100.0)
            .round() as u64
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TokenUsageInfo {
    #[serde(default)]
    pub total_token_usage: TokenUsage,
    #[serde(default)]
    pub last_token_usage: TokenUsage,
    #[serde(default)]
    pub model_context_window: Option<u64>,
}

impl TokenUsageInfo {
    pub fn context_window_usage(&self) -> &TokenUsage {
        if self.last_token_usage.tokens_in_context_window() > 0
            || self.total_token_usage.tokens_in_context_window() == 0
        {
            &self.last_token_usage
        } else {
            &self.total_token_usage
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CodexEvent, EventMsgPayload, ResponseItemPayload, WebSearchAction};

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

    #[test]
    fn parses_token_count_event_msg() {
        let event: CodexEvent = serde_json::from_str(
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":180,"cached_input_tokens":40,"output_tokens":50,"reasoning_output_tokens":15,"total_tokens":230},"last_token_usage":{"input_tokens":90,"cached_input_tokens":30,"output_tokens":40,"reasoning_output_tokens":12,"total_tokens":130},"model_context_window":200000}}}"#,
        )
        .unwrap();
        match event {
            CodexEvent::EventMsg { payload } => match payload {
                EventMsgPayload::TokenCount { info } => {
                    let info = info.expect("token usage info");
                    assert_eq!(info.total_token_usage.total_tokens, 230);
                    assert_eq!(info.last_token_usage.total_tokens, 130);
                    assert_eq!(info.model_context_window, Some(200_000));
                }
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn computes_context_remaining_like_codex_tui() {
        let usage = super::TokenUsage {
            total_tokens: 13_700,
            ..Default::default()
        };
        assert_eq!(usage.tokens_in_context_window(), 13_700);
        assert_eq!(usage.percent_of_context_window_remaining(272_000), 99);
    }

    #[test]
    fn prefers_last_usage_for_context_window_tracking() {
        let info = super::TokenUsageInfo {
            total_token_usage: super::TokenUsage {
                total_tokens: 1_234_567,
                ..Default::default()
            },
            last_token_usage: super::TokenUsage {
                total_tokens: 98_765,
                ..Default::default()
            },
            model_context_window: Some(272_000),
        };

        assert_eq!(info.context_window_usage().total_tokens, 98_765);
    }
}
