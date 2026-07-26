//! Parsing of `codex exec --json` NDJSON output.
//!
//! Both the scheduler runner (which buffers the whole stdout pipe) and the
//! shadow runner (which streams lines) need the same extraction: decode each
//! line as a [`CodexEvent`], keep the completed `agent_message` items, and
//! join their text.

use crate::codex::CodexEvent;

/// Concatenate the text of every completed `agent_message` item, one per line.
///
/// Lines that are blank, not valid JSON, not an `item.completed` event, not an
/// `agent_message`, or carry no `text` field are skipped.
pub fn agent_messages_from_lines<I, S>(lines: I) -> String
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

/// [`agent_messages_from_lines`] over a raw stdout buffer (lossy UTF-8).
pub fn agent_messages_from_stdout(stdout: &[u8]) -> String {
    agent_messages_from_lines(String::from_utf8_lossy(stdout).lines())
}
