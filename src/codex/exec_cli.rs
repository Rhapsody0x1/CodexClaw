//! Shared runner for one-shot `codex exec --json` subprocesses.
//!
//! Both one-shot callers — cron `CodexExec` jobs and the shadow memory
//! distiller — go through [`run`], so the hard-won process hygiene lives in
//! exactly one place:
//!
//! - the prompt travels over **stdin**, never argv (argv is visible to every
//!   user on the host via `ps` and caps out at ARG_MAX);
//! - stderr is drained concurrently — `codex exec` logs progress there, and
//!   an undrained pipe fills (~64KB), blocks the child, and stalls stdout
//!   until the deadline;
//! - the deadline kills the child instead of abandoning it, and even after
//!   stdout closes the reap is guarded so a wedged child cannot hang the
//!   caller forever.
//!
//! Interpreting the exit status and shaping the output stay with the
//! callers: the two sites have deliberately different contracts (the cron
//! path folds stderr into its error text and falls back to raw stdout, the
//! shadow path is strict and agent-messages-only).

use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, anyhow};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::timeout,
};

/// Everything needed to run one `codex exec` invocation.
pub(crate) struct ExecSpec<'a> {
    pub(crate) binary: &'a str,
    pub(crate) codex_home: &'a Path,
    pub(crate) cwd: &'a Path,
    /// Written to the child's stdin.
    pub(crate) prompt: &'a str,
    pub(crate) model: Option<&'a str>,
    /// Value for `-c model_reasoning_effort="…"`.
    pub(crate) reasoning: Option<&'a str>,
    /// Value for `--sandbox <mode>`.
    pub(crate) sandbox: Option<&'a str>,
    pub(crate) ephemeral: bool,
    /// Caller-supplied extra CLI arguments, appended last.
    pub(crate) extra_args: &'a [String],
    /// Extra environment variables on top of `CODEX_HOME`.
    pub(crate) env: Option<&'a BTreeMap<String, String>>,
    pub(crate) deadline: Duration,
    /// Prefix for error messages, e.g. `codex exec` / `codex shadow`.
    /// Timeout messages keep the phrase "timed out" — the scheduler's retry
    /// classifier matches on it.
    pub(crate) label: &'a str,
}

impl<'a> ExecSpec<'a> {
    /// A spec with every optional knob neutral, for callers that set only the
    /// few fields they care about via struct-update syntax:
    /// `ExecSpec { ephemeral: true, ..ExecSpec::new(bin, home, cwd, prompt, label, deadline) }`.
    pub(crate) fn new(
        binary: &'a str,
        codex_home: &'a Path,
        cwd: &'a Path,
        prompt: &'a str,
        label: &'a str,
        deadline: Duration,
    ) -> Self {
        Self {
            binary,
            codex_home,
            cwd,
            prompt,
            model: None,
            reasoning: None,
            sandbox: None,
            ephemeral: false,
            extra_args: &[],
            env: None,
            deadline,
            label,
        }
    }
}

/// What the child produced. The exit status is data, not an error: each
/// caller renders its own failure message.
pub(crate) struct ExecOutput {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout_lines: Vec<String>,
    /// Everything the child wrote to stderr (`codex exec` logs progress
    /// there). Callers that don't want it just ignore the field — it has to
    /// be drained either way, or the pipe fills and stalls the child.
    pub(crate) stderr: String,
}

/// The argv for `spec`, without the prompt (which goes over stdin).
/// `--skip-git-repo-check` and `--json` are always present but never
/// duplicated when the caller already passes them in `extra_args`.
fn build_args(spec: &ExecSpec<'_>) -> Vec<String> {
    let mut args = vec!["exec".to_string()];
    if !spec
        .extra_args
        .iter()
        .any(|arg| arg == "--skip-git-repo-check")
    {
        args.push("--skip-git-repo-check".to_string());
    }
    if !spec.extra_args.iter().any(|arg| arg == "--json") {
        args.push("--json".to_string());
    }
    if let Some(sandbox) = spec.sandbox {
        args.push("--sandbox".to_string());
        args.push(sandbox.to_string());
    }
    if spec.ephemeral {
        args.push("--ephemeral".to_string());
    }
    if let Some(model) = spec.model {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    if let Some(reasoning) = spec.reasoning {
        args.push("-c".to_string());
        args.push(format!("model_reasoning_effort=\"{reasoning}\""));
    }
    args.extend(spec.extra_args.iter().cloned());
    args
}

pub(crate) async fn run(spec: ExecSpec<'_>) -> Result<ExecOutput> {
    let mut cmd = Command::new(spec.binary);
    cmd.args(build_args(&spec))
        .env("CODEX_HOME", spec.codex_home)
        .current_dir(spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Any early return / drop kills the child instead of leaking a
        // still-running codex process.
        .kill_on_drop(true);
    if let Some(env) = spec.env {
        cmd.envs(env);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn {}", spec.binary))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(spec.prompt.as_bytes())
            .await
            .with_context(|| format!("failed to write {} prompt to codex stdin", spec.label))?;
        stdin.shutdown().await.ok();
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("codex exec produced no stdout"))?;
    let stderr = child.stderr.take();

    let mut stdout_lines = Vec::new();
    let mut stderr_text = String::new();
    let collect = async {
        let read_stdout = async {
            let mut reader = BufReader::new(stdout).lines();
            while let Some(line) = reader.next_line().await? {
                stdout_lines.push(line);
            }
            Ok::<(), anyhow::Error>(())
        };
        let drain_stderr = async {
            if let Some(stderr) = stderr {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    if !stderr_text.is_empty() {
                        stderr_text.push('\n');
                    }
                    stderr_text.push_str(&line);
                }
            }
        };
        let (stdout_result, ()) = tokio::join!(read_stdout, drain_stderr);
        stdout_result
    };

    match timeout(spec.deadline, collect).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => return Err(err),
        Err(_) => {
            child.start_kill().ok();
            let _ = timeout(Duration::from_secs(5), child.wait()).await;
            return Err(anyhow!(
                "{} timed out after {}s",
                spec.label,
                spec.deadline.as_secs()
            ));
        }
    }

    // Even after stdout closes the child could wedge; never wait unbounded.
    let status = match timeout(Duration::from_secs(5), child.wait()).await {
        Ok(status) => status.with_context(|| format!("{} wait failed", spec.label))?,
        Err(_) => {
            child.start_kill().ok();
            return Err(anyhow!("{} did not exit after stdout closed", spec.label));
        }
    };
    Ok(ExecOutput {
        status,
        stdout_lines,
        stderr: stderr_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec<'a>(extra_args: &'a [String], home: &'a Path, cwd: &'a Path) -> ExecSpec<'a> {
        ExecSpec {
            extra_args,
            ..ExecSpec::new(
                "codex",
                home,
                cwd,
                "hello",
                "codex exec",
                Duration::from_secs(1),
            )
        }
    }

    #[test]
    fn args_include_git_repo_check_skip_and_json_by_default() {
        let home = PathBuf::from("/home");
        let cwd = PathBuf::from("/ws");
        let extra = vec!["--output-schema".to_string(), "{}".to_string()];
        let mut s = spec(&extra, &home, &cwd);
        s.model = Some("gpt-5.5");
        assert_eq!(
            build_args(&s),
            vec![
                "exec",
                "--skip-git-repo-check",
                "--json",
                "--model",
                "gpt-5.5",
                "--output-schema",
                "{}",
            ]
        );
    }

    #[test]
    fn args_do_not_duplicate_explicit_flags_and_never_carry_the_prompt() {
        let home = PathBuf::from("/home");
        let cwd = PathBuf::from("/ws");
        let extra = vec!["--skip-git-repo-check".to_string(), "--json".to_string()];
        let args = build_args(&spec(&extra, &home, &cwd));
        assert_eq!(args, vec!["exec", "--skip-git-repo-check", "--json"]);
        assert!(
            !args.iter().any(|arg| arg == "hello"),
            "the prompt must travel over stdin, not argv"
        );
    }

    #[test]
    fn args_render_sandbox_ephemeral_and_reasoning() {
        let home = PathBuf::from("/home");
        let cwd = PathBuf::from("/ws");
        let extra: Vec<String> = Vec::new();
        let mut s = spec(&extra, &home, &cwd);
        s.sandbox = Some("read-only");
        s.ephemeral = true;
        s.reasoning = Some("low");
        assert_eq!(
            build_args(&s),
            vec![
                "exec",
                "--skip-git-repo-check",
                "--json",
                "--sandbox",
                "read-only",
                "--ephemeral",
                "-c",
                "model_reasoning_effort=\"low\"",
            ]
        );
    }
}
