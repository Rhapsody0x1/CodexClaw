use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::warn;

use crate::{
    config::AppConfig,
    util::{
        layout::DataLayout, path::search_path_dirs as util_search_path_dirs,
        text::truncate_with_marker,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BuildRecord {
    pub(crate) built_at: String,
    pub(crate) command: String,
    pub(crate) binary_path: String,
    pub(crate) success: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct BuildResult {
    pub(crate) success: bool,
    pub(crate) binary_path: PathBuf,
    pub(crate) summary: String,
}

pub(crate) fn changed_self_repo(
    workspace_dir: &Path,
    changed_files: &[PathBuf],
    self_repo_dir: &Path,
) -> bool {
    changed_files.iter().any(|path| {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            workspace_dir.join(path)
        };
        absolute.starts_with(self_repo_dir)
    })
}

pub(crate) async fn ensure_successful_build(config: &AppConfig) -> Result<BuildResult> {
    // Self-update must deploy the current working tree, not a previously
    // recorded build artifact. Reusing last-build.json can silently roll the
    // service back to an older binary when source files changed after the last
    // successful release build.
    run_build(config).await
}

pub(crate) async fn run_build(config: &AppConfig) -> Result<BuildResult> {
    let parts = shlex::split(&config.general.self_build_command).ok_or_else(|| {
        anyhow!(
            "invalid self_build_command: {}",
            config.general.self_build_command
        )
    })?;
    let Some(program) = parts.first() else {
        return Err(anyhow!("self_build_command is empty"));
    };
    let resolved_program = resolve_program(program);
    let mut command = Command::new(&resolved_program);
    if parts.len() > 1 {
        command.args(&parts[1..]);
    }
    if let Some(path_env) = build_command_path_env(
        env::var_os("PATH").as_ref(),
        env::var_os("HOME").as_deref().map(Path::new),
    ) {
        command.env("PATH", path_env);
    }
    command
        .current_dir(&config.general.self_repo_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = command.output().await.with_context(|| {
        format!(
            "failed to execute build command `{}`",
            config.general.self_build_command
        )
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut snippets = Vec::new();
    if !stdout.trim().is_empty() {
        snippets.push(format!("stdout:\n{}", truncate(stdout.trim(), 1200)));
    }
    if !stderr.trim().is_empty() {
        snippets.push(format!("stderr:\n{}", truncate(stderr.trim(), 1200)));
    }
    let binary_path = if config.general.self_binary_path.is_absolute() {
        config.general.self_binary_path.clone()
    } else {
        config
            .general
            .self_repo_dir
            .join(&config.general.self_binary_path)
    };
    let success = output.status.success() && binary_path.exists();
    let record = BuildRecord {
        built_at: Utc::now().to_rfc3339(),
        command: config.general.self_build_command.clone(),
        binary_path: binary_path.display().to_string(),
        success,
    };
    save_last_build_record(&config.general.data_dir, &record).await?;
    let summary = if success {
        format!("构建成功：`{}`", binary_path.display())
    } else {
        format!(
            "构建失败（status={}，binary_exists={}）",
            output.status,
            binary_path.exists()
        )
    };
    let summary = if snippets.is_empty() {
        summary
    } else {
        format!("{}\n\n{}", summary, snippets.join("\n\n"))
    };
    Ok(BuildResult {
        success,
        binary_path,
        summary,
    })
}

async fn save_last_build_record(data_dir: &Path, record: &BuildRecord) -> Result<()> {
    let path = last_build_path(data_dir);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let raw = serde_json::to_string_pretty(record)?;
    tokio::fs::write(&path, raw)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn last_build_path(data_dir: &Path) -> PathBuf {
    DataLayout::new(data_dir).last_build_file()
}

fn resolve_program(program: &str) -> PathBuf {
    resolve_program_from_env(
        program,
        env::var_os("PATH").as_ref(),
        env::var_os("HOME").as_deref().map(Path::new),
    )
    .unwrap_or_else(|| PathBuf::from(program))
}

fn resolve_program_from_env(
    program: &str,
    path_env: Option<&OsString>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    let raw = Path::new(program);
    if raw.is_absolute() || program.contains(std::path::MAIN_SEPARATOR) {
        return Some(raw.to_path_buf());
    }
    search_path_dirs(path_env, home)
        .into_iter()
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn build_command_path_env(path_env: Option<&OsString>, home: Option<&Path>) -> Option<OsString> {
    let dirs = search_path_dirs(path_env, home);
    if dirs.is_empty() {
        return None;
    }
    env::join_paths(dirs).ok()
}

/// Where to look for the build/install toolchain. Deliberately narrower than
/// the codex turn search list: this resolves the programs we run ourselves.
const BUILD_HOME_BIN_DIRS: &[&str] = &[".cargo/bin"];
const BUILD_SYSTEM_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"];

fn search_path_dirs(path_env: Option<&OsString>, home: Option<&Path>) -> Vec<PathBuf> {
    util_search_path_dirs(path_env, home, BUILD_HOME_BIN_DIRS, BUILD_SYSTEM_BIN_DIRS)
}

fn truncate(input: &str, max_chars: usize) -> String {
    truncate_with_marker(input, max_chars, " ...")
}

/// Run a freshly built binary with `--smoke-test` and require a clean, timely
/// exit, so a binary that compiles but panics on startup (bad config parse,
/// arg handling, env drift) is caught BEFORE it overwrites the running binary.
/// `--smoke-test` loads and normalizes config, unlike `--help` which returns
/// before any startup work.
pub(crate) async fn smoke_test_binary(binary: &Path) -> Result<()> {
    use std::process::Stdio;
    use std::time::Duration;

    let mut child = Command::new(binary)
        .arg("--smoke-test")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to spawn {} for smoke test", binary.display()))?;
    match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(anyhow!("smoke test exited with {status}")),
        Ok(Err(err)) => Err(anyhow!("smoke test wait failed: {err}")),
        Err(_) => {
            child.start_kill().ok();
            Err(anyhow!("smoke test timed out after 30s"))
        }
    }
}

pub(crate) async fn replace_binary_for_restart(
    source_binary: &Path,
    target_binary: &Path,
) -> Result<()> {
    anyhow::ensure!(
        source_binary.exists(),
        "build output does not exist: {}",
        source_binary.display()
    );
    let target_dir = target_binary
        .parent()
        .ok_or_else(|| anyhow!("invalid target binary path: {}", target_binary.display()))?;
    tokio::fs::create_dir_all(target_dir).await?;
    let tmp_name = format!(
        ".{}-{}.new",
        target_binary
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("codex-claw"),
        Utc::now().timestamp_millis()
    );
    let staged = target_dir.join(tmp_name);
    tokio::fs::copy(source_binary, &staged)
        .await
        .with_context(|| {
            format!(
                "failed to stage binary from {} to {}",
                source_binary.display(),
                staged.display()
            )
        })?;

    if let Ok(meta) = tokio::fs::metadata(target_binary).await {
        let _ = tokio::fs::set_permissions(&staged, meta.permissions()).await;
    } else if let Ok(meta) = tokio::fs::metadata(source_binary).await {
        let _ = tokio::fs::set_permissions(&staged, meta.permissions()).await;
    }

    #[cfg(target_os = "macos")]
    if let Err(err) = codesign_ad_hoc(&staged).await {
        let _ = tokio::fs::remove_file(&staged).await;
        return Err(err);
    }

    // Keep the previous good binary as a .bak so a broken restart can be rolled
    // back manually. Best-effort: a missing target (first install) is fine.
    if target_binary.exists() {
        let backup = target_binary.with_file_name(format!(
            "{}.bak",
            target_binary
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("codex-claw")
        ));
        if let Err(err) = tokio::fs::copy(target_binary, &backup).await {
            warn!(error = %err, "failed to back up current binary before self-update");
        }
    }

    if let Err(err) = tokio::fs::rename(&staged, target_binary).await {
        let _ = tokio::fs::remove_file(&staged).await;
        return Err(err).with_context(|| {
            format!(
                "failed to replace running binary {} with {}",
                target_binary.display(),
                source_binary.display()
            )
        });
    }
    Ok(())
}

#[cfg(target_os = "macos")]
async fn codesign_ad_hoc(path: &Path) -> Result<()> {
    let mut command = Command::new(resolve_program("codesign"));
    if let Some(path_env) = build_command_path_env(
        env::var_os("PATH").as_ref(),
        env::var_os("HOME").as_deref().map(Path::new),
    ) {
        command.env("PATH", path_env);
    }
    let output = command
        .arg("--force")
        .arg("--sign")
        .arg("-")
        .arg(path)
        .output()
        .await
        .with_context(|| format!("failed to run codesign for {}", path.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "codesign failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{build_command_path_env, resolve_program_from_env};

    #[test]
    fn resolve_program_falls_back_to_home_cargo_bin() {
        let home = tempdir().unwrap();
        let cargo_dir = home.path().join(".cargo/bin");
        std::fs::create_dir_all(&cargo_dir).unwrap();
        let cargo_path = cargo_dir.join("cargo");
        std::fs::write(&cargo_path, "#!/bin/sh\n").unwrap();

        // Point PATH at a guaranteed-empty dir rather than the real /usr/bin, so
        // the fallback is exercised regardless of what is installed on the host.
        let empty_path = tempdir().unwrap();
        let resolved = resolve_program_from_env(
            "cargo",
            Some(&OsString::from(empty_path.path())),
            Some(home.path()),
        );

        assert_eq!(resolved.as_deref(), Some(cargo_path.as_path()));
    }

    #[test]
    fn build_path_env_includes_cargo_bin_fallback() {
        let home = tempdir().unwrap();
        let joined =
            build_command_path_env(Some(&OsString::from("/usr/bin")), Some(home.path())).unwrap();
        let paths = std::env::split_paths(&joined).collect::<Vec<_>>();

        assert!(paths.contains(&PathBuf::from("/usr/bin")));
        assert!(paths.contains(&home.path().join(".cargo/bin")));
    }
}
