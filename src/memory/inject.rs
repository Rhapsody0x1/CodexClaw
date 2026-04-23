use crate::memory::store::Snapshot;

pub fn render(snapshot: &Snapshot) -> Option<String> {
    if snapshot.memory.is_empty() && snapshot.user.is_empty() {
        return None;
    }
    let mut out = String::from("<persistent-memory>\n");
    out.push_str("(跨会话记忆，供参考；与当前任务无关的请忽略。)\n");
    if !snapshot.user.is_empty() {
        out.push_str("\n## User\n");
        for entry in &snapshot.user {
            out.push_str("- ");
            out.push_str(entry);
            out.push('\n');
        }
    }
    if !snapshot.memory.is_empty() {
        out.push_str("\n## Memory\n");
        for entry in &snapshot.memory {
            out.push_str("- ");
            out.push_str(entry);
            out.push('\n');
        }
    }
    out.push_str("</persistent-memory>");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_renders_to_none() {
        let snap = Snapshot::default();
        assert!(render(&snap).is_none());
    }

    #[test]
    fn memory_only_includes_memory_section_without_user_section() {
        let snap = Snapshot {
            memory: vec!["prefers pnpm".to_string(), "uses zsh".to_string()],
            user: vec![],
        };
        let rendered = render(&snap).unwrap();
        assert!(rendered.contains("prefers pnpm"));
        assert!(rendered.contains("uses zsh"));
        assert!(rendered.contains("Memory"));
        assert!(!rendered.contains("## User"));
    }

    #[test]
    fn user_only_includes_user_section_without_memory_section() {
        let snap = Snapshot {
            memory: vec![],
            user: vec!["name: XiaoMing".to_string()],
        };
        let rendered = render(&snap).unwrap();
        assert!(rendered.contains("name: XiaoMing"));
        assert!(rendered.contains("User"));
        assert!(!rendered.contains("## Memory"));
    }

    #[test]
    fn both_kinds_include_both_sections() {
        let snap = Snapshot {
            memory: vec!["mem-entry".to_string()],
            user: vec!["user-entry".to_string()],
        };
        let rendered = render(&snap).unwrap();
        assert!(rendered.contains("mem-entry"));
        assert!(rendered.contains("user-entry"));
        assert!(rendered.contains("## Memory"));
        assert!(rendered.contains("## User"));
    }

    #[test]
    fn rendered_block_is_wrapped_in_persistent_memory_tag() {
        let snap = Snapshot {
            memory: vec!["x".to_string()],
            user: vec![],
        };
        let rendered = render(&snap).unwrap();
        assert!(rendered.starts_with("<persistent-memory>"));
        assert!(rendered.trim_end().ends_with("</persistent-memory>"));
    }
}
