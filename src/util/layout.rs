//! The on-disk layout under the configured `data_dir`.
//!
//! Every directory and file name the bot writes below `data_dir` is spelled out
//! exactly once here. Before this existed the same literals were re-joined in
//! eight modules, and the list of directories exposed to a codex turn was
//! duplicated between the foreground and cron paths with nothing to keep the
//! two copies in sync.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct DataLayout {
    root: PathBuf,
}

impl DataLayout {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: data_dir.into(),
        }
    }

    /// Only the tests need the bare root back out; every production caller goes
    /// through one of the named accessors below.
    #[cfg(test)]
    pub(crate) fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Session state, shared workspace and attachment inbox.
    pub(crate) fn session_dir(&self) -> PathBuf {
        self.root.join("session")
    }

    pub(crate) fn session_state_file(&self) -> PathBuf {
        self.session_dir().join("state.json")
    }

    /// Pre-multi-user settings file, still read once on first start-up.
    pub(crate) fn legacy_settings_file(&self) -> PathBuf {
        self.session_dir().join("main").join("settings.json")
    }

    pub(crate) fn shared_workspace_dir(&self) -> PathBuf {
        self.session_dir().join("workspace")
    }

    /// Scheduler bookkeeping: the job table and queued deliveries.
    fn scheduler_dir(&self) -> PathBuf {
        self.root.join("scheduler")
    }

    pub(crate) fn cron_jobs_file(&self) -> PathBuf {
        self.scheduler_dir().join("jobs.json")
    }

    pub(crate) fn pending_deliveries_dir(&self) -> PathBuf {
        self.scheduler_dir().join("pending-deliveries")
    }

    /// Per-job working directories.
    pub(crate) fn cron_jobs_dir(&self) -> PathBuf {
        self.root.join("cron-jobs")
    }

    pub(crate) fn cron_job_dir(&self, id: &str) -> PathBuf {
        self.cron_jobs_dir().join(id)
    }

    pub(crate) fn cron_jobs_trash_dir(&self) -> PathBuf {
        self.root.join("cron-jobs-trash")
    }

    pub fn memory_dir(&self) -> PathBuf {
        self.root.join("memory")
    }

    pub fn shadow_workspace_dir(&self) -> PathBuf {
        self.root.join("shadow-workspace")
    }

    pub(crate) fn qq_dir(&self) -> PathBuf {
        self.root.join("qq")
    }

    pub(crate) fn gateway_session_file(&self) -> PathBuf {
        self.qq_dir().join("gateway-session.json")
    }

    /// Directories every codex turn gets write access to on top of its own
    /// workspace, so a turn can read its session state and reach the cron job
    /// tree. Shared by the foreground and the scheduler paths: adding a
    /// directory here reaches both.
    pub(crate) fn turn_add_dirs(&self) -> Vec<PathBuf> {
        vec![
            self.session_dir(),
            self.cron_jobs_dir(),
            self.scheduler_dir(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn paths_hang_off_the_data_dir() {
        let layout = DataLayout::new("/data");
        assert_eq!(layout.root(), Path::new("/data"));
        assert_eq!(layout.session_dir(), Path::new("/data/session"));
        assert_eq!(
            layout.session_state_file(),
            Path::new("/data/session/state.json")
        );
        assert_eq!(
            layout.legacy_settings_file(),
            Path::new("/data/session/main/settings.json")
        );
        assert_eq!(
            layout.shared_workspace_dir(),
            Path::new("/data/session/workspace")
        );
        assert_eq!(
            layout.cron_jobs_file(),
            Path::new("/data/scheduler/jobs.json")
        );
        assert_eq!(
            layout.pending_deliveries_dir(),
            Path::new("/data/scheduler/pending-deliveries")
        );
        assert_eq!(layout.cron_job_dir("j1"), Path::new("/data/cron-jobs/j1"));
        assert_eq!(
            layout.cron_jobs_trash_dir(),
            Path::new("/data/cron-jobs-trash")
        );
        assert_eq!(layout.memory_dir(), Path::new("/data/memory"));
        assert_eq!(
            layout.shadow_workspace_dir(),
            Path::new("/data/shadow-workspace")
        );
        assert_eq!(
            layout.gateway_session_file(),
            Path::new("/data/qq/gateway-session.json")
        );
    }

    #[test]
    fn turn_add_dirs_is_the_single_whitelist() {
        let layout = DataLayout::new("/data");
        assert_eq!(
            layout.turn_add_dirs(),
            vec![
                PathBuf::from("/data/session"),
                PathBuf::from("/data/cron-jobs"),
                PathBuf::from("/data/scheduler"),
            ]
        );
    }
}
