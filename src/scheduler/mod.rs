pub mod cli;
pub mod cron_expr;
pub mod runner;
pub mod store;

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

use crate::app::App;

pub struct Scheduler {
    app: Weak<App>,
    in_flight: Arc<Mutex<HashSet<String>>>,
    sem: Arc<Semaphore>,
}

impl Scheduler {
    pub fn spawn(app: Arc<App>) {
        if !app.config.scheduler.enabled {
            info!("scheduler disabled");
            return;
        }
        let sem = Arc::new(Semaphore::new(
            app.config.scheduler.max_concurrent_jobs.max(1),
        ));
        let scheduler = Arc::new(Self {
            app: Arc::downgrade(&app),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            sem,
        });
        tokio::spawn(scheduler.run());
    }

    async fn run(self: Arc<Self>) {
        self.bootstrap_next_runs().await;
        let tick_secs = self
            .app
            .upgrade()
            .map(|app| app.config.scheduler.tick_secs.max(1))
            .unwrap_or(30);
        let mut interval = tokio::time::interval(Duration::from_secs(tick_secs));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            self.tick().await;
        }
    }

    async fn bootstrap_next_runs(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let now = Utc::now();
        let Ok(jobs) = app.session.list_cron_jobs().await else {
            return;
        };
        for mut job in jobs {
            if job.disabled {
                continue;
            }
            if cron_expr::due_or_past(&job.kind, now) {
                job.next_run_at = Some(now);
                if let Err(err) = app.session.upsert_cron_job(job).await {
                    warn!(error = %err, "failed to persist scheduler bootstrap state");
                }
                continue;
            }
            match cron_expr::next_after(&job.kind, now) {
                Ok(next) => {
                    job.next_run_at = next;
                    if let Err(err) = app.session.upsert_cron_job(job).await {
                        warn!(error = %err, "failed to persist scheduler bootstrap state");
                    }
                }
                Err(err) => warn!(job_id = %job.id, error = %err, "invalid cron job schedule"),
            }
        }
    }

    async fn tick(self: &Arc<Self>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let now = Utc::now();
        let mut due = match app.session.list_cron_jobs().await {
            Ok(jobs) => jobs
                .into_iter()
                .filter(|job| {
                    !job.disabled
                        && job
                            .next_run_at
                            .or_else(|| cron_expr::next_after(&job.kind, now).ok().flatten())
                            .is_some_and(|next| next <= now)
                })
                .collect::<Vec<_>>(),
            Err(err) => {
                warn!(error = %err, "failed to load cron jobs");
                return;
            }
        };
        due.sort_by_key(|job| job.next_run_at);
        for job in due {
            let scheduler = self.clone();
            let app = app.clone();
            tokio::spawn(async move {
                scheduler.spawn_job(app, job).await;
            });
        }
    }

    async fn spawn_job(self: Arc<Self>, app: Arc<App>, job: store::CronJob) {
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
        let result = runner::run_job(app, job).await;
        drop(permit);
        self.in_flight.lock().await.remove(&job_id);
        match result {
            Ok(job) => info!(job_id = %job.id, "cron job completed"),
            Err(err) => error!(job_id = %job_id, error = %err, "cron job failed"),
        }
    }
}
