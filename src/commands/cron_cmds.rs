use super::*;

pub(super) async fn handle_cron(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    let locale = lang.as_str();
    let Some(subcommand) = args.first().copied() else {
        return Ok(CommandOutcome::reply_t("commands.cron.usage", locale));
    };
    match subcommand {
        "list" | "ls" => {
            let mut jobs = session.list_cron_jobs().await?;
            jobs.retain(|job| job.owner_openid == openid);
            jobs.sort_by_key(|job| job.next_run_at);
            if jobs.is_empty() {
                return Ok(CommandOutcome::reply_t("commands.cron.empty", locale));
            }
            let mut text = t!("commands.cron.list_header", locale = locale).into_owned();
            for job in jobs {
                text.push_str(&format!(
                    "\n{}  {}  next={}  runs={}  failures={}  {}",
                    job.id,
                    if job.disabled { "disabled" } else { "enabled" },
                    fmt_rfc3339_or(job.next_run_at, "-"),
                    job.run_count,
                    job.failure_streak,
                    job.title
                ));
            }
            Ok(CommandOutcome::reply(text))
        }
        "pause" | "resume" | "rm" | "remove" | "run-now" | "tail" => {
            // These are user-input mistakes, not internal failures: return a
            // reply like the sibling not_owned/unknown cases. An Err here
            // propagates to the gateway task where it is only warn!-logged, so
            // the user would get no response at all.
            let Some(id) = args.get(1) else {
                return Ok(CommandOutcome::reply(t!(
                    "commands.cron.requires_job_id",
                    subcommand = subcommand,
                    locale = locale
                )));
            };
            let Some(job) = session.get_cron_job(id).await? else {
                return Ok(CommandOutcome::reply(t!(
                    "commands.cron.not_found",
                    id = id,
                    locale = locale
                )));
            };
            if job.owner_openid != openid {
                return Ok(CommandOutcome::reply_t("commands.cron.not_owned", locale));
            }
            match subcommand {
                "pause" => {
                    session
                        .update_cron_job(id, |job| {
                            job.disabled = true;
                            Ok(())
                        })
                        .await?;
                    Ok(CommandOutcome::reply(t!(
                        "commands.cron.paused",
                        title = job.title.as_str(),
                        locale = locale
                    )))
                }
                "resume" => {
                    session
                        .update_cron_job(id, |job| {
                            job.disabled = false;
                            job.next_run_at = crate::scheduler::next_after(&job.kind, Utc::now())?;
                            if job.next_run_at.is_none()
                                && matches!(job.kind, crate::model::cron::CronKind::OneShot { .. })
                            {
                                job.run_now_at = Some(Utc::now());
                            }
                            Ok(())
                        })
                        .await?;
                    Ok(CommandOutcome::reply(t!(
                        "commands.cron.resumed",
                        title = job.title.as_str(),
                        locale = locale
                    )))
                }
                "rm" | "remove" => {
                    session.remove_cron_job(id).await?;
                    crate::scheduler::remove_job_files(
                        session.data_dir(),
                        session.codex_home(),
                        id,
                        false,
                    )
                    .await?;
                    Ok(CommandOutcome::reply(t!(
                        "commands.cron.removed",
                        title = job.title.as_str(),
                        locale = locale
                    )))
                }
                "run-now" => {
                    session
                        .update_cron_job(id, |job| {
                            job.run_now_at = Some(Utc::now());
                            Ok(())
                        })
                        .await?;
                    Ok(CommandOutcome::reply(t!(
                        "commands.cron.run_now",
                        title = job.title.as_str(),
                        locale = locale
                    )))
                }
                "tail" => {
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
                        return Ok(CommandOutcome::reply_t("commands.cron.no_logs", locale));
                    };
                    let raw = std::fs::read_to_string(last.path())?;
                    let preview = tail_chars(&raw, 3500);
                    Ok(CommandOutcome::reply(t!(
                        "commands.cron.tail_header",
                        path = last.path().display(),
                        preview = preview,
                        locale = locale
                    )))
                }
                _ => unreachable!(),
            }
        }
        _ => Ok(CommandOutcome::reply_t("commands.cron.unknown", locale)),
    }
}

pub(super) fn tail_chars(raw: &str, max_chars: usize) -> String {
    let count = raw.chars().count();
    if count <= max_chars {
        return raw.to_string();
    }
    raw.chars()
        .skip(count.saturating_sub(max_chars))
        .collect::<String>()
}
