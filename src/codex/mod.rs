pub mod app_server;
pub mod config_snapshot;
pub(crate) mod display;
pub(crate) mod events;
pub(crate) mod exec_output;
pub(crate) mod executor;
pub(crate) mod prompt;
pub(crate) mod runtime;
pub(crate) mod types;

// Façade: the symbols the rest of the crate reaches for, re-exported so callers
// import `crate::codex::X` instead of spelling out the submodule layout.
//
// The `pub` block is the part the binary and the app-server smoke test consume;
// everything else is `pub(crate)` so the compiler keeps reporting unused items
// instead of treating "some external crate might want it" as a use.
pub use app_server::{AppServerHandle, ClientInfo};
pub use events::TokenUsageInfo;
pub use executor::{CodexExecutor, build_codex_path_env};
pub use types::{ExecutionRequest, ExecutionResult, ExecutionUpdate};

pub(crate) use app_server::{
    ApprovalOutcome, ApprovalRequest, CommandApprovalEvent, FileChangeApprovalEvent,
    PermissionsApprovalEvent,
};
pub(crate) use events::CodexEvent;
pub(crate) use exec_output::{agent_messages_from_lines, agent_messages_from_stdout};
pub(crate) use prompt::build_prompt;
pub(crate) use runtime::{
    CodexModelEntry, CodexRuntimeProfile, list_codex_model_entries,
    read_codex_runtime_profile_from_path, write_context_mode_to_config_path,
    write_model_to_config_path, write_reasoning_effort_to_config_path,
    write_service_tier_to_config_path,
};
pub(crate) use types::CompactRequest;
