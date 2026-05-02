use std::{path::Path, sync::Arc, time::Duration};

use chrono::{Datelike, Local, Timelike};
use rand::seq::SliceRandom;
use tracing::{error, warn};

use crate::app::App;
use crate::time::now_in_beijing;

pub fn spawn_daily_408_scheduler(app: Arc<App>) {
    tokio::spawn(async move {
        loop {
            if let Err(err) = run_once(&app).await {
                error!(error = %err, "daily 408 scheduler tick failed");
            }
            tokio::time::sleep(sleep_duration()).await;
        }
    });
}

pub fn spawn_one_shot_push_scheduler(app: Arc<App>) {
    tokio::spawn(async move {
        loop {
            if let Err(err) = run_one_shot_once(&app).await {
                error!(error = %err, "one-shot push scheduler tick failed");
            }
            tokio::time::sleep(sleep_duration()).await;
        }
    });
}

async fn run_once(app: &Arc<App>) -> anyhow::Result<()> {
    let now = now_in_beijing();
    let today = format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day());

    for (openid, cfg) in app.session.list_daily_408_targets().await {
        if now.hour() != u32::from(cfg.hour) {
            continue;
        }
        if cfg.last_pushed_on.as_deref() == Some(today.as_str()) {
            continue;
        }

        let content = match build_daily_question(&cfg.note_dir).await {
            Ok(Some(question)) => format!(
                "408 每日一题（操作系统）\n\n{question}\n\n请回复你的答案（例如：A）和理由。"
            ),
            Ok(None) => format!(
                "408 每日一题：未在 {} 找到可解析题目。请放入 .md 或 .txt 题库文件。",
                cfg.note_dir.display()
            ),
            Err(err) => {
                warn!(error = %err, openid = %openid, "failed to build 408 question");
                format!(
                    "408 每日一题：读取题库失败（{}）。请检查 note408 目录权限与文件格式（建议 .md/.txt）。",
                    err
                )
            }
        };

        app.qq_client.send_text_proactive(&openid, &content).await?;
        app.session.mark_daily_408_pushed(&openid, &today).await?;
    }

    Ok(())
}

async fn run_one_shot_once(app: &Arc<App>) -> anyhow::Result<()> {
    let now_local = now_in_beijing();
    let today = format!(
        "{:04}-{:02}-{:02}",
        now_local.year(),
        now_local.month(),
        now_local.day()
    );
    let now_utc = chrono::Utc::now();

    for (openid, cfg) in app.session.list_one_shot_push_targets().await {
        if cfg.expires_on != today {
            app.session.set_one_shot_push(&openid, None).await?;
            continue;
        }
        if now_utc < cfg.trigger_at {
            continue;
        }
        app.qq_client.send_text_proactive(&openid, &cfg.message).await?;
        app.session.set_one_shot_push(&openid, None).await?;
    }
    Ok(())
}

fn sleep_duration() -> Duration {
    let now = Local::now();
    Duration::from_secs(60_u64.saturating_sub(now.second() as u64).max(5))
}

async fn build_daily_question(note_dir: &Path) -> anyhow::Result<Option<String>> {
    let files = collect_text_files(note_dir).await?;
    if files.is_empty() {
        return Ok(None);
    }

    let mut blocks = Vec::new();
    for path in files {
        let raw = match tokio::fs::read_to_string(&path).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        blocks.extend(extract_question_blocks(&raw));
    }

    if blocks.is_empty() {
        return Ok(None);
    }

    let mut rng = rand::thread_rng();
    Ok(blocks.choose(&mut rng).cloned())
}

async fn collect_text_files(root: &Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let Ok(ft) = entry.file_type().await else {
                continue;
            };
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|v| v.to_str()) else {
                continue;
            };
            let ext = ext.to_ascii_lowercase();
            if ext == "md" || ext == "txt" {
                out.push(path);
            }
        }
    }

    Ok(out)
}

fn extract_question_blocks(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    let mut saw_option = false;

    for line in raw.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if is_question_start_line(line) {
            push_question_block(&mut out, &mut current, saw_option);
            current.push(line.to_string());
            saw_option = false;
            continue;
        }
        if current.is_empty() {
            continue;
        }
        if is_question_noise_line(line) {
            continue;
        }
        if is_option_line(line) {
            saw_option = true;
            current.push(line.to_string());
            continue;
        }
        if is_section_boundary(line) {
            push_question_block(&mut out, &mut current, saw_option);
            saw_option = false;
            continue;
        }
        current.push(line.to_string());
    }

    push_question_block(&mut out, &mut current, saw_option);
    out
}

fn push_question_block(out: &mut Vec<String>, current: &mut Vec<String>, saw_option: bool) {
    if current.is_empty() || !saw_option {
        current.clear();
        return;
    }
    let block = current.join("\n");
    if (20..=1200).contains(&block.len()) {
        out.push(block);
    }
    current.clear();
}

fn is_question_start_line(line: &str) -> bool {
    let mut chars = line.chars().peekable();
    let mut saw_digit = false;
    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() {
            saw_digit = true;
            chars.next();
            continue;
        }
        break;
    }
    if !saw_digit {
        return false;
    }
    matches!(chars.next(), Some('.' | '、'))
}

fn is_option_line(line: &str) -> bool {
    let mut chars = line.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some('A' | 'B' | 'C' | 'D'), Some('.' | '、' | ' '))
    )
}

fn is_question_noise_line(line: &str) -> bool {
    line.starts_with("--- 第 ")
        || line.starts_with('#')
        || line.starts_with("- [")
        || line == "目录"
        || line.starts_with("闲鱼:")
        || line.contains("做题本")
}

fn is_section_boundary(line: &str) -> bool {
    line.starts_with("## ")
        || line.starts_with("### ")
        || line.starts_with("第") && line.contains("章")
}

#[cfg(test)]
mod tests {
    use super::extract_question_blocks;

    #[test]
    fn extract_question_blocks_handles_note408_markdown_layout() {
        let raw = "## 第1章 计算机系统概述\n\
1. 操作系统是对 (   ) 进行管理的软件。\n\
A. 软件                                   B. 硬件\n\
C. 计算机资源                             D. 应用程序\n\
\n\
\n\
2. 下面的 (    ) 资源不是操作系统应该管理的。\n\
A. CPU B. 内存 C. 外存 D. 源程序\n";
        let blocks = extract_question_blocks(raw);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("1. 操作系统是对"));
        assert!(blocks[0].contains("A. 软件"));
        assert!(blocks[1].contains("2. 下面的"));
    }
}
