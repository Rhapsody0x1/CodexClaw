use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};

use crate::{
    config::AppConfig, session::state::ApprovalPolicySetting, session::store::SessionStore,
};

use super::{
    cron_expr,
    store::{self, CronJob, CronKind, DeliverPolicy, JobAction, SessionStrategy},
};

pub async fn run(args: &[String], config: &AppConfig) -> Result<()> {
    let session = SessionStore::load_or_init(
        &config.general.data_dir,
        &config.general.codex_home_global,
        &config.general.system_codex_home,
        &config.general.default_workspace_dir,
    )
    .await?;
    match args.first().map(String::as_str) {
        Some("add") => add(&session, config, &args[1..], false).await,
        Some("once") => add(&session, config, &args[1..], true).await,
        Some("list") => list(&session, &args[1..]).await,
        Some("rm") => remove(&session, config, &args[1..]).await,
        Some("pause") => set_disabled(&session, &args[1..], true).await,
        Some("resume") => set_disabled(&session, &args[1..], false).await,
        Some("run-now") => run_now(&session, &args[1..]).await,
        Some("tail") => tail(&session, &args[1..]).await,
        _ => {
            print_usage();
            Ok(())
        }
    }
}

async fn add(
    session: &SessionStore,
    config: &AppConfig,
    args: &[String],
    one_shot: bool,
) -> Result<()> {
    let opts = Opts::parse(args);
    let owner = opts
        .value("owner")
        .or_else(read_owner_from_turn_file)
        .ok_or_else(|| anyhow!("missing --owner and no .claw-turn.json in cwd"))?;
    let title = opts
        .value("title")
        .unwrap_or_else(|| "scheduled task".to_string());
    let action_name = opts.value("action").unwrap_or_else(|| "shell".to_string());
    let prompt = match opts.value("prompt") {
        Some(prompt) => prompt,
        None => read_prompt_file(opts.value("prompt-file"))?,
    };
    let id = store::new_id();
    let job_dir = store::prepare_job_dirs(&config.general.data_dir, &id).await?;
    let workspace_dir = opts
        .value("workspace")
        .map(PathBuf::from)
        .unwrap_or_else(|| job_dir.join("workspace"));
    tokio::fs::create_dir_all(&workspace_dir).await?;
    let kind = if one_shot {
        let at = opts
            .value("at")
            .ok_or_else(|| anyhow!("once requires --at <RFC3339>"))?;
        CronKind::OneShot {
            at: DateTime::parse_from_rfc3339(&at)
                .with_context(|| format!("invalid --at `{at}`"))?
                .with_timezone(&Utc),
        }
    } else {
        let cron = opts
            .value("cron")
            .ok_or_else(|| anyhow!("add requires --cron"))?;
        CronKind::Recurring {
            cron: normalize_cron(&cron),
            tz: opts
                .value("tz")
                .unwrap_or_else(|| config.scheduler.default_tz.clone()),
        }
    };
    let action = match action_name.as_str() {
        "reminder" => JobAction::Reminder {
            message: opts
                .value("message")
                .or_else(|| {
                    if prompt.trim().is_empty() {
                        None
                    } else {
                        Some(prompt.clone())
                    }
                })
                .ok_or_else(|| anyhow!("reminder action requires --message or --prompt"))?,
        },
        "shell" => JobAction::Shell {
            program: opts
                .value("program")
                .ok_or_else(|| anyhow!("shell action requires --program"))?,
            args: opts.values("arg"),
            env: BTreeMap::new(),
        },
        "codex-exec" => JobAction::CodexExec {
            prompt,
            model: opts.value("model"),
            extra_args: opts.values("extra-arg"),
            env: BTreeMap::new(),
        },
        "codex-turn" => JobAction::CodexTurn {
            prompt,
            model: opts.value("model"),
            session_state: None,
            approval_policy: opts
                .value("approval")
                .as_deref()
                .map(parse_approval_policy)
                .transpose()?,
            session_strategy: parse_session_strategy(
                opts.value("session-strategy")
                    .as_deref()
                    .unwrap_or("per-invocation"),
            )?,
            interactive: if opts.flag("interactive") {
                Some(store::InteractiveSpec {
                    reply_ttl_secs: opts
                        .value("reply-ttl")
                        .as_deref()
                        .map(str::parse)
                        .transpose()
                        .context("invalid --reply-ttl")?
                        .unwrap_or(86_400),
                    end_signal: opts
                        .value("end-signal")
                        .unwrap_or_else(|| "<<<CLAW_END>>>".to_string()),
                    max_rounds_hard_cap: opts
                        .value("max-rounds")
                        .as_deref()
                        .map(str::parse)
                        .transpose()
                        .context("invalid --max-rounds")?
                        .unwrap_or(10),
                })
            } else {
                None
            },
        },
        other => return Err(anyhow!("unsupported --action `{other}`")),
    };
    let now = Utc::now();
    let mut job = CronJob {
        id,
        owner_openid: owner,
        title,
        kind,
        action,
        workspace_dir,
        deliver: DeliverPolicy::PushToOwner,
        created_at: now,
        next_run_at: None,
        run_now_at: None,
        last_run_at: None,
        last_run_status: None,
        run_count: 0,
        failure_streak: 0,
        disabled: false,
    };
    job.next_run_at = match &job.kind {
        CronKind::OneShot { at } if *at <= now => Some(*at),
        _ => cron_expr::next_after(&job.kind, now)?,
    };
    store::write_job_metadata(
        &job,
        &config.general.data_dir,
        &config.general.codex_home_global,
    )
    .await?;
    session.upsert_cron_job(job.clone()).await?;
    println!(
        "created cron job {} `{}` next_run_at={}",
        job.id,
        job.title,
        job.next_run_at
            .map(|value| value.to_rfc3339())
            .unwrap_or_else(|| "none".to_string())
    );
    Ok(())
}

async fn list(session: &SessionStore, args: &[String]) -> Result<()> {
    let opts = Opts::parse(args);
    let owner = opts.value("owner");
    let mut jobs = session.list_cron_jobs().await?;
    if let Some(owner) = owner {
        jobs.retain(|job| job.owner_openid == owner);
    }
    jobs.sort_by_key(|job| job.next_run_at);
    for job in jobs {
        println!(
            "{}\t{}\tnext={}\truns={}\tfailures={}\t{}\t{}",
            job.id,
            if job.disabled { "disabled" } else { "enabled" },
            job.next_run_at
                .map(|value| value.to_rfc3339())
                .unwrap_or_else(|| "-".to_string()),
            job.run_count,
            job.failure_streak,
            job.owner_openid,
            job.title
        );
    }
    Ok(())
}

async fn remove(session: &SessionStore, config: &AppConfig, args: &[String]) -> Result<()> {
    let opts = Opts::parse(args);
    let keep_files = opts.flag("keep-files");
    let id = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .ok_or_else(|| anyhow!("rm requires <job_id>"))?;
    if session.remove_cron_job(id).await?.is_some() {
        store::remove_job_files(
            &config.general.data_dir,
            &config.general.codex_home_global,
            id,
            keep_files,
        )
        .await?;
        println!("removed {id}");
    } else {
        println!("not found {id}");
    }
    Ok(())
}

async fn set_disabled(session: &SessionStore, args: &[String], disabled: bool) -> Result<()> {
    let id = args
        .first()
        .ok_or_else(|| anyhow!("pause/resume requires <job_id>"))?;
    let mut job = session
        .get_cron_job(id)
        .await?
        .ok_or_else(|| anyhow!("job not found `{id}`"))?;
    job.disabled = disabled;
    if !disabled {
        job.next_run_at = cron_expr::next_after(&job.kind, Utc::now())?;
        if job.next_run_at.is_none() && matches!(job.kind, CronKind::OneShot { .. }) {
            job.run_now_at = Some(Utc::now());
        }
    }
    session.upsert_cron_job(job).await?;
    println!("{} {id}", if disabled { "paused" } else { "resumed" });
    Ok(())
}

async fn run_now(session: &SessionStore, args: &[String]) -> Result<()> {
    let id = args
        .first()
        .ok_or_else(|| anyhow!("run-now requires <job_id>"))?;
    session
        .update_cron_job(id, |job| {
            job.run_now_at = Some(Utc::now());
            Ok(())
        })
        .await?
        .ok_or_else(|| anyhow!("job not found `{id}`"))?;
    println!("scheduled {id} to run now");
    Ok(())
}

async fn tail(session: &SessionStore, args: &[String]) -> Result<()> {
    let id = args
        .first()
        .ok_or_else(|| anyhow!("tail requires <job_id>"))?;
    let job = session
        .get_cron_job(id)
        .await?
        .ok_or_else(|| anyhow!("job not found `{id}`"))?;
    let runs_dir = job
        .workspace_dir
        .parent()
        .unwrap_or(job.workspace_dir.as_path())
        .join("runs");
    let mut entries = std::fs::read_dir(&runs_dir)
        .with_context(|| format!("failed to read {}", runs_dir.display()))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    let Some(last) = entries.last() else {
        println!(
            "{}\t{}\tnext={}\truns={}\tfailures={}",
            job.id,
            if job.disabled { "disabled" } else { "enabled" },
            job.next_run_at
                .map(|value| value.to_rfc3339())
                .unwrap_or_else(|| "-".to_string()),
            job.run_count,
            job.failure_streak
        );
        return Ok(());
    };
    println!(
        "{}\t{}\tnext={}\truns={}\tfailures={}\tlast_status={:?}\n--- {} ---",
        job.id,
        if job.disabled { "disabled" } else { "enabled" },
        job.next_run_at
            .map(|value| value.to_rfc3339())
            .unwrap_or_else(|| "-".to_string()),
        job.run_count,
        job.failure_streak,
        job.last_run_status,
        last.path().display()
    );
    print!("{}", std::fs::read_to_string(last.path())?);
    Ok(())
}

fn read_prompt_file(path: Option<String>) -> Result<String> {
    let Some(path) = path else {
        return Ok(String::new());
    };
    std::fs::read_to_string(&path).with_context(|| format!("failed to read prompt file `{path}`"))
}

fn read_owner_from_turn_file() -> Option<String> {
    let raw = std::fs::read_to_string(".claw-turn.json").ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    value
        .get("owner_openid")
        .or_else(|| value.get("openid"))
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

fn normalize_cron(raw: &str) -> String {
    if raw.split_whitespace().count() == 5 {
        format!("0 {raw}")
    } else {
        raw.to_string()
    }
}

fn parse_session_strategy(raw: &str) -> Result<SessionStrategy> {
    match raw {
        "per-invocation" | "per_invocation" | "fresh" => Ok(SessionStrategy::PerInvocation),
        "persistent" => Ok(SessionStrategy::Persistent),
        _ => Err(anyhow!("invalid --session-strategy `{raw}`")),
    }
}

fn parse_approval_policy(raw: &str) -> Result<ApprovalPolicySetting> {
    ApprovalPolicySetting::parse(raw).ok_or_else(|| anyhow!("invalid --approval `{raw}`"))
}

fn print_usage() {
    println!(
        "usage: codex-claw cron add|once|list|rm|pause|resume|run-now|tail\n\
         examples:\n\
         codex-claw cron add --owner OPENID --cron '0 16 * * *' --title homework --action reminder --message '记得检查还有没有没交的作业'\n\
         codex-claw cron once --owner OPENID --at 2026-05-20T08:00:00+08:00 --title drink --action reminder --message '喝水'"
    );
}

struct Opts<'a> {
    args: &'a [String],
}

impl<'a> Opts<'a> {
    fn parse(args: &'a [String]) -> Self {
        Self { args }
    }

    fn value(&self, name: &str) -> Option<String> {
        let needle = format!("--{name}");
        self.args
            .windows(2)
            .find(|pair| pair[0] == needle)
            .map(|pair| pair[1].clone())
    }

    fn values(&self, name: &str) -> Vec<String> {
        let needle = format!("--{name}");
        self.args
            .windows(2)
            .filter(|pair| pair[0] == needle)
            .map(|pair| pair[1].clone())
            .collect()
    }

    fn flag(&self, name: &str) -> bool {
        let needle = format!("--{name}");
        self.args.iter().any(|arg| arg == &needle)
    }
}
