//! The scheduler's own task: a periodic tick that finds due cron jobs and
//! spawns them, bounded by an in-flight set and a concurrency semaphore.

use std::{
    collections::HashSet,
    sync::{Arc, Weak},
};

use chrono::Utc;
use tokio::{
    sync::{Mutex, Semaphore},
    time::{Duration, MissedTickBehavior},
};
use tracing::{error, info, warn};

use super::{cron_expr, ctx::SchedulerCtx, interactive, runner, store};

pub struct Scheduler {
    ctx: Weak<SchedulerCtx>,
    in_flight: Arc<Mutex<HashSet<String>>>,
    sem: Arc<Semaphore>,
}

impl Scheduler {
    /// Start the tick loop against `ctx`. Only a `Weak` is retained here: the
    /// caller (the composition root in `main`) must park the strong reference
    /// somewhere long-lived — in production the `App` owns it — so dropping
    /// that owner silently parks the scheduler.
    pub fn spawn(ctx: Arc<SchedulerCtx>) {
        if !ctx.config.scheduler.enabled {
            info!("scheduler disabled");
            return;
        }
        let sem = Arc::new(Semaphore::new(
            ctx.config.scheduler.max_concurrent_jobs.max(1),
        ));
        let scheduler = Arc::new(Self {
            ctx: Arc::downgrade(&ctx),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            sem,
        });
        tokio::spawn(scheduler.run());
    }

    async fn run(self: Arc<Self>) {
        self.bootstrap_next_runs().await;
        let tick_secs = self
            .ctx
            .upgrade()
            .map(|ctx| ctx.config.scheduler.tick_secs.max(1))
            .unwrap_or(30);
        let mut interval = tokio::time::interval(Duration::from_secs(tick_secs));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            self.tick().await;
        }
    }

    async fn bootstrap_next_runs(&self) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Err(err) = interactive::sweep_expired(&ctx).await {
            warn!(error = %err, "failed to sweep expired interactive cron jobs");
        }
        let now = Utc::now();
        let Ok(jobs) = ctx.session.list_cron_jobs().await else {
            return;
        };
        for mut job in jobs {
            if job.disabled {
                continue;
            }
            if job.run_now_at.is_some() {
                continue;
            }
            if cron_expr::due_or_past(&job.kind, now) {
                job.next_run_at = Some(now);
                if let Err(err) = ctx.session.upsert_cron_job(job).await {
                    warn!(error = %err, "failed to persist scheduler bootstrap state");
                }
                continue;
            }
            // A persisted next_run_at that is already past-due means a run was
            // missed while the process was down. Keep it so the tick loop fires
            // it once to catch up, instead of overwriting it with the next
            // future occurrence and silently dropping the missed run.
            if job.next_run_at.is_some_and(|next| next <= now) {
                continue;
            }
            match cron_expr::next_after(&job.kind, now) {
                Ok(next) => {
                    job.next_run_at = next;
                    if let Err(err) = ctx.session.upsert_cron_job(job).await {
                        warn!(error = %err, "failed to persist scheduler bootstrap state");
                    }
                }
                Err(err) => warn!(job_id = %job.id, error = %err, "invalid cron job schedule"),
            }
        }
    }

    async fn tick(self: &Arc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Err(err) = interactive::sweep_expired(&ctx).await {
            warn!(error = %err, "failed to sweep expired interactive cron jobs");
        }
        let now = Utc::now();
        let mut due = match ctx.session.list_cron_jobs().await {
            Ok(jobs) => jobs
                .into_iter()
                .filter(|job| is_due(job, now))
                .collect::<Vec<_>>(),
            Err(err) => {
                warn!(error = %err, "failed to load cron jobs");
                return;
            }
        };
        due.sort_by_key(|job| job.next_run_at);
        for job in due {
            let scheduler = self.clone();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                scheduler.spawn_job(ctx, job).await;
            });
        }
    }

    async fn spawn_job(self: Arc<Self>, ctx: Arc<SchedulerCtx>, job: store::CronJob) {
        {
            let mut guard = self.in_flight.lock().await;
            if !guard.insert(job.id.clone()) {
                warn!(job_id = %job.id, "cron job already in flight; skipping tick");
                return;
            }
        }
        let permit = match self.sem.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return,
        };
        let job_id = job.id.clone();
        let result = runner::run_job(ctx, job).await;
        drop(permit);
        self.in_flight.lock().await.remove(&job_id);
        match result {
            Ok(job) => info!(job_id = %job.id, "cron job completed"),
            Err(err) => error!(job_id = %job_id, error = %err, "cron job failed"),
        }
    }
}

/// Whether the tick loop should fire `job` now: a due `run-now` request always
/// wins (even on a disabled job); otherwise the job must be enabled with a due
/// `next_run_at` (computed from the schedule when unset).
fn is_due(job: &store::CronJob, now: chrono::DateTime<Utc>) -> bool {
    if job.run_now_at.is_some_and(|run_now_at| run_now_at <= now) {
        return true;
    }
    if job.disabled {
        return false;
    }
    let next = match job.next_run_at {
        Some(next) => Some(next),
        // Distinguish a genuine Err from Ok(None): the old code swallowed Err
        // via .ok(), so a job with an invalid or uncomputable schedule looked
        // enabled but silently never ran and never warned. Surface the error
        // instead.
        None => match cron_expr::next_after(&job.kind, now) {
            Ok(next) => next,
            Err(err) => {
                warn!(
                    job_id = %job.id,
                    error = %err,
                    "cron job has an invalid schedule and will not run until fixed"
                );
                None
            }
        },
    };
    next.is_some_and(|next| next <= now)
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;
    use crate::model::cron::fixtures::{shell_job, ts};
    use crate::scheduler::store::CronKind;

    #[test]
    fn run_now_fires_even_a_disabled_job() {
        let now = ts("2026-05-10T10:00:00Z");
        let mut job = shell_job("job-1", "/tmp".into(), now + Duration::hours(1));
        job.disabled = true;
        job.run_now_at = Some(now);
        assert!(is_due(&job, now));

        // A run-now stamped in the future does not fire yet, and the disabled
        // job stays parked.
        job.run_now_at = Some(now + Duration::seconds(1));
        assert!(!is_due(&job, now));
    }

    #[test]
    fn disabled_jobs_are_never_due_without_run_now() {
        let now = ts("2026-05-10T10:00:00Z");
        let mut job = shell_job("job-1", "/tmp".into(), now - Duration::hours(1));
        job.disabled = true;
        assert!(!is_due(&job, now));
    }

    #[test]
    fn next_run_at_decides_when_present() {
        let now = ts("2026-05-10T10:00:00Z");
        let mut job = shell_job("job-1", "/tmp".into(), now);
        job.next_run_at = Some(now);
        assert!(is_due(&job, now));
        job.next_run_at = Some(now + Duration::seconds(1));
        assert!(!is_due(&job, now));
    }

    #[test]
    fn one_shot_without_next_run_follows_the_schedule() {
        let now = ts("2026-05-10T10:00:00Z");
        // A one-shot whose `at` already passed has no next occurrence: its
        // single run is spent, so it must not fire again.
        let mut job = shell_job("job-1", "/tmp".into(), now - Duration::hours(1));
        job.next_run_at = None;
        assert!(!is_due(&job, now));

        // One scheduled for later is not due yet either (`at > now`).
        let mut job = shell_job("job-2", "/tmp".into(), now + Duration::hours(1));
        job.next_run_at = None;
        assert!(!is_due(&job, now));
    }

    #[test]
    fn invalid_schedule_is_not_due() {
        let now = ts("2026-05-10T10:00:00Z");
        let mut job = shell_job("job-1", "/tmp".into(), now);
        job.kind = CronKind::Recurring {
            cron: "not a cron".to_string(),
            tz: "UTC".to_string(),
        };
        job.next_run_at = None;
        assert!(!is_due(&job, now));
    }
}
