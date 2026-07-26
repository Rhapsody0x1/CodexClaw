//! The `codex exec --json` NDJSON wire format — reduced to the part this
//! crate consumes.
//!
//! `codex exec` streams many event kinds, but the one-shot callers (cron
//! `CodexExec` jobs and the shadow distiller, via `exec_output`) only ever
//! read the text of completed `agent_message` items. Everything else lands in
//! [`CodexEvent::Unknown`] and serde skips unknown fields inside the item, so
//! upstream schema growth cannot break parsing. The interactive path speaks
//! the app-server protocol (`app_server/protocol.rs`) instead and never sees
//! these events; the display vocabulary lives in `codex::display`.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum CodexEvent {
    #[serde(rename = "item.completed")]
    ItemCompleted { item: ExecItem },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ExecItem {
    #[serde(rename = "type")]
    pub(crate) item_type: String,
    #[serde(default)]
    pub(crate) text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_completed_agent_message_and_tolerates_everything_else() {
        let event: CodexEvent = serde_json::from_str(
            r#"{"type":"item.completed","item":{"id":"a","type":"agent_message","text":"hi","unknown_field":1}}"#,
        )
        .unwrap();
        let CodexEvent::ItemCompleted { item } = event else {
            panic!("expected ItemCompleted");
        };
        assert_eq!(item.item_type, "agent_message");
        assert_eq!(item.text.as_deref(), Some("hi"));

        let other: CodexEvent =
            serde_json::from_str(r#"{"type":"turn.completed","usage":{}}"#).unwrap();
        assert!(matches!(other, CodexEvent::Unknown));
    }
}
