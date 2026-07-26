use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;

use crate::{
    config::AppConfig,
    session::{SessionStore, state::ApprovalPolicySetting},
    util::time::{fmt_rfc3339_or, parse_utc_strict},
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
    let kind = parse_kind(&opts, one_shot, &config.scheduler.default_tz)?;
    let action = parse_action(&opts, &action_name, prompt)?;
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
        fmt_rfc3339_or(job.next_run_at, "none")
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
            fmt_rfc3339_or(job.next_run_at, "-"),
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
    let runs_dir = store::job_runs_dir(&job);
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
            fmt_rfc3339_or(job.next_run_at, "-"),
            job.run_count,
            job.failure_streak
        );
        return Ok(());
    };
    println!(
        "{}\t{}\tnext={}\truns={}\tfailures={}\tlast_status={:?}\n--- {} ---",
        job.id,
        if job.disabled { "disabled" } else { "enabled" },
        fmt_rfc3339_or(job.next_run_at, "-"),
        job.run_count,
        job.failure_streak,
        job.last_run_status,
        last.path().display()
    );
    print!("{}", std::fs::read_to_string(last.path())?);
    Ok(())
}

/// `once` schedules a single run at `--at`; `add` a recurring one from
/// `--cron` (5-field specs get a seconds column prepended) in `--tz`, falling
/// back to the configured default timezone.
fn parse_kind(opts: &Opts, one_shot: bool, default_tz: &str) -> Result<CronKind> {
    if one_shot {
        let at = opts
            .value("at")
            .ok_or_else(|| anyhow!("once requires --at <RFC3339>"))?;
        Ok(CronKind::OneShot {
            at: parse_utc_strict(&at).with_context(|| format!("invalid --at `{at}`"))?,
        })
    } else {
        let cron = opts
            .value("cron")
            .ok_or_else(|| anyhow!("add requires --cron"))?;
        Ok(CronKind::Recurring {
            cron: normalize_cron(&cron),
            tz: opts.value("tz").unwrap_or_else(|| default_tz.to_string()),
        })
    }
}

fn parse_action(opts: &Opts, action_name: &str, prompt: String) -> Result<JobAction> {
    match action_name {
        "reminder" => Ok(JobAction::Reminder {
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
        }),
        "shell" => Ok(JobAction::Shell {
            program: opts
                .value("program")
                .ok_or_else(|| anyhow!("shell action requires --program"))?,
            args: opts.values("arg"),
            env: BTreeMap::new(),
        }),
        "codex-exec" => Ok(JobAction::CodexExec {
            prompt,
            model: opts.value("model"),
            extra_args: opts.values("extra-arg"),
            env: BTreeMap::new(),
        }),
        "codex-turn" => Ok(JobAction::CodexTurn {
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
            interactive: parse_interactive(opts)?,
        }),
        other => Err(anyhow!("unsupported --action `{other}`")),
    }
}

/// `--interactive` opts a codex-turn job into the foreground-interaction
/// protocol; the knobs default to the same values serde fills in when the
/// fields are absent from a persisted `job.toml`.
fn parse_interactive(opts: &Opts) -> Result<Option<store::InteractiveSpec>> {
    if !opts.flag("interactive") {
        return Ok(None);
    }
    Ok(Some(store::InteractiveSpec {
        reply_ttl_secs: opts
            .value("reply-ttl")
            .as_deref()
            .map(str::parse)
            .transpose()
            .context("invalid --reply-ttl")?
            .unwrap_or_else(store::default_reply_ttl_secs),
        end_signal: opts
            .value("end-signal")
            .unwrap_or_else(store::default_end_signal),
        max_rounds_hard_cap: opts
            .value("max-rounds")
            .as_deref()
            .map(str::parse)
            .transpose()
            .context("invalid --max-rounds")?
            .unwrap_or_else(store::default_max_rounds_hard_cap),
    }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::time::parse_utc_strict;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn parse_kind_once_requires_and_parses_at() {
        let missing = args(&[]);
        let err = parse_kind(&Opts::parse(&missing), true, "UTC").unwrap_err();
        assert!(err.to_string().contains("once requires --at"));

        let bad = args(&["--at", "not-a-time"]);
        let err = parse_kind(&Opts::parse(&bad), true, "UTC").unwrap_err();
        assert!(err.to_string().contains("invalid --at `not-a-time`"));

        let good = args(&["--at", "2026-05-20T08:00:00+08:00"]);
        let kind = parse_kind(&Opts::parse(&good), true, "UTC").unwrap();
        assert_eq!(
            kind,
            CronKind::OneShot {
                at: parse_utc_strict("2026-05-20T08:00:00+08:00").unwrap(),
            }
        );
    }

    #[test]
    fn parse_kind_recurring_normalizes_cron_and_defaults_tz() {
        let missing = args(&[]);
        let err = parse_kind(&Opts::parse(&missing), false, "UTC").unwrap_err();
        assert!(err.to_string().contains("add requires --cron"));

        // A 5-field spec gets a seconds column prepended; tz falls back to the
        // configured default.
        let five = args(&["--cron", "0 16 * * *"]);
        assert_eq!(
            parse_kind(&Opts::parse(&five), false, "Asia/Shanghai").unwrap(),
            CronKind::Recurring {
                cron: "0 0 16 * * *".to_string(),
                tz: "Asia/Shanghai".to_string(),
            }
        );

        // A 6-field spec passes through untouched; --tz wins over the default.
        let six = args(&["--cron", "30 0 16 * * *", "--tz", "UTC"]);
        assert_eq!(
            parse_kind(&Opts::parse(&six), false, "Asia/Shanghai").unwrap(),
            CronKind::Recurring {
                cron: "30 0 16 * * *".to_string(),
                tz: "UTC".to_string(),
            }
        );
    }

    #[test]
    fn parse_action_reminder_prefers_message_then_prompt() {
        let with_message = args(&["--message", "drink water"]);
        assert_eq!(
            parse_action(
                &Opts::parse(&with_message),
                "reminder",
                "ignored".to_string()
            )
            .unwrap(),
            JobAction::Reminder {
                message: "drink water".to_string(),
            }
        );

        let empty = args(&[]);
        assert_eq!(
            parse_action(&Opts::parse(&empty), "reminder", "from prompt".to_string()).unwrap(),
            JobAction::Reminder {
                message: "from prompt".to_string(),
            }
        );

        let err = parse_action(&Opts::parse(&empty), "reminder", "  ".to_string()).unwrap_err();
        assert!(
            err.to_string()
                .contains("reminder action requires --message or --prompt")
        );
    }

    #[test]
    fn parse_action_shell_requires_program_and_collects_args() {
        let empty = args(&[]);
        let err = parse_action(&Opts::parse(&empty), "shell", String::new()).unwrap_err();
        assert!(err.to_string().contains("shell action requires --program"));

        let full = args(&["--program", "/bin/echo", "--arg", "a", "--arg", "b"]);
        assert_eq!(
            parse_action(&Opts::parse(&full), "shell", String::new()).unwrap(),
            JobAction::Shell {
                program: "/bin/echo".to_string(),
                args: vec!["a".to_string(), "b".to_string()],
                env: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn parse_action_codex_exec_collects_extra_args() {
        let full = args(&["--model", "gpt-5.5", "--extra-arg", "--json"]);
        assert_eq!(
            parse_action(&Opts::parse(&full), "codex-exec", "do it".to_string()).unwrap(),
            JobAction::CodexExec {
                prompt: "do it".to_string(),
                model: Some("gpt-5.5".to_string()),
                extra_args: vec!["--json".to_string()],
                env: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn parse_action_codex_turn_defaults() {
        let empty = args(&[]);
        assert_eq!(
            parse_action(&Opts::parse(&empty), "codex-turn", "p".to_string()).unwrap(),
            JobAction::CodexTurn {
                prompt: "p".to_string(),
                model: None,
                session_state: None,
                approval_policy: None,
                session_strategy: SessionStrategy::PerInvocation,
                interactive: None,
            }
        );
    }

    #[test]
    fn parse_action_codex_turn_honours_strategy_and_approval() {
        let full = args(&[
            "--session-strategy",
            "persistent",
            "--approval",
            "never",
            "--model",
            "gpt-5.5",
        ]);
        let action = parse_action(&Opts::parse(&full), "codex-turn", "p".to_string()).unwrap();
        let JobAction::CodexTurn {
            model,
            approval_policy,
            session_strategy,
            interactive,
            ..
        } = action
        else {
            panic!("expected codex-turn");
        };
        assert_eq!(model.as_deref(), Some("gpt-5.5"));
        assert_eq!(approval_policy, Some(ApprovalPolicySetting::Never));
        assert_eq!(session_strategy, SessionStrategy::Persistent);
        assert_eq!(interactive, None);

        let bad = args(&["--session-strategy", "sticky"]);
        let err = parse_action(&Opts::parse(&bad), "codex-turn", "p".to_string()).unwrap_err();
        assert!(err.to_string().contains("invalid --session-strategy"));

        let bad = args(&["--approval", "sometimes"]);
        let err = parse_action(&Opts::parse(&bad), "codex-turn", "p".to_string()).unwrap_err();
        assert!(err.to_string().contains("invalid --approval"));
    }

    #[test]
    fn parse_action_rejects_unknown_action() {
        let empty = args(&[]);
        let err = parse_action(&Opts::parse(&empty), "warp", String::new()).unwrap_err();
        assert!(err.to_string().contains("unsupported --action `warp`"));
    }

    #[test]
    fn parse_interactive_absent_flag_is_none() {
        let empty = args(&[]);
        assert_eq!(parse_interactive(&Opts::parse(&empty)).unwrap(), None);
    }

    #[test]
    fn parse_interactive_flag_alone_uses_wire_defaults() {
        let flag = args(&["--interactive"]);
        assert_eq!(
            parse_interactive(&Opts::parse(&flag)).unwrap(),
            Some(store::InteractiveSpec::default())
        );
    }

    #[test]
    fn parse_interactive_honours_explicit_values_and_rejects_bad_numbers() {
        let full = args(&[
            "--interactive",
            "--reply-ttl",
            "600",
            "--end-signal",
            "<<<DONE>>>",
            "--max-rounds",
            "3",
        ]);
        assert_eq!(
            parse_interactive(&Opts::parse(&full)).unwrap(),
            Some(store::InteractiveSpec {
                reply_ttl_secs: 600,
                end_signal: "<<<DONE>>>".to_string(),
                max_rounds_hard_cap: 3,
            })
        );

        let bad_ttl = args(&["--interactive", "--reply-ttl", "soon"]);
        let err = parse_interactive(&Opts::parse(&bad_ttl)).unwrap_err();
        assert!(err.to_string().contains("invalid --reply-ttl"));

        let bad_rounds = args(&["--interactive", "--max-rounds", "many"]);
        let err = parse_interactive(&Opts::parse(&bad_rounds)).unwrap_err();
        assert!(err.to_string().contains("invalid --max-rounds"));
    }
}
