use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
};

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};

use crate::session::state::ContextMode;

pub const SUMMARIZATION_PROMPT: &str = "You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.\n\nInclude:\n- Current progress and key decisions made\n- Important context, constraints, or user preferences\n- What remains to be done (clear next steps)\n- Any critical data, examples, or references needed to continue\n\nBe concise, structured, and focused on helping the next LLM seamlessly continue the work.";

pub const SUMMARY_PREFIX: &str = "Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the state of the tools that were used by that language model. Use this to build on the work that has already been done and avoid duplicating work. Here is the summary produced by the other language model, use the information in this summary to assist with your own analysis:";

const APPROX_BYTES_PER_TOKEN: usize = 4;
const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;
const ONE_M_CONTEXT_WINDOW: u64 = 1_000_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RolloutSnapshot {
    pub user_messages: Vec<String>,
}

pub fn context_mode_window(mode: ContextMode) -> u64 {
    match mode {
        ContextMode::Standard => ContextMode::STANDARD_CONTEXT_WINDOW,
        ContextMode::OneM => ONE_M_CONTEXT_WINDOW,
    }
}

pub fn approx_token_count(text: &str) -> u64 {
    approx_tokens_from_byte_count(text.len())
}

pub fn build_summary_text(summary: &str) -> String {
    let trimmed = summary.trim();
    let suffix = if trimmed.is_empty() {
        "(no summary available)"
    } else {
        trimmed
    };
    format!("{SUMMARY_PREFIX}\n{suffix}")
}

pub fn build_compacted_history(user_messages: &[String], summary_text: &str) -> Vec<Value> {
    let mut selected_messages = Vec::new();
    let mut remaining = COMPACT_USER_MESSAGE_MAX_TOKENS;

    if remaining > 0 {
        for message in user_messages.iter().rev() {
            if remaining == 0 {
                break;
            }

            let tokens = approx_token_count(message) as usize;
            if tokens <= remaining {
                selected_messages.push(message.clone());
                remaining = remaining.saturating_sub(tokens);
            } else {
                let truncated = truncate_middle_with_token_budget(message, remaining);
                if !truncated.is_empty() {
                    selected_messages.push(truncated);
                }
                break;
            }
        }
    }

    selected_messages.reverse();

    let mut history = selected_messages
        .into_iter()
        .map(build_user_message_item)
        .collect::<Vec<_>>();
    history.push(build_user_message_item(if summary_text.is_empty() {
        "(no summary available)".to_string()
    } else {
        summary_text.to_string()
    }));
    history
}

pub fn append_compacted_rollout(
    rollout_path: &Path,
    summary_text: &str,
    replacement_history: &[Value],
) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(rollout_path)
        .with_context(|| format!("failed to open {}", rollout_path.display()))?;
    let line = json!({
        "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "type": "compacted",
        "payload": {
            "message": summary_text,
            "replacement_history": replacement_history,
        },
    });
    writeln!(
        file,
        "{}",
        serde_json::to_string(&line).context("failed to serialize compacted rollout item")?
    )
    .with_context(|| format!("failed to append {}", rollout_path.display()))?;
    Ok(())
}

pub fn read_rollout_snapshot(rollout_path: &Path) -> Result<RolloutSnapshot> {
    let file = File::open(rollout_path)
        .with_context(|| format!("failed to open {}", rollout_path.display()))?;
    let reader = BufReader::new(file);
    let mut snapshot = RolloutSnapshot::default();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        if let Some(message) = parse_user_message(&value) {
            snapshot.user_messages.push(message);
        }
    }

    Ok(snapshot)
}

fn parse_user_message(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("response_item") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    if payload.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let content = payload.get("content")?.as_array()?;
    let parts = content
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    let joined = parts.join("\n");
    (!is_summary_message(&joined)).then_some(joined)
}

fn is_summary_message(message: &str) -> bool {
    message.starts_with(SUMMARY_PREFIX)
        && message
            .as_bytes()
            .get(SUMMARY_PREFIX.len())
            .copied()
            .is_some_and(|value| value == b'\n')
}

fn build_user_message_item(text: String) -> Value {
    json!({
        "type": "message",
        "role": "user",
        "content": [
            {
                "type": "input_text",
                "text": text,
            }
        ],
        "end_turn": Value::Null,
        "phase": Value::Null,
    })
}

fn truncate_middle_with_token_budget(content: &str, max_tokens: usize) -> String {
    if content.is_empty() {
        return String::new();
    }
    if max_tokens > 0 && content.len() <= approx_bytes_for_tokens(max_tokens) {
        return content.to_string();
    }
    truncate_with_byte_estimate(content, approx_bytes_for_tokens(max_tokens), true)
}

fn truncate_with_byte_estimate(content: &str, max_bytes: usize, use_tokens: bool) -> String {
    if content.is_empty() {
        return String::new();
    }
    if max_bytes == 0 {
        return format_truncation_marker(
            use_tokens,
            removed_units(use_tokens, content.len(), content.chars().count()),
        );
    }
    if content.len() <= max_bytes {
        return content.to_string();
    }

    let total_bytes = content.len();
    let (left_budget, right_budget) = split_budget(max_bytes);
    let (removed_chars, left, right) = split_string(content, left_budget, right_budget);
    let marker = format_truncation_marker(
        use_tokens,
        removed_units(
            use_tokens,
            total_bytes.saturating_sub(max_bytes),
            removed_chars,
        ),
    );

    let mut out = String::with_capacity(left.len() + right.len() + marker.len());
    out.push_str(left);
    out.push_str(&marker);
    out.push_str(right);
    out
}

fn approx_bytes_for_tokens(tokens: usize) -> usize {
    tokens.saturating_mul(APPROX_BYTES_PER_TOKEN)
}

fn approx_tokens_from_byte_count(bytes: usize) -> u64 {
    let bytes_u64 = bytes as u64;
    bytes_u64.saturating_add(APPROX_BYTES_PER_TOKEN as u64 - 1) / APPROX_BYTES_PER_TOKEN as u64
}

fn split_budget(budget: usize) -> (usize, usize) {
    let left = budget / 2;
    (left, budget.saturating_sub(left))
}

fn split_string(content: &str, beginning_bytes: usize, end_bytes: usize) -> (usize, &str, &str) {
    if content.is_empty() {
        return (0, "", "");
    }

    let len = content.len();
    let tail_start_target = len.saturating_sub(end_bytes);
    let mut prefix_end = 0usize;
    let mut suffix_start = len;
    let mut removed_chars = 0usize;
    let mut suffix_started = false;

    for (idx, ch) in content.char_indices() {
        let char_end = idx + ch.len_utf8();
        if char_end <= beginning_bytes {
            prefix_end = char_end;
            continue;
        }
        if idx >= tail_start_target {
            if !suffix_started {
                suffix_start = idx;
                suffix_started = true;
            }
            continue;
        }
        removed_chars = removed_chars.saturating_add(1);
    }

    if suffix_start < prefix_end {
        suffix_start = prefix_end;
    }

    (
        removed_chars,
        &content[..prefix_end],
        &content[suffix_start..],
    )
}

fn format_truncation_marker(use_tokens: bool, removed_count: u64) -> String {
    if use_tokens {
        format!("...{removed_count} tokens truncated...")
    } else {
        format!("...{removed_count} chars truncated...")
    }
}

fn removed_units(use_tokens: bool, removed_bytes: usize, removed_chars: usize) -> u64 {
    if use_tokens {
        approx_tokens_from_byte_count(removed_bytes)
    } else {
        removed_chars as u64
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        SUMMARY_PREFIX, append_compacted_rollout, build_compacted_history, build_summary_text,
        context_mode_window, read_rollout_snapshot,
    };
    use crate::session::state::ContextMode;

    #[test]
    fn context_mode_window_uses_expected_sizes() {
        assert_eq!(context_mode_window(ContextMode::Standard), 272_000);
        assert_eq!(context_mode_window(ContextMode::OneM), 1_000_000);
    }

    #[test]
    fn rollout_snapshot_ignores_previous_summary_messages() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        fs::write(
            &path,
            format!(
                concat!(
                    "{{\"timestamp\":\"2026-04-20T12:00:00.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"first\"}}]}}}}\n",
                    "{{\"timestamp\":\"2026-04-20T12:01:00.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"{summary}\\nold summary\"}}]}}}}\n",
                    "{{\"timestamp\":\"2026-04-20T12:02:00.000Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":1000,\"cached_input_tokens\":200,\"output_tokens\":100,\"total_tokens\":1100}},\"last_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":20,\"output_tokens\":10,\"total_tokens\":130}},\"model_context_window\":272000}}}}}}\n"
                ),
                summary = SUMMARY_PREFIX,
            ),
        )
        .unwrap();

        let snapshot = read_rollout_snapshot(&path).unwrap();
        assert_eq!(snapshot.user_messages, vec!["first".to_string()]);
    }

    #[test]
    fn compacted_history_appends_summary_as_user_message() {
        let summary = build_summary_text("handoff");
        let history =
            build_compacted_history(&["first".to_string(), "second".to_string()], &summary);
        assert_eq!(history.len(), 3);
        assert_eq!(history[2]["role"], "user");
        assert_eq!(history[2]["content"][0]["text"], summary);
    }

    #[test]
    fn append_compacted_rollout_writes_compacted_item() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        append_compacted_rollout(
            &path,
            "summary",
            &[serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{"type":"input_text","text":"hello"}],
            })],
        )
        .unwrap();
        let raw = fs::read_to_string(path).unwrap();
        assert!(raw.contains("\"type\":\"compacted\""));
        assert!(raw.contains("\"message\":\"summary\""));
    }
}
