//! Plain data carried across the codex boundary: what a turn is asked to do,
//! what it produced, and what it streams while it runs.
//!
//! This module is a leaf — it depends only on value types, never on the
//! app-server transport or the display formatters.

use std::path::PathBuf;

use crate::model::settings::{ContextMode, ReasoningEffort, ServiceTier, SessionState};

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
pub(crate) struct CompactRequest {
    pub(crate) session_id: String,
    pub(crate) workspace_dir: PathBuf,
    pub(crate) config_overrides: Vec<String>,
    pub(crate) add_dirs: Vec<PathBuf>,
    pub(crate) model: Option<String>,
    pub(crate) service_tier: Option<ServiceTier>,
    pub(crate) context_mode: Option<ContextMode>,
    pub(crate) reasoning_effort: ReasoningEffort,
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

/// Token counters as the display layer needs them. Not a wire type: the
/// app-server payload is parsed by `app_server::protocol::TokenCountBucket`
/// and copied across in `app_server::session::build_token_usage_info`.
#[derive(Debug, Clone, Default)]
pub(crate) struct TokenUsage {
    pub(crate) input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) total_tokens: u64,
}

impl TokenUsage {
    pub(crate) fn total(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    pub(crate) fn tokens_in_context_window(&self) -> u64 {
        if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.total()
        }
    }

    #[cfg(test)]
    fn percent_of_context_window_remaining(&self, context_window: u64) -> u64 {
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

#[derive(Debug, Clone, Default)]
pub struct TokenUsageInfo {
    pub(crate) total_token_usage: TokenUsage,
    pub(crate) last_token_usage: TokenUsage,
    pub(crate) model_context_window: Option<u64>,
}

impl TokenUsageInfo {
    pub(crate) fn context_window_usage(&self) -> &TokenUsage {
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
