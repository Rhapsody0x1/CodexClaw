pub const MEMORY_SUMMARY_PREFIX_CHARS: usize = 60;

pub const SKILL_DISTILL_PROMPT: &str = "\
你是 codex-claw 的 Skill 蒸馏助手。下面是最近一个 turn 的对话，目标是判断是否值得为以后类似任务固化一个 Skill。\n\
\n\
**只产出 JSON**，schema：\n\
{\n\
  \"action\": \"none\" | \"create\",\n\
  \"name\": \"短横线命名\",\n\
  \"description\": \"一句话描述，≤ 140 字符\",\n\
  \"body\": \"Markdown 正文（操作手册风格）\"\n\
}\n\
\n\
创建门槛：\n\
- 任务是多步、可复用、在未来大概率重复出现的工作流。\n\
- 一次性的调试、沟通、琐碎问答 → action=none。\n\
- 正文要像操作手册：触发条件、步骤、易错点，不要复述对话。\n\
- 如果与已有 claw-skill 高度重合，action=none。\n\
\n\
已有 claw-skill 列表（若类似，直接 action=none）：\n\
<existing>\n\
{existing_claw_skills}\n\
</existing>\n\
\n\
<last_turn>\n\
<user>{last_user}</user>\n\
<assistant>{last_assistant}</assistant>\n\
</last_turn>\n\
";

pub fn render_skill_prompt(
    existing_claw_skills: &[(String, String)],
    last_user: &str,
    last_assistant: &str,
) -> String {
    let existing = if existing_claw_skills.is_empty() {
        "(none)".to_string()
    } else {
        existing_claw_skills
            .iter()
            .map(|(n, d)| format!("- {n}: {d}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    SKILL_DISTILL_PROMPT
        .replace("{existing_claw_skills}", &existing)
        .replace("{last_user}", last_user.trim())
        .replace("{last_assistant}", last_assistant.trim())
}

pub const MEMORY_DISTILL_PROMPT: &str = "\
你是 codex-claw 的记忆蒸馏助手。刚刚结束了一个 turn，下面给你最近一段对话与当前已有记忆摘要。\n\
\n\
**只产出 JSON**，schema：\n\
{\n\
  \"memory\": [ { \"action\": \"add\", \"content\": \"...\" } ],\n\
  \"user\":   [ { \"action\": \"add\", \"content\": \"...\" } ]\n\
}\n\
\n\
写入准则：\n\
- MEMORY 放：用户纠正过的点、稳定的环境事实、工作流习惯、工具使用偏好。\n\
- USER 放：称呼、身份、长期偏好、沟通风格、工作语言。\n\
- 不要写：当前任务进度、临时 TODO、会话特定结果、时间敏感信息、shell 输出或路径列表。\n\
- 如果这轮没值得沉淀的，返回空数组；宁缺毋滥。\n\
- 每条 ≤ 160 字符。\n\
\n\
<already_memory>\n\
{existing_memory}\n\
</already_memory>\n\
<already_user>\n\
{existing_user}\n\
</already_user>\n\
<last_turn>\n\
<user>{last_user}</user>\n\
<assistant>{last_assistant}</assistant>\n\
</last_turn>\n\
";

pub fn render_memory_prompt(
    existing_memory: &[String],
    existing_user: &[String],
    last_user: &str,
    last_assistant: &str,
) -> String {
    MEMORY_DISTILL_PROMPT
        .replace("{existing_memory}", &summarize_entries(existing_memory))
        .replace("{existing_user}", &summarize_entries(existing_user))
        .replace("{last_user}", last_user.trim())
        .replace("{last_assistant}", last_assistant.trim())
}

fn summarize_entries(entries: &[String]) -> String {
    if entries.is_empty() {
        return "(none)".to_string();
    }
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let prefix: String = e.chars().take(MEMORY_SUMMARY_PREFIX_CHARS).collect();
            format!("{}. {}", i + 1, prefix)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_memory_prompt_substitutes_all_placeholders() {
        let out = render_memory_prompt(
            &["mem-1".to_string()],
            &["user-1".to_string()],
            "hello",
            "hi",
        );
        assert!(!out.contains("{existing_memory}"));
        assert!(!out.contains("{existing_user}"));
        assert!(!out.contains("{last_user}"));
        assert!(!out.contains("{last_assistant}"));
        assert!(out.contains("mem-1"));
        assert!(out.contains("user-1"));
        assert!(out.contains("hello"));
        assert!(out.contains("hi"));
    }

    #[test]
    fn render_memory_prompt_empty_existing_shows_none_marker() {
        let out = render_memory_prompt(&[], &[], "u", "a");
        assert!(out.contains("(none)"));
    }

    #[test]
    fn summarize_entries_truncates_to_prefix_length() {
        let long = "x".repeat(200);
        let s = summarize_entries(&[long]);
        assert!(s.chars().count() < 100);
    }

    #[test]
    fn summarize_entries_numbers_entries_for_llm_dedup() {
        let s = summarize_entries(&["a".to_string(), "b".to_string()]);
        assert!(s.contains("1. a"));
        assert!(s.contains("2. b"));
    }

    #[test]
    fn render_skill_prompt_substitutes_all_placeholders() {
        let existing = vec![("foo".to_string(), "bar".to_string())];
        let out = render_skill_prompt(&existing, "question", "answer");
        assert!(!out.contains("{existing_claw_skills}"));
        assert!(!out.contains("{last_user}"));
        assert!(!out.contains("{last_assistant}"));
        assert!(out.contains("foo: bar"));
        assert!(out.contains("question"));
        assert!(out.contains("answer"));
    }

    #[test]
    fn render_skill_prompt_empty_existing_shows_none_marker() {
        let out = render_skill_prompt(&[], "u", "a");
        assert!(out.contains("(none)"));
    }
}
