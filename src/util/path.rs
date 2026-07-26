//! Path helpers: home resolution and executable search paths.

use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The user's home directory, falling back to `/root` — the container image the
/// bot ships in runs as root without `HOME` set in some launchers.
pub(crate) fn home_dir() -> PathBuf {
    env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/root"))
}

/// Append `dir` unless it is already present, preserving first-seen order.
fn push_unique_dir(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dirs.iter().any(|existing| existing == &dir) {
        dirs.push(dir);
    }
}

/// Build a de-duplicated executable search path: the inherited `PATH` first,
/// then `home`-relative directories, then a fixed list of system directories.
///
/// The two tails are parameters rather than a shared constant because the call
/// sites deliberately search different sets — widening either one would change
/// which binary gets picked.
pub(crate) fn search_path_dirs(
    path_env: Option<&OsString>,
    home: Option<&Path>,
    home_dirs: &[&str],
    system_dirs: &[&str],
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path_env) = path_env {
        for dir in env::split_paths(path_env) {
            push_unique_dir(&mut dirs, dir);
        }
    }
    if let Some(home) = home {
        for relative in home_dirs {
            push_unique_dir(&mut dirs, home.join(relative));
        }
    }
    for dir in system_dirs {
        push_unique_dir(&mut dirs, PathBuf::from(dir));
    }
    dirs
}

/// Maximum rendered width of a path before middle-elision kicks in — about
/// two QQ bubble lines of half-width columns.
const FMT_PATH_MAX: usize = 38;

/// User-facing rendering of a filesystem path: the shared temporary
/// workspace shows as a localized label, `$HOME` collapses to `~`, and
/// anything still longer than [`FMT_PATH_MAX`] keeps its head and tail
/// components around a `…`.
pub(crate) fn fmt_path(
    path: &std::path::Path,
    shared_workspace: &std::path::Path,
    locale: &str,
) -> String {
    use rust_i18n::t;
    if path.starts_with(shared_workspace) {
        return t!("commands.shared.temp_workspace", locale = locale).into_owned();
    }
    let display = match home_dir() {
        home if path.starts_with(&home) => {
            let rest = path.strip_prefix(&home).unwrap_or(path);
            if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rest.display())
            }
        }
        _ => path.display().to_string(),
    };
    if display.chars().count() <= FMT_PATH_MAX {
        return display;
    }
    let parts: Vec<&str> = display
        .split('/')
        .filter(|p| !p.is_empty() || true)
        .collect();
    if parts.len() <= 3 {
        return display;
    }
    let head = parts[..2.min(parts.len())].join("/");
    let tail = parts[parts.len().saturating_sub(2)..].join("/");
    let short = format!("{head}/…/{tail}");
    if short.chars().count() < display.chars().count() {
        short
    } else {
        display
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_unique_dir_keeps_first_occurrence() {
        let mut dirs = Vec::new();
        push_unique_dir(&mut dirs, PathBuf::from("/a"));
        push_unique_dir(&mut dirs, PathBuf::from("/b"));
        push_unique_dir(&mut dirs, PathBuf::from("/a"));
        assert_eq!(dirs, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn search_path_dirs_orders_path_then_home_then_system() {
        let dirs = search_path_dirs(
            Some(&OsString::from("/usr/bin:/custom")),
            Some(Path::new("/home/u")),
            &[".cargo/bin", ".local/bin"],
            &["/usr/bin", "/bin"],
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/usr/bin"),
                PathBuf::from("/custom"),
                PathBuf::from("/home/u/.cargo/bin"),
                PathBuf::from("/home/u/.local/bin"),
                PathBuf::from("/bin"),
            ]
        );
    }

    #[test]
    fn search_path_dirs_tolerates_missing_inputs() {
        let dirs = search_path_dirs(None, None, &[".cargo/bin"], &["/bin"]);
        assert_eq!(dirs, vec![PathBuf::from("/bin")]);
        assert!(search_path_dirs(None, None, &[], &[]).is_empty());
    }

    #[test]
    fn fmt_path_labels_shared_workspace_and_collapses_home() {
        let shared = std::path::PathBuf::from("/data/session/workspace");
        assert_eq!(fmt_path(&shared.join("sub"), &shared, "zh"), "临时工作区");
        let home = home_dir();
        assert_eq!(
            fmt_path(&home.join("dev/CodexClaw"), &shared, "zh"),
            "~/dev/CodexClaw"
        );
        let deep = home.join("a/very/long/nested/path/that/keeps/going/forever/deep/dir");
        let rendered = fmt_path(&deep, &shared, "zh");
        assert!(
            rendered.contains('…'),
            "long paths middle-elide: {rendered}"
        );
        assert!(rendered.ends_with("deep/dir"));
    }
}
