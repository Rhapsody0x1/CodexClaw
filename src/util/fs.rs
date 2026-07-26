//! Filesystem helpers: crash-safe writes, optional reads, directory walks.

use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use ulid::Ulid;

/// Write `contents` to `path` so that a reader never observes a partial file:
/// the bytes go to a unique temporary sibling, are fsynced, and only then get
/// renamed over the target. Missing parent directories are created.
///
/// An existing target keeps its permission bits: `File::create` gives the
/// temporary file umask-derived permissions, so without this an operator's
/// deliberately tightened file (e.g. `chmod 600 state.json`) would silently
/// loosen on the next rewrite. Note that fsync errors propagate — callers that
/// used to swallow them get the stricter behavior on purpose.
pub(crate) fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let tmp = tmp_path_for(path);
    {
        let mut file =
            File::create(&tmp).with_context(|| format!("failed to write {}", tmp.display()))?;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        match std::fs::metadata(path) {
            Ok(meta) => file
                .set_permissions(meta.permissions())
                .with_context(|| format!("failed to preserve permissions on {}", tmp.display()))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to stat {} for permissions", path.display()));
            }
        }
        file.sync_all()
            .with_context(|| format!("failed to sync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).with_context(|| {
        format!(
            "failed to replace {} with {}",
            path.display(),
            tmp.display()
        )
    })?;
    Ok(())
}

/// A unique `<name>.<ulid>.tmp` sibling of `path`, so concurrent writers (even
/// across processes) never fight over the same scratch file.
fn tmp_path_for(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("tmp"),
        Ulid::new()
    ))
}

/// Read `path` as UTF-8, mapping "file does not exist" to `None` instead of an
/// error. Every other I/O failure still propagates.
pub(crate) fn read_to_string_opt(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Async twin of [`read_to_string_opt`].
pub(crate) async fn read_to_string_opt_async(path: &Path) -> Result<Option<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(raw) => Ok(Some(raw)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Read and deserialize a JSON file, mapping a missing file to `None`.
pub(crate) fn read_json_opt<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let Some(raw) = read_to_string_opt(path)? else {
        return Ok(None);
    };
    serde_json::from_str(&raw)
        .map(Some)
        .with_context(|| format!("failed to parse {}", path.display()))
}

/// Async twin of [`read_json_opt`].
pub(crate) async fn read_json_opt_async<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let Some(raw) = read_to_string_opt_async(path).await? else {
        return Ok(None);
    };
    serde_json::from_str(&raw)
        .map(Some)
        .with_context(|| format!("failed to parse {}", path.display()))
}

/// Depth-first walk of `root`, calling `visit(path, file_name)` for every
/// regular file whose name is valid UTF-8. Symlinks and other non-regular
/// entries are skipped; a missing `root` yields no visits and no error.
pub(crate) fn walk_files(root: &Path, mut visit: impl FnMut(&Path, &str)) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("failed to read directory {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            visit(&path, name);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_creates_parents_and_leaves_no_tmp_residue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("state.json");
        atomic_write(&path, "{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
        let leftover: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftover.is_empty(), "leftover tmp files: {leftover:?}");
    }

    #[test]
    fn atomic_write_replaces_existing_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        atomic_write(&path, "old").unwrap();
        atomic_write(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn read_helpers_map_missing_files_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        assert!(read_to_string_opt(&missing).unwrap().is_none());
        assert!(
            read_json_opt::<serde_json::Value>(&missing)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn read_json_opt_parses_and_reports_bad_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("value.json");
        std::fs::write(&path, "{\"a\":1}").unwrap();
        let value: serde_json::Value = read_json_opt(&path).unwrap().unwrap();
        assert_eq!(value["a"], 1);

        std::fs::write(&path, "not json").unwrap();
        let err = read_json_opt::<serde_json::Value>(&path).unwrap_err();
        assert!(err.to_string().contains("failed to parse"));
    }

    #[tokio::test]
    async fn async_read_helpers_match_their_sync_twins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("value.json");
        assert!(read_to_string_opt_async(&path).await.unwrap().is_none());
        assert!(
            read_json_opt_async::<serde_json::Value>(&path)
                .await
                .unwrap()
                .is_none()
        );
        std::fs::write(&path, "{\"a\":2}").unwrap();
        let value: serde_json::Value = read_json_opt_async(&path).await.unwrap().unwrap();
        assert_eq!(value["a"], 2);
    }

    #[test]
    fn walk_files_recurses_and_reports_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a").join("b")).unwrap();
        std::fs::write(dir.path().join("top.txt"), "x").unwrap();
        std::fs::write(dir.path().join("a").join("mid.txt"), "x").unwrap();
        std::fs::write(dir.path().join("a").join("b").join("deep.log"), "x").unwrap();

        let mut names = Vec::new();
        walk_files(dir.path(), |path, name| {
            assert!(path.is_file());
            names.push(name.to_string());
        })
        .unwrap();
        names.sort();
        assert_eq!(names, vec!["deep.log", "mid.txt", "top.txt"]);
    }

    #[test]
    fn walk_files_on_missing_root_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let mut visited = 0;
        walk_files(&dir.path().join("nope"), |_, _| visited += 1).unwrap();
        assert_eq!(visited, 0);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_tightened_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        atomic_write(&path, "new").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "rewrite must not loosen a chmod 600 file");
    }
}
