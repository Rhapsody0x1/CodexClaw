pub const SLUG_PREFIX: &str = "claw-";
pub const SLUG_MAX_CHARS: usize = 48;
pub const DESCRIPTION_MAX_CHARS: usize = 140;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SkillValidationError {
    #[error("name is empty or invalid (must match [a-z0-9-]+)")]
    InvalidName,
    #[error("description is empty")]
    EmptyDescription,
    #[error("description exceeds {limit} chars")]
    DescriptionTooLong { limit: usize },
    #[error("description must be a single line (no newlines)")]
    DescriptionMultiline,
    #[error("body is empty after trimming")]
    EmptyBody,
}

pub fn build_skill_md(
    name: &str,
    description: &str,
    body: &str,
) -> Result<String, SkillValidationError> {
    if name.is_empty() || !is_valid_slug(name) {
        return Err(SkillValidationError::InvalidName);
    }
    let desc_trimmed = description.trim();
    if desc_trimmed.is_empty() {
        return Err(SkillValidationError::EmptyDescription);
    }
    if desc_trimmed.contains('\n') {
        return Err(SkillValidationError::DescriptionMultiline);
    }
    if desc_trimmed.chars().count() > DESCRIPTION_MAX_CHARS {
        return Err(SkillValidationError::DescriptionTooLong {
            limit: DESCRIPTION_MAX_CHARS,
        });
    }
    let body_trimmed = body.trim();
    if body_trimmed.is_empty() {
        return Err(SkillValidationError::EmptyBody);
    }
    Ok(format!(
        "---\nname: {name}\ndescription: {desc_trimmed}\n---\n{body_trimmed}\n"
    ))
}

pub fn write_new_skill(
    skills_root: &std::path::Path,
    slug: &str,
    skill_md: &str,
) -> anyhow::Result<std::path::PathBuf> {
    use anyhow::{Context, anyhow};

    if !is_valid_slug(slug) {
        return Err(anyhow!("invalid slug for claw skill: {slug:?}"));
    }
    std::fs::create_dir_all(skills_root)
        .with_context(|| format!("failed to create skills root {}", skills_root.display()))?;

    let base_dirname = format!("{SLUG_PREFIX}{slug}");
    let mut candidate = base_dirname.clone();
    let mut version = 2;
    loop {
        let dir_path = skills_root.join(&candidate);
        if !dir_path.exists() {
            std::fs::create_dir_all(&dir_path)
                .with_context(|| format!("failed to create {}", dir_path.display()))?;
            let skill_path = dir_path.join("SKILL.md");
            std::fs::write(&skill_path, skill_md)
                .with_context(|| format!("failed to write {}", skill_path.display()))?;
            return Ok(skill_path);
        }
        candidate = format!("{base_dirname}-v{version}");
        version += 1;
        if version > 99 {
            return Err(anyhow!("too many version collisions for slug {slug}"));
        }
    }
}

fn is_valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

pub fn normalize_slug(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut last_hyphen = true;
    for ch in raw.chars() {
        let ch = ch.to_ascii_lowercase();
        let keep = ch.is_ascii_alphanumeric();
        if keep {
            out.push(ch);
            last_hyphen = false;
        } else if !last_hyphen {
            out.push('-');
            last_hyphen = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        return None;
    }
    let slug: String = trimmed.chars().take(SLUG_MAX_CHARS).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() { None } else { Some(slug) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lowercases_and_replaces_spaces() {
        assert_eq!(
            normalize_slug("My Cool Skill"),
            Some("my-cool-skill".to_string())
        );
    }

    #[test]
    fn normalize_replaces_non_alnum_with_hyphen() {
        assert_eq!(
            normalize_slug("build/remotion.video!"),
            Some("build-remotion-video".to_string())
        );
    }

    #[test]
    fn normalize_collapses_consecutive_hyphens_and_trims() {
        assert_eq!(
            normalize_slug("---foo---bar---"),
            Some("foo-bar".to_string())
        );
    }

    #[test]
    fn normalize_truncates_to_slug_max_chars() {
        let raw = "a".repeat(100);
        let slug = normalize_slug(&raw).unwrap();
        assert!(slug.chars().count() <= SLUG_MAX_CHARS);
    }

    #[test]
    fn normalize_empty_or_all_invalid_returns_none() {
        assert_eq!(normalize_slug(""), None);
        assert_eq!(normalize_slug("!!!"), None);
        assert_eq!(normalize_slug("   "), None);
    }

    #[test]
    fn normalize_keeps_digits_and_hyphens() {
        assert_eq!(
            normalize_slug("api-v2-migration"),
            Some("api-v2-migration".to_string())
        );
    }

    #[test]
    fn build_skill_md_emits_frontmatter_and_body() {
        let md = build_skill_md("my-slug", "one-line desc", "# Body\nstep 1").unwrap();
        assert!(md.starts_with("---\n"));
        assert!(md.contains("name: my-slug\n"));
        assert!(md.contains("description: one-line desc\n"));
        assert!(md.contains("\n---\n"));
        assert!(md.trim_end().ends_with("step 1"));
    }

    #[test]
    fn build_skill_md_rejects_name_with_invalid_chars() {
        assert!(build_skill_md("has spaces", "d", "b").is_err());
        assert!(build_skill_md("UPPER", "d", "b").is_err());
        assert!(build_skill_md("has/slash", "d", "b").is_err());
    }

    #[test]
    fn build_skill_md_rejects_empty_name_or_description_or_body() {
        assert!(build_skill_md("", "d", "b").is_err());
        assert!(build_skill_md("name", "", "b").is_err());
        assert!(build_skill_md("name", "d", "  \n  ").is_err());
    }

    #[test]
    fn build_skill_md_rejects_description_over_140_chars() {
        let desc = "x".repeat(141);
        assert!(build_skill_md("name", &desc, "b").is_err());
    }

    #[test]
    fn build_skill_md_rejects_description_containing_newline() {
        assert!(build_skill_md("name", "line1\nline2", "b").is_err());
    }

    #[test]
    fn write_new_skill_creates_claw_prefixed_dir_with_skill_md() {
        let dir = tempfile::tempdir().unwrap();
        let md = build_skill_md("my-skill", "desc", "body").unwrap();
        let path = write_new_skill(dir.path(), "my-skill", &md).unwrap();
        assert_eq!(path, dir.path().join("claw-my-skill").join("SKILL.md"));
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, md);
    }

    #[test]
    fn write_new_skill_appends_version_suffix_on_collision() {
        let dir = tempfile::tempdir().unwrap();
        let md = build_skill_md("dup", "d", "b").unwrap();
        let p1 = write_new_skill(dir.path(), "dup", &md).unwrap();
        let p2 = write_new_skill(dir.path(), "dup", &md).unwrap();
        let p3 = write_new_skill(dir.path(), "dup", &md).unwrap();
        assert_eq!(p1, dir.path().join("claw-dup").join("SKILL.md"));
        assert_eq!(p2, dir.path().join("claw-dup-v2").join("SKILL.md"));
        assert_eq!(p3, dir.path().join("claw-dup-v3").join("SKILL.md"));
    }

    #[test]
    fn write_new_skill_rejects_slug_with_invalid_chars_for_safety() {
        let dir = tempfile::tempdir().unwrap();
        let md = "anything";
        assert!(write_new_skill(dir.path(), "../escape", md).is_err());
        assert!(write_new_skill(dir.path(), "has spaces", md).is_err());
        assert!(write_new_skill(dir.path(), "", md).is_err());
    }
}
