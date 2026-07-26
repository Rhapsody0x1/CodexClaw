use super::*;

use crate::model::cron::CronJob;

pub(super) async fn handle_cron(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let display_tz = ctx.display_tz;
    let lang = user_locale(session, openid).await;
    let locale = lang.as_str();
    let Some(subcommand) = args.first().copied() else {
        return Ok(CommandOutcome::reply_t("commands.cron.usage", locale));
    };
    match subcommand {
        "list" | "ls" => {
            let mut jobs = session.list_cron_jobs().await?;
            jobs.retain(|job| job.owner_openid == openid);
            // Upcoming runs first (soonest on top); paused / exhausted jobs
            // sink to the bottom instead of leading the list.
            jobs.sort_by_key(|job| match (job.disabled, job.next_run_at) {
                (false, Some(next)) => (0, Some(next)),
                (false, None) => (1, None),
                (true, next) => (2, next),
            });
            if jobs.is_empty() {
                return Ok(CommandOutcome::reply_t("commands.cron.empty", locale));
            }
            let now = chrono::Utc::now();
            let mut text = t!("commands.cron.list_header", locale = locale).into_owned();
            let mut view_ids = Vec::new();
            for (index, job) in jobs.iter().enumerate() {
                let state = if job.disabled {
                    t!("commands.cron.state_paused", locale = locale).into_owned()
                } else {
                    String::new()
                };
                text.push_str(&format!("\n{}. {}{}", index + 1, job.title, state));
                let mut meta = Vec::new();
                if !job.disabled
                    && let Some(next) = job.next_run_at
                {
                    meta.push(
                        t!(
                            "commands.cron.row_next",
                            next = crate::util::time::fmt_next(next, now, display_tz, locale),
                            locale = locale
                        )
                        .into_owned(),
                    );
                }
                if job.run_count > 0 {
                    meta.push(
                        t!(
                            "commands.cron.row_runs",
                            count = job.run_count,
                            locale = locale
                        )
                        .into_owned(),
                    );
                }
                if job.failure_streak > 0 {
                    meta.push(
                        t!(
                            "commands.cron.row_failures",
                            count = job.failure_streak,
                            locale = locale
                        )
                        .into_owned(),
                    );
                }
                if !meta.is_empty() {
                    text.push_str("\n   ");
                    text.push_str(&meta.join(" · "));
                }
                view_ids.push(job.id.clone());
            }
            text.push('\n');
            text.push('\n');
            text.push_str(&t!("commands.cron.list_footer", locale = locale));
            session.set_last_cron_view(openid, view_ids).await?;
            Ok(CommandOutcome::reply(text))
        }
        "pause" => {
            let (id, job) =
                match resolve_owned_job(subcommand, args, session, openid, locale).await? {
                    Ok(found) => found,
                    Err(reply) => return Ok(reply),
                };
            session
                .update_cron_job(&id, |job| {
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
            let (id, job) =
                match resolve_owned_job(subcommand, args, session, openid, locale).await? {
                    Ok(found) => found,
                    Err(reply) => return Ok(reply),
                };
            session
                .update_cron_job(&id, |job| {
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
            let (id, job) =
                match resolve_owned_job(subcommand, args, session, openid, locale).await? {
                    Ok(found) => found,
                    Err(reply) => return Ok(reply),
                };
            session.remove_cron_job(&id).await?;
            crate::scheduler::remove_job_files(
                session.data_dir(),
                session.codex_home(),
                &id,
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
            let (id, job) =
                match resolve_owned_job(subcommand, args, session, openid, locale).await? {
                    Ok(found) => found,
                    Err(reply) => return Ok(reply),
                };
            session
                .update_cron_job(&id, |job| {
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
            let (_, job) =
                match resolve_owned_job(subcommand, args, session, openid, locale).await? {
                    Ok(found) => found,
                    Err(reply) => return Ok(reply),
                };
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
                title = job.title.as_str(),
                preview = preview,
                locale = locale
            )))
        }
        _ => Ok(CommandOutcome::reply_t("commands.cron.unknown", locale)),
    }
}

/// Shared validation for the per-job `/cron` subcommands: require a job id
/// argument, look the job up, and require ownership. The inner `Err` carries
/// the user-facing reply to short-circuit with — these are user-input
/// mistakes, not internal failures: an outer `Err` would propagate to the
/// gateway task where it is only warn!-logged, so the user would get no
/// response at all.
async fn resolve_owned_job(
    subcommand: &str,
    args: &[&str],
    session: &SessionStore,
    openid: &str,
    locale: &str,
) -> Result<std::result::Result<(String, CronJob), CommandOutcome>> {
    let Some(selector) = args.get(1).copied() else {
        return Ok(Err(CommandOutcome::reply(t!(
            "commands.cron.requires_job_id",
            subcommand = subcommand,
            locale = locale
        ))));
    };
    // Row numbers refer to the snapshot the user last saw via `/cron list`,
    // so later list changes cannot shift what a number means.
    let id = if selector.chars().all(|c| c.is_ascii_digit()) {
        let view = session.last_cron_view(openid).await?;
        let index: usize = selector.parse().unwrap_or(0);
        match index.checked_sub(1).and_then(|i| view.get(i)) {
            Some(id) => id.clone(),
            None => {
                return Ok(Err(CommandOutcome::reply_t(
                    "commands.cron.stale_index",
                    locale,
                )));
            }
        }
    } else {
        selector.to_string()
    };
    let job = match session.get_cron_job(&id).await? {
        Some(job) => Some(job),
        // Fall back to a unique prefix match over the user's own jobs, so a
        // short ID fragment works without pasting the full ULID.
        None if id.len() >= 4 => {
            let jobs = session.list_cron_jobs().await?;
            let mut matches = jobs
                .into_iter()
                .filter(|job| job.owner_openid == openid && job.id.starts_with(&id));
            match (matches.next(), matches.next()) {
                (Some(job), None) => Some(job),
                _ => None,
            }
        }
        None => None,
    };
    let Some(job) = job else {
        return Ok(Err(CommandOutcome::reply(t!(
            "commands.cron.not_found",
            id = selector,
            locale = locale
        ))));
    };
    if job.owner_openid != openid {
        return Ok(Err(CommandOutcome::reply_t(
            "commands.cron.not_owned",
            locale,
        )));
    }
    let id = job.id.clone();
    Ok(Ok((id, job)))
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
