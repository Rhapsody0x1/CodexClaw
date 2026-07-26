use anyhow::{Result, anyhow};

use crate::codex::{
    agent_messages_from_lines,
    exec_cli::{self, ExecSpec},
};

/// Run one read-only, ephemeral `codex exec` and return only the agent's
/// message text. Strict contract: a non-zero exit is an error, never partial
/// output — a distillation that half-ran must not be applied.
///
/// The caller owns the invocation (`ExecSpec`); this adds only the shadow
/// policy on top of it.
pub(crate) async fn run_codex_oneshot(spec: ExecSpec<'_>) -> Result<String> {
    let output = exec_cli::run(ExecSpec {
        sandbox: Some("read-only"),
        ephemeral: true,
        ..spec
    })
    .await?;
    if !output.status.success() {
        return Err(anyhow!("codex shadow exited with status {}", output.status));
    }
    Ok(agent_messages_from_lines(output.stdout_lines))
}
