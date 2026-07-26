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
}
