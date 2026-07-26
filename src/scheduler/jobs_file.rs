//! Locked persistence for `scheduler/jobs.json`.
//!
//! Unlike `session/state.json`, the cron jobs file is shared with the
//! independent `codex-claw cron` process, so every access goes through an
//! fs2 advisory file lock (`jobs.json.lock`): shared for reads, exclusive
//! for writes and read-modify-write cycles. All functions here block on
//! disk I/O — callers run them under `spawn_blocking`.

use std::{collections::BTreeMap, fs::OpenOptions, path::Path, path::PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;

use crate::model::cron::CronJob;
use crate::util::fs::{atomic_write, read_json_opt};

/// Reads the jobs map under a shared lock. `fallback` is returned when the
/// file does not exist yet (legacy inline state before migration).
pub(crate) fn read_jobs(
    path: &Path,
    fallback: BTreeMap<String, CronJob>,
) -> Result<BTreeMap<String, CronJob>> {
    with_cron_jobs_lock(&cron_jobs_lock_path(path), false, || {
        read_cron_jobs_file(path, fallback)
    })
}

/// Writes the jobs map under an exclusive lock (atomic temp-file + rename).
pub(crate) fn write_jobs(path: &Path, jobs: &BTreeMap<String, CronJob>) -> Result<()> {
    with_cron_jobs_lock(&cron_jobs_lock_path(path), true, || {
        write_cron_jobs_file(path, jobs)
    })
}

/// Read-modify-write cycle under one exclusive lock, so no other process can
/// interleave between the read and the write-back.
pub(crate) fn mutate_jobs<T>(
    path: &Path,
    fallback: BTreeMap<String, CronJob>,
    mutator: impl FnOnce(&mut BTreeMap<String, CronJob>) -> Result<T>,
) -> Result<T> {
    with_cron_jobs_lock(&cron_jobs_lock_path(path), true, || {
        let mut jobs = read_cron_jobs_file(path, fallback)?;
        let result = mutator(&mut jobs)?;
        write_cron_jobs_file(path, &jobs)?;
        Ok(result)
    })
}

fn cron_jobs_lock_path(path: &Path) -> PathBuf {
    path.with_extension("json.lock")
}

fn with_cron_jobs_lock<T, F>(lock_path: &Path, exclusive: bool, f: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .with_context(|| format!("failed to open {}", lock_path.display()))?;
    if exclusive {
        FileExt::lock_exclusive(&lock_file)
            .with_context(|| format!("failed to lock {}", lock_path.display()))?;
    } else {
        FileExt::lock_shared(&lock_file)
            .with_context(|| format!("failed to lock {}", lock_path.display()))?;
    }
    let result = f();
    FileExt::unlock(&lock_file)
        .with_context(|| format!("failed to unlock {}", lock_path.display()))?;
    result
}

fn read_cron_jobs_file(
    path: &Path,
    fallback: BTreeMap<String, CronJob>,
) -> Result<BTreeMap<String, CronJob>> {
    Ok(read_json_opt::<BTreeMap<String, CronJob>>(path)?.unwrap_or(fallback))
}

fn write_cron_jobs_file(path: &Path, jobs: &BTreeMap<String, CronJob>) -> Result<()> {
    atomic_write(path, &serde_json::to_string_pretty(jobs)?)
}
