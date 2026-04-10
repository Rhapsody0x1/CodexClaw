use std::path::{Path, PathBuf};

use shlex::split;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedOutput {
    pub text: String,
    pub directives: Vec<Directive>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Directive {
    Image { path: PathBuf },
    File { path: PathBuf, name: Option<String> },
}

pub fn parse_output(raw: &str, workspace_dir: &Path) -> ParsedOutput {
    let Some(start) = raw.rfind("```qqbot") else {
        return ParsedOutput {
            text: raw.trim().to_string(),
            directives: Vec::new(),
        };
    };
    let prefix = &raw[..start];
    let remainder = &raw[start + "```qqbot".len()..];
    let Some(end) = remainder.find("```") else {
        return ParsedOutput {
            text: raw.trim().to_string(),
            directives: Vec::new(),
        };
    };
    let block = &remainder[..end];
    let directives = block
        .lines()
        .filter_map(|line| parse_directive_line(line.trim(), workspace_dir))
        .collect();
    ParsedOutput {
        text: prefix.trim().to_string(),
        directives,
    }
}

fn parse_directive_line(line: &str, workspace_dir: &Path) -> Option<Directive> {
    if line.is_empty() {
        return None;
    }
    let parts = split(line)?;
    let command = parts.first()?.as_str();
    let mut path = None;
    let mut name = None;
    for part in parts.iter().skip(1) {
        let (key, value) = part.split_once('=')?;
        match key {
            "path" => path = Some(resolve_path(value, workspace_dir)),
            "name" => name = Some(value.to_string()),
            _ => {}
        }
    }
    let path = path?;
    match command {
        "image" => Some(Directive::Image { path }),
        "file" => Some(Directive::File { path, name }),
        _ => None,
    }
}

fn resolve_path(value: &str, workspace_dir: &Path) -> PathBuf {
    let candidate = PathBuf::from(value);
    if candidate.is_absolute() {
        candidate
    } else {
        workspace_dir.join(candidate)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Directive, parse_output};

    #[test]
    fn parses_directive_block() {
        let output = parse_output(
            "hello\n```qqbot\nimage path=foo.png\nfile path=bar.txt name=report.txt\n```",
            Path::new("/tmp/workspace"),
        );
        assert_eq!(output.text, "hello");
        assert_eq!(
            output.directives,
            vec![
                Directive::Image {
                    path: "/tmp/workspace/foo.png".into()
                },
                Directive::File {
                    path: "/tmp/workspace/bar.txt".into(),
                    name: Some("report.txt".into())
                }
            ]
        );
    }
}
