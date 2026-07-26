//! Parsing of `codex exec --json` NDJSON output.
//!
//! Both one-shot callers — the scheduler runner and the shadow runner — get
//! their stdout as lines from `exec_cli` and need the same extraction: decode
//! each line as a [`CodexEvent`], keep the completed `agent_message` items,
//! and join their text.

use crate::codex::CodexEvent;

/// Concatenate the text of every completed `agent_message` item, one per line.
///
/// Lines that are blank, not valid JSON, not an `item.completed` event, not an
/// `agent_message`, or carry no `text` field are skipped.
pub(crate) fn agent_messages_from_lines<I, S>(lines: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut parts = Vec::new();
    for line in lines {
        let trimmed = line.as_ref().trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<CodexEvent>(trimmed) else {
            continue;
        };
        if let CodexEvent::ItemCompleted { item } = event
            && item.item_type == "agent_message"
            && let Some(text) = item.text
        {
            parts.push(text);
        }
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::agent_messages_from_lines;

    #[test]
    fn concatenates_agent_text_items_in_order() {
        let lines = vec![
            r#"{"type":"thread.started","thread_id":"x"}"#,
            r#"{"type":"item.completed","item":{"id":"a","type":"agent_message","text":"first"}}"#,
            r#"{"type":"item.completed","item":{"id":"b","type":"agent_message","text":"second"}}"#,
            r#"{"type":"turn.completed"}"#,
        ];
        assert_eq!(agent_messages_from_lines(lines), "first\nsecond");
    }

    #[test]
    fn skips_non_agent_items() {
        let lines = vec![
            r#"{"type":"item.completed","item":{"id":"a","type":"tool_call","text":"ignored"}}"#,
            r#"{"type":"item.completed","item":{"id":"b","type":"agent_message","text":"kept"}}"#,
        ];
        assert_eq!(agent_messages_from_lines(lines), "kept");
    }

    #[test]
    fn ignores_blank_and_invalid_json_lines() {
        let lines = vec![
            "not json",
            "",
            "   ",
            r#"{"type":"item.completed","item":{"id":"a","type":"agent_message","text":"ok"}}"#,
        ];
        assert_eq!(agent_messages_from_lines(lines), "ok");
    }

    #[test]
    fn empty_when_no_agent_message_completed() {
        let lines = vec![r#"{"type":"thread.started","thread_id":"x"}"#];
        assert_eq!(agent_messages_from_lines(lines), "");
    }
}
