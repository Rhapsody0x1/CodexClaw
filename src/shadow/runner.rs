use std::{path::Path, time::Duration};

use anyhow::{Result, anyhow};

use crate::codex::{
    agent_messages_from_lines,
    exec_cli::{self, ExecSpec},
};

pub(crate) struct OneshotConfig<'a> {
    pub(crate) codex_binary: &'a str,
    pub(crate) workspace_dir: &'a Path,
    pub(crate) codex_home: &'a Path,
    pub(crate) model: Option<&'a str>,
    pub(crate) reasoning: Option<&'a str>,
    pub(crate) prompt: &'a str,
    pub(crate) deadline: Duration,
}

/// Run one read-only, ephemeral `codex exec` and return only the agent's
/// message text. Strict contract: a non-zero exit is an error, never partial
/// output — a distillation that half-ran must not be applied.
pub(crate) async fn run_codex_oneshot(cfg: OneshotConfig<'_>) -> Result<String> {
    let output = exec_cli::run(ExecSpec {
        binary: cfg.codex_binary,
        codex_home: cfg.codex_home,
        cwd: cfg.workspace_dir,
        prompt: cfg.prompt,
        model: cfg.model,
        reasoning: cfg.reasoning,
        sandbox: Some("read-only"),
        ephemeral: true,
        extra_args: &[],
        env: None,
        deadline: cfg.deadline,
        capture_stderr: false,
        label: "codex shadow",
    })
    .await?;
    if !output.status.success() {
        return Err(anyhow!("codex shadow exited with status {}", output.status));
    }
    Ok(agent_messages_from_lines(output.stdout_lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_agent_messages_concatenates_agent_text_items() {
        let lines = vec![
            r#"{"type":"thread.started","thread_id":"x"}"#.to_string(),
            r#"{"type":"item.completed","item":{"id":"a","type":"agent_message","text":"first"}}"#
                .to_string(),
            r#"{"type":"item.completed","item":{"id":"b","type":"agent_message","text":"second"}}"#
                .to_string(),
            r#"{"type":"turn.completed"}"#.to_string(),
        ];
        let result = agent_messages_from_lines(lines);
        assert_eq!(result, "first\nsecond");
    }

    #[test]
    fn extract_agent_messages_skips_non_agent_items() {
        let lines = vec![
            r#"{"type":"item.completed","item":{"id":"a","type":"tool_call","text":"ignored"}}"#
                .to_string(),
            r#"{"type":"item.completed","item":{"id":"b","type":"agent_message","text":"kept"}}"#
                .to_string(),
        ];
        assert_eq!(agent_messages_from_lines(lines), "kept".to_string());
    }

    #[test]
    fn extract_agent_messages_ignores_invalid_json_lines() {
        let lines = vec![
            "not json".to_string(),
            "".to_string(),
            r#"{"type":"item.completed","item":{"id":"a","type":"agent_message","text":"ok"}}"#
                .to_string(),
        ];
        assert_eq!(agent_messages_from_lines(lines), "ok".to_string());
    }

    #[test]
    fn extract_agent_messages_empty_on_no_items() {
        let lines = vec![r#"{"type":"thread.started","thread_id":"x"}"#.to_string()];
        assert_eq!(agent_messages_from_lines(lines), "".to_string());
    }
}
