use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::RwLock,
};

use crate::skills::writer::SLUG_PREFIX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMeta {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
}

pub struct SkillIndex {
    root: PathBuf,
    cache: RwLock<Option<Vec<SkillMeta>>>,
}

impl SkillIndex {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            cache: RwLock::new(None),
        }
    }

    pub fn invalidate(&self) {
        *self.cache.write().expect("skill cache poisoned") = None;
    }

    pub fn list_claw(&self) -> anyhow::Result<Vec<SkillMeta>> {
        if let Some(hit) = self.cache.read().expect("skill cache poisoned").as_ref() {
            return Ok(hit.clone());
        }
        let scanned = scan_claw_skills(&self.root)?;
        *self.cache.write().expect("skill cache poisoned") = Some(scanned.clone());
        Ok(scanned)
    }
}

pub fn parse_frontmatter(md: &str) -> Option<HashMap<String, String>> {
    let rest = md.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let body = &rest[..end];
    let mut out = HashMap::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (k, v) = line.split_once(':')?;
        let key = k.trim().to_string();
        let value = v.trim().trim_matches('"').trim_matches('\'').to_string();
        out.insert(key, value);
    }
    Some(out)
}

pub fn scan_claw_skills(root: &Path) -> anyhow::Result<Vec<SkillMeta>> {
    let mut out = Vec::new();
    let read = match std::fs::read_dir(root) {
        Ok(r) => r,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(err) => return Err(err.into()),
    };
    for entry in read {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = dir_name.strip_prefix(SLUG_PREFIX) else {
            continue;
        };
        let skill_md = entry.path().join("SKILL.md");
        let Ok(content) = std::fs::read_to_string(&skill_md) else {
            continue;
        };
        let Some(fm) = parse_frontmatter(&content) else {
            continue;
        };
        let name = fm.get("name").cloned().unwrap_or_else(|| rest.to_string());
        let description = fm.get("description").cloned().unwrap_or_default();
        out.push(SkillMeta {
            slug: rest.to_string(),
            name,
            description,
            dir: entry.path(),
        });
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, slug: &str, name: &str, desc: &str) {
        let dir = root.join(format!("{SLUG_PREFIX}{slug}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\nbody\n"),
        )
        .unwrap();
    }

    #[test]
    fn parse_frontmatter_reads_name_and_description() {
        let md = "---\nname: foo\ndescription: bar baz\n---\n# body\nhello";
        let fm = parse_frontmatter(md).unwrap();
        assert_eq!(fm.get("name"), Some(&"foo".to_string()));
        assert_eq!(fm.get("description"), Some(&"bar baz".to_string()));
    }

    #[test]
    fn parse_frontmatter_without_opening_marker_returns_none() {
        assert!(parse_frontmatter("no frontmatter here").is_none());
    }

    #[test]
    fn parse_frontmatter_strips_surrounding_quotes() {
        let md = "---\nname: \"quoted\"\ndescription: 'single'\n---\nbody";
        let fm = parse_frontmatter(md).unwrap();
        assert_eq!(fm.get("name"), Some(&"quoted".to_string()));
        assert_eq!(fm.get("description"), Some(&"single".to_string()));
    }

    #[test]
    fn scan_on_missing_dir_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let out = scan_claw_skills(&dir.path().join("no-such-dir")).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn scan_finds_only_claw_prefixed_dirs() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "alpha", "alpha-name", "alpha-desc");
        write_skill(dir.path(), "bravo", "bravo-name", "bravo-desc");
        std::fs::create_dir_all(dir.path().join("system-skill")).unwrap();
        std::fs::write(
            dir.path().join("system-skill").join("SKILL.md"),
            "---\nname: sys\ndescription: d\n---\nb",
        )
        .unwrap();
        let skills = scan_claw_skills(dir.path()).unwrap();
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].slug, "alpha");
        assert_eq!(skills[0].name, "alpha-name");
        assert_eq!(skills[1].slug, "bravo");
    }

    #[test]
    fn scan_skips_claw_dirs_without_skill_md() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("claw-orphan")).unwrap();
        write_skill(dir.path(), "good", "g", "d");
        let skills = scan_claw_skills(dir.path()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].slug, "good");
    }

    #[test]
    fn index_list_claws_caches_after_first_scan() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "cached", "c", "d");
        let idx = SkillIndex::new(dir.path().to_path_buf());
        let first = idx.list_claw().unwrap();
        // Mutate disk; cached list should NOT observe the change.
        write_skill(dir.path(), "new-one", "n", "d");
        let second = idx.list_claw().unwrap();
        assert_eq!(first, second);
        // After invalidate, the new entry appears.
        idx.invalidate();
        let third = idx.list_claw().unwrap();
        assert_eq!(third.len(), 2);
    }
}
