use std::path::Path;

use anyhow::{Context, Result};
use tracing::warn;

pub async fn bootstrap_codex_home(codex_home: &Path, system_codex_home: &Path) -> Result<()> {
    tokio::fs::create_dir_all(codex_home).await?;

    copy_file_if_missing(
        &system_codex_home.join("config.toml"),
        &codex_home.join("config.toml"),
    )
    .await?;
    copy_file_if_missing(
        &system_codex_home.join("auth.json"),
        &codex_home.join("auth.json"),
    )
    .await?;

    let system_skills = system_codex_home.join("skills");
    let local_skills = codex_home.join("skills");
    if !local_skills.exists() {
        copy_dir_recursive_if_exists(&system_skills, &local_skills).with_context(|| {
            format!(
                "failed to copy skills directory from {} to {}",
                system_skills.display(),
                local_skills.display()
            )
        })?;
    }

    // Old dual-profile mode used `config-codex-claw.toml`; keep a single `config.toml` now.
    let legacy = codex_home.join("config-codex-claw.toml");
    if legacy.exists() {
        let _ = tokio::fs::remove_file(&legacy).await;
    }
    Ok(())
}

async fn copy_file_if_missing(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        return Ok(());
    }
    if !source.exists() {
        warn!(
            source = %source.display(),
            destination = %destination.display(),
            "skip copy because source does not exist"
        );
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::copy(source, destination)
        .await
        .with_context(|| {
            format!(
                "failed to copy {} to {}",
                source.display(),
                destination.display()
            )
        })?;
    Ok(())
}

fn copy_dir_recursive_if_exists(source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    for entry in
        std::fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_recursive_if_exists(&source_path, &destination_path)?;
        } else if ty.is_file() {
            std::fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::bootstrap_codex_home;

    #[tokio::test]
    async fn bootstrap_removes_legacy_codex_claw_config() {
        let codex_home = tempdir().unwrap();
        let system_home = tempdir().unwrap();
        tokio::fs::write(
            system_home.path().join("config.toml"),
            "model = \"gpt-5.4\"",
        )
        .await
        .unwrap();
        tokio::fs::write(system_home.path().join("auth.json"), "{}")
            .await
            .unwrap();
        tokio::fs::write(codex_home.path().join("config-codex-claw.toml"), "legacy")
            .await
            .unwrap();

        bootstrap_codex_home(codex_home.path(), system_home.path())
            .await
            .unwrap();
        assert!(!codex_home.path().join("config-codex-claw.toml").exists());
        assert!(codex_home.path().join("config.toml").exists());
    }
}
