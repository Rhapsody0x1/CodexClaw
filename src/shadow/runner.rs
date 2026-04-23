use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, anyhow};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::timeout,
};

use crate::codex::events::CodexEvent;

pub fn extract_agent_messages_from_lines<I: IntoIterator<Item = String>>(lines: I) -> String {
    let mut parts = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<CodexEvent>(trimmed) else {
            continue;
        };
        if let CodexEvent::ItemCompleted { item } = event {
            if item.item_type == "agent_message" {
                if let Some(text) = item.text {
                    parts.push(text);
                }
            }
        }
    }
    parts.join("\n")
}

pub struct OneshotConfig<'a> {
    pub codex_binary: &'a str,
    pub workspace_dir: &'a Path,
    pub codex_home: &'a Path,
    pub model: Option<&'a str>,
    pub reasoning: Option<&'a str>,
    pub prompt: &'a str,
    pub deadline: Duration,
}

pub async fn run_codex_oneshot(cfg: OneshotConfig<'_>) -> Result<String> {
    let mut cmd = Command::new(cfg.codex_binary);
    cmd.arg("exec")
        .arg("--sandbox")
        .arg("read-only")
        .arg("--skip-git-repo-check")
        .arg("--ephemeral")
        .arg("--json")
        .arg("-C")
        .arg(cfg.workspace_dir)
        .env("CODEX_HOME", cfg.codex_home);
    if let Some(model) = cfg.model {
        cmd.arg("-m").arg(model);
    }
    if let Some(reasoning) = cfg.reasoning {
        cmd.arg("-c")
            .arg(format!("model_reasoning_effort=\"{reasoning}\""));
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn {}", cfg.codex_binary))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(cfg.prompt.as_bytes())
            .await
            .context("failed to write shadow prompt to codex stdin")?;
        stdin.shutdown().await.ok();
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("codex exec produced no stdout"))?;
    let mut reader = BufReader::new(stdout).lines();

    let mut lines = Vec::new();
    let collect = async {
        while let Some(line) = reader.next_line().await? {
            lines.push(line);
        }
        Ok::<(), anyhow::Error>(())
    };

    match timeout(cfg.deadline, collect).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => return Err(err),
        Err(_) => {
            child.start_kill().ok();
            return Err(anyhow!("codex shadow timed out after {:?}", cfg.deadline));
        }
    }

    let status = child.wait().await.context("codex shadow wait failed")?;
    if !status.success() {
        return Err(anyhow!("codex shadow exited with status {status}"));
    }
    Ok(extract_agent_messages_from_lines(lines))
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
        let result = extract_agent_messages_from_lines(lines);
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
        assert_eq!(extract_agent_messages_from_lines(lines), "kept".to_string());
    }

    #[test]
    fn extract_agent_messages_ignores_invalid_json_lines() {
        let lines = vec![
            "not json".to_string(),
            "".to_string(),
            r#"{"type":"item.completed","item":{"id":"a","type":"agent_message","text":"ok"}}"#
                .to_string(),
        ];
        assert_eq!(extract_agent_messages_from_lines(lines), "ok".to_string());
    }

    #[test]
    fn extract_agent_messages_empty_on_no_items() {
        let lines = vec![r#"{"type":"thread.started","thread_id":"x"}"#.to_string()];
        assert_eq!(extract_agent_messages_from_lines(lines), "".to_string());
    }
}
