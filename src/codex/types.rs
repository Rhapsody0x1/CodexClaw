//! Plain data carried across the codex boundary: what a turn is asked to do,
//! what it produced, and what it streams while it runs.
//!
//! This module is a leaf — it depends only on value types, never on the
//! app-server transport or the display formatters.

use std::path::PathBuf;

use crate::{
    codex::events::TokenUsageInfo,
    model::settings::{ContextMode, ReasoningEffort, ServiceTier, SessionState},
};

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
