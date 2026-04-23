use std::path::Path;

use serde::Deserialize;

use crate::shadow::memory::ShadowContext;
use crate::skills::index::{SkillIndex, SkillMeta};
use crate::skills::writer::{build_skill_md, normalize_slug, write_new_skill};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SkillResponse {
    None,
    Create {
        name: String,
        description: String,
        body: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillApplyReport {
    pub created: Option<std::path::PathBuf>,
    pub skipped_none: bool,
    pub skipped_invalid_slug: bool,
    pub skipped_validation: Option<String>,
    pub write_error: Option<String>,
}

pub fn parse_skill_response(raw: &str) -> anyhow::Result<SkillResponse> {
    let cleaned = extract_json_block(raw);
    let parsed = serde_json::from_str::<SkillResponse>(&cleaned)?;
    Ok(parsed)
}

pub fn skill_threshold_met(ctx: &ShadowContext, cfg: &SkillShadowConfig) -> bool {
    ctx.modified_file_count >= cfg.files_threshold || ctx.tool_call_count >= cfg.tool_threshold
}

#[derive(Debug, Clone)]
pub struct SkillShadowConfig {
    pub files_threshold: usize,
    pub tool_threshold: usize,
}

impl Default for SkillShadowConfig {
    fn default() -> Self {
        Self {
            files_threshold: 2,
            tool_threshold: 5,
        }
    }
}

pub fn apply_skill_response(
    skills_root: &Path,
    index: &SkillIndex,
    response: &SkillResponse,
) -> SkillApplyReport {
    let mut report = SkillApplyReport::default();
    match response {
        SkillResponse::None => {
            report.skipped_none = true;
        }
        SkillResponse::Create {
            name,
            description,
            body,
        } => {
            let Some(slug) = normalize_slug(name) else {
                report.skipped_invalid_slug = true;
                return report;
            };
            let md = match build_skill_md(&slug, description, body) {
                Ok(md) => md,
                Err(err) => {
                    report.skipped_validation = Some(err.to_string());
                    return report;
                }
            };
            match write_new_skill(skills_root, &slug, &md) {
                Ok(path) => {
                    report.created = Some(path);
                    index.invalidate();
                }
                Err(err) => {
                    report.write_error = Some(err.to_string());
                }
            }
        }
    }
    report
}

pub fn existing_skill_hints(metas: &[SkillMeta]) -> Vec<(String, String)> {
    metas
        .iter()
        .map(|m| (m.name.clone(), m.description.clone()))
        .collect()
}

fn extract_json_block(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(fenced) = strip_fenced(trimmed) {
        return fenced.to_string();
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            return trimmed[start..=end].to_string();
        }
    }
    trimmed.to_string()
}

fn strip_fenced(s: &str) -> Option<&str> {
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
    fn parse_none_action() {
        let resp = parse_skill_response(r#"{"action":"none"}"#).unwrap();
        assert!(matches!(resp, SkillResponse::None));
    }

    #[test]
    fn parse_create_action() {
        let raw = r#"{"action":"create","name":"foo","description":"d","body":"b"}"#;
        let resp = parse_skill_response(raw).unwrap();
        match resp {
            SkillResponse::Create {
                name,
                description,
                body,
            } => {
                assert_eq!(name, "foo");
                assert_eq!(description, "d");
                assert_eq!(body, "b");
            }
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn parse_create_with_markdown_fence() {
        let raw = "```json\n{\"action\":\"create\",\"name\":\"f\",\"description\":\"d\",\"body\":\"b\"}\n```";
        let resp = parse_skill_response(raw).unwrap();
        assert!(matches!(resp, SkillResponse::Create { .. }));
    }

    #[test]
    fn parse_garbage_returns_error() {
        assert!(parse_skill_response("not json").is_err());
    }

    #[test]
    fn apply_none_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let index = SkillIndex::new(dir.path().to_path_buf());
        let report = apply_skill_response(dir.path(), &index, &SkillResponse::None);
        assert!(report.skipped_none);
        assert!(report.created.is_none());
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn apply_create_writes_claw_skill() {
        let dir = tempfile::tempdir().unwrap();
        let index = SkillIndex::new(dir.path().to_path_buf());
        let response = SkillResponse::Create {
            name: "My Shiny Skill".to_string(),
            description: "does a thing".to_string(),
            body: "# Body\nsteps".to_string(),
        };
        let report = apply_skill_response(dir.path(), &index, &response);
        let path = report.created.as_ref().expect("expected a file");
        assert_eq!(path.file_name().unwrap(), "SKILL.md");
        let parent = path.parent().unwrap().file_name().unwrap();
        assert_eq!(parent, "claw-my-shiny-skill");
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("name: my-shiny-skill"));
    }

    #[test]
    fn apply_create_with_invalid_slug_skips() {
        let dir = tempfile::tempdir().unwrap();
        let index = SkillIndex::new(dir.path().to_path_buf());
        let response = SkillResponse::Create {
            name: "!!!".to_string(),
            description: "d".to_string(),
            body: "b".to_string(),
        };
        let report = apply_skill_response(dir.path(), &index, &response);
        assert!(report.skipped_invalid_slug);
        assert!(report.created.is_none());
    }

    #[test]
    fn apply_create_with_invalid_description_skips() {
        let dir = tempfile::tempdir().unwrap();
        let index = SkillIndex::new(dir.path().to_path_buf());
        let long_desc = "x".repeat(200);
        let response = SkillResponse::Create {
            name: "valid".to_string(),
            description: long_desc,
            body: "b".to_string(),
        };
        let report = apply_skill_response(dir.path(), &index, &response);
        assert!(report.skipped_validation.is_some());
    }

    #[test]
    fn threshold_met_when_files_exceed_threshold() {
        let cfg = SkillShadowConfig::default();
        let ctx = ShadowContext {
            openid: "u".to_string(),
            last_user_text: "x".to_string(),
            last_assistant_text: "y".to_string(),
            tool_call_count: 0,
            modified_file_count: cfg.files_threshold,
        };
        assert!(skill_threshold_met(&ctx, &cfg));
    }

    #[test]
    fn threshold_not_met_for_simple_chat() {
        let cfg = SkillShadowConfig::default();
        let ctx = ShadowContext {
            openid: "u".to_string(),
            last_user_text: "x".to_string(),
            last_assistant_text: "y".to_string(),
            tool_call_count: 0,
            modified_file_count: 0,
        };
        assert!(!skill_threshold_met(&ctx, &cfg));
    }
}
