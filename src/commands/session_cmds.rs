use super::*;

pub(super) async fn handle_new(raw_args: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        default_model,
        runtime_profile,
        ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    let moved = if raw_args.trim().is_empty() {
        session.new_foreground(openid).await?
    } else {
        let workspace_dir = resolve_new_workspace(raw_args, &snapshot, session);
        session
            .new_foreground_in_workspace(openid, &workspace_dir)
            .await?
    };
    let parked = moved
        .parked_alias
        .map(|alias| {
            let mut line = t!(
                "commands.new.parked",
                alias = alias.as_str(),
                locale = lang.as_str()
            )
            .into_owned();
            line.push('\n');
            line
        })
        .unwrap_or_default();
    let snapshot = session.snapshot_for_user(openid).await?;
    let mut lines = vec![if raw_args.trim().is_empty() {
        t!("commands.new.created_temp", locale = lang.as_str()).into_owned()
    } else {
        t!(
            "commands.new.created_with_workspace",
            dir = snapshot.foreground.workspace_dir.display().to_string(),
            locale = lang.as_str()
        )
        .into_owned()
    }];
    lines.push(format_effective_runtime_text(
        &snapshot,
        default_model,
        runtime_profile,
        None,
    ));
    Ok(CommandOutcome::reply(format!(
        "{parked}{}",
        lines.join("\n")
    )))
}

pub(super) async fn handle_bg(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    let moved = session
        .move_foreground_to_background(openid, args.first().copied())
        .await?;
    let text = if let Some(alias) = moved.parked_alias {
        format!(
            "{}\n{}",
            t!(
                "commands.bg.moved",
                alias = alias.as_str(),
                locale = lang.as_str()
            ),
            t!("commands.bg.nav_hint", locale = lang.as_str())
        )
    } else {
        t!("commands.bg.reset_empty", locale = lang.as_str()).into_owned()
    };
    Ok(CommandOutcome::reply(text))
}

pub(super) async fn handle_fg(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let snapshot = ctx.session.snapshot_for_user(ctx.openid).await?;
    let lang = snapshot.settings.language.clone();
    let Some(input) = args.first() else {
        // Bare `/fg` is the return half of the `/bg` ↔ `/fg` pair: jump
        // straight back to the most recently parked dialog (cd - semantics)
        // instead of asking the user to recall an alias.
        let Some(alias) = most_recent_background_alias(&snapshot) else {
            return Ok(CommandOutcome::reply(t!(
                "commands.fg.prompt_empty",
                locale = lang.as_str()
            )));
        };
        return interactive::switch_foreground(&alias, ctx, lang.as_str()).await;
    };
    // Direct arguments get the same fuzzy resolution as the pickers, so a
    // near-miss lands on the dialog instead of an error.
    let aliases: Vec<String> = snapshot.background.keys().cloned().collect();
    match interactive::fuzzy_match_unique(input, &aliases) {
        interactive::FuzzyOutcome::Exact(alias) => {
            interactive::switch_foreground(&alias, ctx, lang.as_str()).await
        }
        interactive::FuzzyOutcome::Ambiguous(matches) => Ok(CommandOutcome::reply(
            interactive::ambiguous_reply(lang.as_str(), input, &matches),
        )),
        interactive::FuzzyOutcome::None => {
            interactive::switch_foreground(input, ctx, lang.as_str()).await
        }
    }
}

/// Most recently parked alias that still exists, mirroring what `/stop`
/// restores (`background_order` is recency-ordered; `background` keys are
/// alphabetical and must not be used for this).
fn most_recent_background_alias(snapshot: &UserSessionState) -> Option<String> {
    snapshot
        .background_order
        .iter()
        .rev()
        .find(|alias| snapshot.background.contains_key(*alias))
        .cloned()
}

/// `/resume` and `/loadbg` walk the same project/session picker flow and only
/// differ in which `PendingSetting` variants they park and how the selected
/// session is finally applied (restore to foreground vs. load to background).
#[derive(Clone, Copy)]
pub(super) enum RestoreMode {
    Resume,
    Loadbg,
}

impl RestoreMode {
    pub(super) fn projects_pending(self) -> PendingSetting {
        match self {
            RestoreMode::Resume => PendingSetting::ResumeProjects,
            RestoreMode::Loadbg => PendingSetting::LoadbgProjects,
        }
    }

    pub(super) fn sessions_pending(
        self,
        project_key: String,
        page: usize,
        alias: Option<String>,
    ) -> PendingSetting {
        match self {
            RestoreMode::Resume => PendingSetting::ResumeSessions { project_key, page },
            RestoreMode::Loadbg => PendingSetting::LoadbgSessions {
                project_key,
                page,
                alias,
            },
        }
    }
}

pub(super) async fn handle_restore(
    mode: RestoreMode,
    args: &[&str],
    ctx: CmdCtx<'_>,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    if args.is_empty() {
        return interactive::enter_restore_projects_prompt(mode, ctx, lang.as_str()).await;
    }
    let selector = args[0];
    // Try project selector first (if we have a recent projects view), otherwise
    // fall back to session selector (legacy one-shot behavior).
    let projects_view = session.last_projects_view(openid).await?;
    if !projects_view.is_empty()
        && let Ok(project_key) = resolve_project_selector(selector, &projects_view, lang.as_str())
    {
        let page = args
            .get(1)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        return interactive::enter_restore_sessions_prompt(
            mode,
            ctx,
            project_key,
            page,
            None,
            lang.as_str(),
        )
        .await;
    }
    let sessions = session.list_disk_sessions(SessionListScope::All).await?;
    let target = resolve_selector(
        selector,
        &sessions,
        &session.last_sessions_view(openid).await?,
        lang.as_str(),
    )?;
    match mode {
        RestoreMode::Resume => interactive::execute_resume(ctx, &target).await,
        RestoreMode::Loadbg => {
            interactive::execute_loadbg(ctx, &target, args.get(1).copied()).await
        }
    }
}

pub(super) async fn handle_save(ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    let changed = session.save_foreground(openid).await?;
    let key = if changed {
        "commands.save.updated"
    } else {
        "commands.save.already_saved"
    };
    Ok(CommandOutcome::reply_t(key, lang.as_str()))
}

pub(super) async fn handle_rename(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    if args.len() != 2 {
        return Ok(CommandOutcome::reply_t(
            "commands.rename.usage",
            lang.as_str(),
        ));
    }
    session
        .rename_background_alias(openid, args[0], args[1])
        .await?;
    Ok(CommandOutcome::reply(t!(
        "commands.rename.renamed",
        old = args[0],
        new = args[1],
        locale = lang.as_str()
    )))
}

pub(super) async fn handle_sessions(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    if args.is_empty() || is_scope_token(args[0]) {
        let scope = if args.is_empty() {
            SessionListScope::All
        } else {
            parse_scope(args[0], lang.as_str())?
        };
        let sessions = session.list_disk_sessions(scope).await?;
        let projects = collect_projects(&sessions);
        let (text, project_keys) = format_projects_list(
            &SESSIONS_LIST_KEYS,
            &projects,
            lang.as_str(),
            ctx.display_tz,
            ctx.session.attachment_workspace_dir(),
        );
        let has_projects = !project_keys.is_empty();
        session.set_last_projects_view(openid, project_keys).await?;
        session.set_last_sessions_view(openid, Vec::new()).await?;
        if has_projects {
            session
                .set_pending_setting(openid, Some(PendingSetting::SessionsProjects))
                .await?;
        }
        return Ok(CommandOutcome::reply(text));
    }

    let selector = args[0];
    let page = args
        .get(1)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    let project_key = resolve_project_selector(
        selector,
        &session.last_projects_view(openid).await?,
        lang.as_str(),
    )?;
    let (scope, project_path) = decode_project_key(&project_key)?;
    let all_sessions = session.list_disk_sessions(scope).await?;
    let sessions = all_sessions
        .into_iter()
        .filter(|item| item.cwd.display().to_string() == project_path)
        .collect::<Vec<_>>();
    let (text, ids) = format_project_sessions_page(
        &SESSIONS_LIST_KEYS,
        &project_path,
        &sessions,
        page,
        lang.as_str(),
        ctx.display_tz,
        ctx.session.attachment_workspace_dir(),
    );
    session.set_last_sessions_view(openid, ids).await?;
    session
        .set_pending_setting(
            openid,
            Some(PendingSetting::SessionsSessions {
                project_key: project_key.clone(),
                page,
            }),
        )
        .await?;
    Ok(CommandOutcome::reply(text))
}

pub(super) async fn handle_import(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    let all = session.list_importable_sessions()?;
    let last_session_view = session.last_import_sessions_view(openid).await?;
    if let Some(selector) = args.first()
        && !last_session_view.is_empty()
        && let Ok(target) = resolve_selector(selector, &all, &last_session_view, lang.as_str())
    {
        return import_and_reply(session, openid, &target, lang.as_str()).await;
    }

    if args.is_empty() {
        let projects = collect_projects(&all);
        let (text, project_keys) = format_projects_list(
            &IMPORT_LIST_KEYS,
            &projects,
            lang.as_str(),
            ctx.display_tz,
            ctx.session.attachment_workspace_dir(),
        );
        let has_projects = !project_keys.is_empty();
        session
            .set_last_import_projects_view(openid, project_keys)
            .await?;
        session
            .set_last_import_sessions_view(openid, Vec::new())
            .await?;
        if has_projects {
            session
                .set_pending_setting(openid, Some(PendingSetting::ImportProjects))
                .await?;
        }
        return Ok(CommandOutcome::reply(text));
    }

    let selector = args[0];
    let page = args
        .get(1)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    let import_projects = session.last_import_projects_view(openid).await?;
    let project_key = match resolve_project_selector(selector, &import_projects, lang.as_str()) {
        Ok(value) => value,
        Err(_) => {
            let target = resolve_selector(selector, &all, &last_session_view, lang.as_str())?;
            return import_and_reply(session, openid, &target, lang.as_str()).await;
        }
    };
    let (_, project_path) = decode_project_key(&project_key)?;
    let project_sessions = all
        .into_iter()
        .filter(|item| item.cwd.display().to_string() == project_path)
        .collect::<Vec<_>>();
    let (text, ids) = format_project_sessions_page(
        &IMPORT_LIST_KEYS,
        &project_path,
        &project_sessions,
        page,
        lang.as_str(),
        ctx.display_tz,
        ctx.session.attachment_workspace_dir(),
    );
    session.set_last_import_sessions_view(openid, ids).await?;
    session
        .set_pending_setting(
            openid,
            Some(PendingSetting::ImportSessions {
                project_key: project_key.clone(),
                page,
            }),
        )
        .await?;
    Ok(CommandOutcome::reply(text))
}

/// Imports `target` and renders the localized result reply. Shared by the two
/// direct-selector paths in `handle_import` and by the interactive
/// `consume_import_sessions` flow.
pub(super) async fn import_and_reply(
    session: &SessionStore,
    openid: &str,
    target: &DiskSessionMeta,
    locale: &str,
) -> Result<CommandOutcome> {
    let result = session.import_disk_session(target).await?;
    let profile = result.profile;
    let action = if result.copied {
        t!("commands.import.imported", locale = locale)
    } else {
        t!("commands.import.refreshed", locale = locale)
    };
    session.set_pending_setting(openid, None).await?;
    Ok(CommandOutcome::reply(t!(
        "commands.import.result",
        action = action.as_ref(),
        summary = session_summary(target, locale),
        workspace = profile.workspace_dir.display().to_string(),
        model = compact_imported_profile_summary(&profile, locale),
        locale = locale
    )))
}

pub(super) async fn handle_stop(ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        default_model,
        runtime_profile,
        ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    let locale = lang.as_str();
    let result = session.stop_foreground(openid).await?;
    let summary = if let Some(alias) = result.restored_alias.as_deref() {
        let snapshot = session.snapshot_for_user(openid).await?;
        let preview = foreground_last_user_message(session, &snapshot).await?;
        let prefix_key = if !result.had_session {
            None
        } else if result.saved {
            Some("commands.stop.ended_saved")
        } else if result.dropped_unsaved {
            Some("commands.stop.ended_dropped")
        } else {
            Some("commands.stop.ended_plain")
        };
        let header = if let Some(key) = prefix_key {
            let prefix = t!(key, locale = locale);
            t!(
                "commands.stop.ended_restored",
                prefix = prefix.as_ref(),
                alias = alias,
                locale = locale
            )
            .into_owned()
        } else {
            t!(
                "commands.stop.had_none_restored",
                alias = alias,
                locale = locale
            )
            .into_owned()
        };
        format!(
            "{header}\n{}",
            format_effective_runtime_text(
                &snapshot,
                default_model,
                runtime_profile,
                preview.as_deref()
            )
        )
    } else {
        let key = if !result.had_session {
            "commands.stop.had_none_reset"
        } else if result.saved {
            "commands.stop.ended_saved_new"
        } else if result.dropped_unsaved {
            "commands.stop.ended_dropped_new"
        } else {
            "commands.stop.ended_plain_new"
        };
        t!(key, locale = locale).into_owned()
    };
    Ok(CommandOutcome::StopCurrent(summary))
}

pub(super) async fn foreground_last_user_message(
    session: &SessionStore,
    snapshot: &UserSessionState,
) -> Result<Option<String>> {
    let Some(session_id) = snapshot.foreground.session_id.as_deref() else {
        return Ok(None);
    };
    let sessions = session.list_disk_sessions(SessionListScope::All).await?;
    Ok(sessions
        .into_iter()
        .find(|item| item.id == session_id)
        .and_then(|item| item.last_user_message))
}

pub(super) fn resolve_new_workspace(
    raw_args: &str,
    snapshot: &UserSessionState,
    session: &SessionStore,
) -> PathBuf {
    let candidate = PathBuf::from(raw_args.trim());
    if candidate.is_absolute() {
        return candidate;
    }
    let base = if snapshot.foreground.session_id.is_none()
        && !snapshot.foreground.saved
        && snapshot.foreground.profile.is_none()
        && snapshot
            .foreground
            .workspace_dir
            .starts_with(session.default_workspace_dir())
    {
        session.default_workspace_dir().to_path_buf()
    } else {
        snapshot.foreground.workspace_dir.clone()
    };
    base.join(candidate)
}
