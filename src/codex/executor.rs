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
        types::{CompactRequest, ExecutionRequest, ExecutionResult, ExecutionUpdate},
    },
    session::state::ApprovalPolicySetting,
    util::{layout::DataLayout, path::search_path_dirs},
};

#[derive(Clone)]
pub struct CodexExecutor {
    pub binary: PathBuf,
    pub sqlite_home: PathBuf,
    handle: Arc<AppServerHandle>,
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

#[cfg(test)]
mod tests {
    use std::{env, ffi::OsString};

    use tempfile::tempdir;

    use crate::codex::executor::build_codex_path_env;

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
