use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::config::AppConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildRecord {
    pub built_at: String,
    pub command: String,
    pub binary_path: String,
    pub success: bool,
}

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub success: bool,
    pub binary_path: PathBuf,
    pub summary: String,
}

pub fn changed_self_repo(
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

pub async fn ensure_successful_build(config: &AppConfig) -> Result<BuildResult> {
    if let Some(record) = load_last_build_record(&config.general.data_dir).await?
        && record.success
    {
        let binary_path = PathBuf::from(record.binary_path);
        if binary_path.exists() {
            return Ok(BuildResult {
                success: true,
                binary_path,
                summary: "复用最近一次成功构建产物。".to_string(),
            });
        }
    }
    run_build(config).await
}

pub async fn run_build(config: &AppConfig) -> Result<BuildResult> {
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

pub async fn load_last_build_record(data_dir: &Path) -> Result<Option<BuildRecord>> {
    let path = last_build_path(data_dir);
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return Ok(None);
    };
    let record = serde_json::from_str::<BuildRecord>(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(record))
}

pub async fn save_last_build_record(data_dir: &Path, record: &BuildRecord) -> Result<()> {
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
    data_dir.join("self-update").join("last-build.json")
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

fn search_path_dirs(path_env: Option<&OsString>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path_env) = path_env {
        for dir in env::split_paths(path_env) {
            push_unique_dir(&mut dirs, dir);
        }
    }
    if let Some(home) = home {
        push_unique_dir(&mut dirs, home.join(".cargo").join("bin"));
    }
    for dir in [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ] {
        push_unique_dir(&mut dirs, dir);
    }
    dirs
}

fn push_unique_dir(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dirs.iter().any(|existing| existing == &dir) {
        dirs.push(dir);
    }
}

fn truncate(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut value = input.chars().take(max_chars).collect::<String>();
    value.push_str(" ...");
    value
}

pub async fn replace_binary_for_restart(source_binary: &Path, target_binary: &Path) -> Result<()> {
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
        let path_dir = home.path().join("empty-path");
        std::fs::create_dir_all(&cargo_dir).unwrap();
        std::fs::create_dir_all(&path_dir).unwrap();
        let cargo_path = cargo_dir.join("cargo");
        std::fs::write(&cargo_path, "#!/bin/sh\n").unwrap();

        let resolved = resolve_program_from_env(
            "cargo",
            Some(&path_dir.into_os_string()),
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
