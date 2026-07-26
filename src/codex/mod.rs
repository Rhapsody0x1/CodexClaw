pub mod app_server;
pub mod config_snapshot;
pub mod display;
pub mod events;
pub mod exec_output;
pub mod executor;
pub mod prompt;
pub mod runtime;
pub mod types;

// Façade: the symbols the rest of the crate reaches for, re-exported so callers
// import `crate::codex::X` instead of spelling out the submodule layout.
pub use app_server::{
    AppServerHandle, ApprovalOutcome, ApprovalRequest, ClientInfo, CommandApprovalEvent,
    FileChangeApprovalEvent, PermissionsApprovalEvent,
};
pub use events::{CodexEvent, TokenUsage, TokenUsageInfo};
pub use exec_output::{agent_messages_from_lines, agent_messages_from_stdout};
pub use executor::{CodexExecutor, build_codex_path_env, build_turn_policy};
pub use prompt::build_prompt;
pub use runtime::{
    CodexModelEntry, CodexRuntimeProfile, list_codex_model_entries,
    read_codex_runtime_profile_from_path, write_context_mode_to_config_path,
    write_model_to_config_path, write_reasoning_effort_to_config_path,
    write_service_tier_to_config_path,
};
pub use types::{CompactRequest, ExecutionRequest, ExecutionResult, ExecutionUpdate};
