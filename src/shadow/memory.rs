use serde::Deserialize;

use crate::memory::store::{AddState, MemoryKind, MemoryStore};
use crate::util::text::extract_json_block;

#[derive(Debug, Clone)]
pub(crate) struct ShadowContext {
    pub(crate) openid: String,
    pub(crate) last_user_text: String,
    pub(crate) last_assistant_text: String,
    pub(crate) tool_call_count: usize,
    pub(crate) modified_file_count: usize,
}

#[derive(Debug, Clone)]
pub struct ShadowConfig {
    pub min_user_msg_chars: usize,
    pub model_override: Option<String>,
    pub reasoning: String,
    pub deadline: std::time::Duration,
}

impl Default for ShadowConfig {
    fn default() -> Self {
        Self {
            min_user_msg_chars: 40,
            model_override: None,
            reasoning: "low".to_string(),
            deadline: std::time::Duration::from_secs(120),
        }
    }
}

pub(crate) fn memory_threshold_met(ctx: &ShadowContext, cfg: &ShadowConfig) -> bool {
    ctx.tool_call_count >= 1
        || ctx.modified_file_count >= 1
        || ctx.last_user_text.chars().count() >= cfg.min_user_msg_chars
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ApplyReport {
    pub(crate) added: usize,
    pub(crate) duplicate: usize,
    pub(crate) rejected: usize,
    pub(crate) over_budget: usize,
    pub(crate) too_long: usize,
    pub(crate) empty: usize,
    pub(crate) non_add_actions: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct DistillEntry {
    pub(crate) action: String,
    pub(crate) content: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct DistillResponse {
    #[serde(default)]
    pub(crate) memory: Vec<DistillEntry>,
    #[serde(default)]
    pub(crate) user: Vec<DistillEntry>,
}

pub(crate) fn parse_memory_response(raw: &str) -> anyhow::Result<DistillResponse> {
    let cleaned = extract_json_block(raw);
    let parsed = serde_json::from_str::<DistillResponse>(&cleaned)?;
    Ok(parsed)
}

pub(crate) fn apply_memory_response(
    store: &MemoryStore,
    openid: &str,
    response: &DistillResponse,
) -> anyhow::Result<ApplyReport> {
    let mut report = ApplyReport::default();
    for entry in &response.memory {
        apply_entry(store, openid, MemoryKind::Memory, entry, &mut report)?;
    }
    for entry in &response.user {
        apply_entry(store, openid, MemoryKind::User, entry, &mut report)?;
    }
    Ok(report)
}

fn apply_entry(
    store: &MemoryStore,
    openid: &str,
    kind: MemoryKind,
    entry: &DistillEntry,
    report: &mut ApplyReport,
) -> anyhow::Result<()> {
    if entry.action != "add" {
        report.non_add_actions += 1;
        return Ok(());
    }
    let outcome = store.add(openid, kind, &entry.content)?;
    match outcome.state {
        AddState::Added => report.added += 1,
        AddState::Duplicate => report.duplicate += 1,
        AddState::Rejected(_) => report.rejected += 1,
        AddState::OverBudget => report.over_budget += 1,
        AddState::EntryTooLong => report.too_long += 1,
        AddState::EntryEmpty => report.empty += 1,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_json_object() {
        let raw = r#"{"memory":[{"action":"add","content":"x"}],"user":[]}"#;
        let parsed = parse_memory_response(raw).unwrap();
        assert_eq!(parsed.memory.len(), 1);
        assert_eq!(parsed.memory[0].action, "add");
        assert_eq!(parsed.memory[0].content, "x");
        assert!(parsed.user.is_empty());
    }

    #[test]
    fn parse_missing_keys_uses_defaults() {
        let parsed = parse_memory_response("{}").unwrap();
        assert!(parsed.memory.is_empty());
        assert!(parsed.user.is_empty());
    }

    #[test]
    fn parse_json_wrapped_in_markdown_fence() {
        let raw = "```json\n{\"memory\":[{\"action\":\"add\",\"content\":\"y\"}],\"user\":[]}\n```";
        let parsed = parse_memory_response(raw).unwrap();
        assert_eq!(parsed.memory[0].content, "y");
    }

    #[test]
    fn parse_json_wrapped_in_bare_fence() {
        let raw = "```\n{\"memory\":[],\"user\":[{\"action\":\"add\",\"content\":\"z\"}]}\n```";
        let parsed = parse_memory_response(raw).unwrap();
        assert_eq!(parsed.user[0].content, "z");
    }

    #[test]
    fn parse_json_with_surrounding_prose() {
        let raw = "Here is the JSON you asked for:\n{\"memory\":[{\"action\":\"add\",\"content\":\"q\"}],\"user\":[]}\nhope this helps";
        let parsed = parse_memory_response(raw).unwrap();
        assert_eq!(parsed.memory[0].content, "q");
    }

    #[test]
    fn parse_garbage_returns_error() {
        assert!(parse_memory_response("not json at all").is_err());
    }

    #[test]
    fn parse_ignores_unknown_top_level_keys() {
        let raw = r#"{"memory":[],"user":[],"extra":"ignored"}"#;
        let parsed = parse_memory_response(raw).unwrap();
        assert!(parsed.memory.is_empty());
        assert!(parsed.user.is_empty());
    }

    #[test]
    fn apply_response_writes_add_entries_to_memory_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());
        let response = DistillResponse {
            memory: vec![DistillEntry {
                action: "add".to_string(),
                content: "prefers pnpm".to_string(),
            }],
            user: vec![DistillEntry {
                action: "add".to_string(),
                content: "name: XiaoMing".to_string(),
            }],
        };
        let report = apply_memory_response(&store, "u", &response).unwrap();
        assert_eq!(report.added, 2);
        let snap = store.snapshot_for("u").unwrap();
        assert_eq!(snap.memory, vec!["prefers pnpm".to_string()]);
        assert_eq!(snap.user, vec!["name: XiaoMing".to_string()]);
    }

    #[test]
    fn apply_response_counts_non_add_actions_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());
        let response = DistillResponse {
            memory: vec![DistillEntry {
                action: "remove".to_string(),
                content: "whatever".to_string(),
            }],
            user: vec![],
        };
        let report = apply_memory_response(&store, "u", &response).unwrap();
        assert_eq!(report.non_add_actions, 1);
        assert_eq!(report.added, 0);
        let snap = store.snapshot_for("u").unwrap();
        assert!(snap.memory.is_empty());
    }

    fn ctx(user: &str, tool: usize, files: usize) -> ShadowContext {
        ShadowContext {
            openid: "u".to_string(),
            last_user_text: user.to_string(),
            last_assistant_text: "a".to_string(),
            tool_call_count: tool,
            modified_file_count: files,
        }
    }

    #[test]
    fn memory_threshold_fires_on_any_signal() {
        let cfg = ShadowConfig::default();
        let at_min = "x".repeat(cfg.min_user_msg_chars);
        let below_min = "x".repeat(cfg.min_user_msg_chars - 1);
        let cases: &[(&str, &str, usize, usize, bool)] = &[
            ("user message exactly at min chars", &at_min, 0, 0, true),
            ("tool call count positive", "hi", 1, 0, true),
            ("files modified", "hi", 0, 1, true),
            ("user message one char below min", &below_min, 0, 0, false),
            ("trivial turn", "hi", 0, 0, false),
        ];
        for (case, user, tool, files, expected) in cases {
            assert_eq!(
                memory_threshold_met(&ctx(user, *tool, *files), &cfg),
                *expected,
                "case: {case}"
            );
        }
    }

    #[test]
    fn apply_response_counts_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path().to_path_buf());
        store
            .add(
                "u",
                crate::memory::store::MemoryKind::Memory,
                "prefers pnpm",
            )
            .unwrap();
        let response = DistillResponse {
            memory: vec![DistillEntry {
                action: "add".to_string(),
                content: "prefers pnpm".to_string(),
            }],
            user: vec![],
        };
        let report = apply_memory_response(&store, "u", &response).unwrap();
        assert_eq!(report.duplicate, 1);
        assert_eq!(report.added, 0);
    }
}
