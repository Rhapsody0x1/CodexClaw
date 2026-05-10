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
    write_builtin_cron_skill(&local_skills).await?;

    // Old dual-profile mode used `config-codex-claw.toml`; keep a single `config.toml` now.
    let legacy = codex_home.join("config-codex-claw.toml");
    if legacy.exists() {
        let _ = tokio::fs::remove_file(&legacy).await;
    }
    Ok(())
}

async fn write_builtin_cron_skill(skills_root: &Path) -> Result<()> {
    let skill_dir = skills_root.join("claw-cron");
    tokio::fs::create_dir_all(&skill_dir).await?;
    let body = r#"---
name: "claw-cron"
description: "Register and manage codex-claw scheduled tasks from an agent turn."
---

# codex-claw Scheduled Tasks

Use the `codex-claw cron` CLI when the user asks to create, list, pause, resume, remove, or immediately trigger scheduled tasks.

Examples:

```shell
codex-claw cron add --cron "0 16 * * *" --tz Asia/Shanghai --title "homework reminder" --action reminder --message "记得检查还有没有没交的作业"
codex-claw cron once --at "2026-05-20T08:00:00+08:00" --title "interview reminder" --action reminder --message "准备面试"
codex-claw cron add --cron "0 9 * * 1-5" --tz Asia/Shanghai --title "repo health check" --action codex-exec --workspace "/path/to/repo" --prompt "检查这个仓库的测试状态，简短总结发现，不要修改文件。"
codex-claw cron add --cron "0 18 * * 5" --tz Asia/Shanghai --title "weekly project review" --action codex-turn --workspace "/path/to/repo" --session-strategy persistent --prompt "继续跟踪这个项目，本周回顾待办、最近变更和下周风险，给出简短中文总结。"
codex-claw cron list
codex-claw cron run-now <job_id>
codex-claw cron pause <job_id>
codex-claw cron resume <job_id>
codex-claw cron rm <job_id>
codex-claw cron tail <job_id>
```

If you are running inside a codex-claw QQ turn, omit `--owner`; the CLI reads `.claw-turn.json` from the current workspace. Otherwise pass `--owner <openid>`.

The CLI auto-discovers `~/.codex-claw/codexclaw.toml` when `CODEX_CLAW_CONFIG` is not set.

Supported actions:
- `reminder`: requires `--message`. Use this for normal user reminders. The message is the text to send when the schedule fires.
- `shell`: requires `--program`, accepts repeated `--arg`.
- `codex-exec`: runs `codex exec` with `CODEX_HOME` set to codex-claw's global Codex home.
- `codex-turn`: runs one app-server turn; pass `--session-strategy per-invocation` or `--session-strategy persistent`.

Action selection:
- Use `reminder` when the scheduled task only needs to send a fixed reminder text.
- Use `shell` when a deterministic local command is enough and no Codex reasoning is needed, for example running `/bin/echo`, a backup script, or a project script.
- Use `codex-exec` when the scheduled task should run an isolated Codex CLI job. It is best for stateless checks or one-off analysis in a workspace. It does not preserve an app-server conversation across runs.
- Use `codex-turn` when the scheduled task should behave like a codex-claw conversation turn. It can keep a Codex session when `--session-strategy persistent` is used, making it suitable for recurring project monitoring or follow-up work that benefits from continuity.

`codex-exec` examples:
```shell
codex-claw cron once --at "2026-05-20T09:00:00+08:00" --title "test summary" --action codex-exec --workspace "/path/to/repo" --prompt "运行或检查测试状态，只返回失败点和下一步建议，不要修改文件。"
codex-claw cron add --cron "0 9 * * 1" --tz Asia/Shanghai --title "weekly dependency scan" --action codex-exec --workspace "/path/to/repo" --prompt "查看依赖相关文件，提醒我是否有明显需要升级或审查的依赖。"
```

`codex-turn` examples:
```shell
codex-claw cron add --cron "0 18 * * 5" --tz Asia/Shanghai --title "weekly project review" --action codex-turn --workspace "/path/to/repo" --session-strategy persistent --prompt "延续上次项目跟踪，汇总本周进展、未完成事项和下周风险。"
codex-claw cron once --at "2026-05-20T20:00:00+08:00" --title "one-off agent check" --action codex-turn --workspace "/path/to/repo" --session-strategy per-invocation --prompt "检查 README 中是否有过期命令，只报告发现，不要修改文件。"
```

Important:
- For requests like "提醒我 X", "每天 16:00 提醒我 X", or "明天提醒我 X", create a `reminder` job. Do not use `codex-exec` or `codex-turn`.
- The reminder message must be only the content the user wants to receive at trigger time, for example `X`. It must not be the full scheduling request like "每天 16:00 提醒我 X".
- Use `codex-exec` or `codex-turn` only when the scheduled task should perform autonomous Codex work at trigger time, such as checking files, running commands, summarizing results, or editing code.
- If you use `codex-exec` or `codex-turn`, write the prompt as an execution instruction for the future trigger. Do not ask Codex to create another reminder.
"#;
    tokio::fs::write(skill_dir.join("SKILL.md"), body).await?;
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
        let cron_skill =
            tokio::fs::read_to_string(codex_home.path().join("skills/claw-cron/SKILL.md"))
                .await
                .unwrap();
        assert!(cron_skill.contains("--action reminder"));
        assert!(cron_skill.contains("Do not use `codex-exec` or `codex-turn`"));
        assert!(cron_skill.contains("Use `codex-exec` when"));
        assert!(cron_skill.contains("Use `codex-turn` when"));
    }
}
