//! String shaping helpers: truncation, JSON extraction, label formatting.

/// Keep at most `max_chars` characters and append `marker` only when something
/// was actually dropped. Callers pass their own marker because the existing
/// call sites differ (`"..."`, `" ..."`), and those strings are user-visible.
pub(crate) fn truncate_with_marker(value: &str, max_chars: usize, marker: &str) -> String {
    let mut chars = value.chars();
    let head = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{head}{marker}")
    } else {
        head
    }
}

/// Keep the first and last `max_chars / 2` characters, replacing the middle
/// with a note about how much was dropped. Used for log bodies, where both the
/// beginning (what ran) and the end (how it failed) matter.
pub(crate) fn truncate_middle(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }
    let half = max_chars / 2;
    let head = value.chars().take(half).collect::<String>();
    let tail = value
        .chars()
        .rev()
        .take(half)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!(
        "{head}\n\n[truncated {} chars]\n\n{tail}",
        count - max_chars
    )
}

/// Compact single-line JSON, with `null` rendering as the empty string so
/// callers can treat "no payload" as "nothing to show".
pub(crate) fn short_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

/// Turn a snake_case tool name into a Title Case label (`file_search` ->
/// `File Search`).
pub(crate) fn humanize_tool_label(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut text = first.to_uppercase().collect::<String>();
                    text.push_str(chars.as_str());
                    text
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pull a JSON object out of a model reply that may be fenced, prefixed with
/// prose, or already bare. Falls back to the trimmed input so the caller's own
/// parse error stays the one the user sees.
pub(crate) fn extract_json_block(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(fenced) = strip_fenced(trimmed) {
        return fenced.to_string();
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
    {
        return trimmed[start..=end].to_string();
    }
    trimmed.to_string()
}

/// Strip a leading ```` ```json ```` / ```` ``` ```` fence and its closing
/// fence, returning the trimmed body.
pub(crate) fn strip_fenced(s: &str) -> Option<&str> {
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))?;
    let s = s.trim_start_matches(char::is_whitespace);
    let end = s.rfind("```")?;
    Some(s[..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_with_marker_only_marks_when_it_cuts() {
        for (input, max, marker, expected) in [
            ("hello", 5, "...", "hello"),
            ("hello", 10, "...", "hello"),
            ("hello", 4, "...", "hell..."),
            ("hello", 4, " ...", "hell ..."),
            ("", 0, "...", ""),
            ("中文很长的一句话", 2, "...", "中文..."),
        ] {
            assert_eq!(
                truncate_with_marker(input, max, marker),
                expected,
                "input={input:?} max={max} marker={marker:?}"
            );
        }
    }

    #[test]
    fn truncate_middle_keeps_both_ends() {
        assert_eq!(truncate_middle("abcdef", 6), "abcdef");
        assert_eq!(
            truncate_middle("abcdefgh", 4),
            "ab\n\n[truncated 4 chars]\n\ngh"
        );
    }

    #[test]
    fn short_json_renders_null_as_empty() {
        assert_eq!(short_json(&serde_json::Value::Null), "");
        assert_eq!(short_json(&serde_json::json!({"a": 1})), "{\"a\":1}");
    }

    #[test]
    fn humanize_tool_label_title_cases_segments() {
        assert_eq!(humanize_tool_label("file_search"), "File Search");
        assert_eq!(humanize_tool_label("__weird__name__"), "Weird Name");
        assert_eq!(humanize_tool_label(""), "");
    }

    #[test]
    fn extract_json_block_handles_fenced_bare_and_prose() {
        assert_eq!(extract_json_block("  {\"a\":1}  "), "{\"a\":1}");
        assert_eq!(extract_json_block("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(extract_json_block("```\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(
            extract_json_block("here you go:\n{\"a\":1}\nbye"),
            "{\"a\":1}"
        );
        assert_eq!(extract_json_block("not json"), "not json");
    }

    #[test]
    fn strip_fenced_requires_both_fences() {
        assert_eq!(strip_fenced("```json\n{}\n```"), Some("{}"));
        assert_eq!(strip_fenced("```\n{}\n```"), Some("{}"));
        assert_eq!(strip_fenced("```json\n{}"), None);
        assert_eq!(strip_fenced("{}"), None);
    }
}
