use super::*;

/// Shared tail for the per-dialog profile settings (`model` / `reasoning` /
/// `context`), used by both the direct handlers and the interactive
/// consumers:
/// - unsaved foreground: the setting applies globally instead — return the
///   `SetGlobal*` outcome (clearing the pending picker first when the caller
///   is a consumer);
/// - saved foreground: apply to the active dialog profile, then re-snapshot
///   and render the localized "updated" reply.
pub(super) async fn apply_active_or_global<T, F, Fut, R>(
    ctx: CmdCtx<'_>,
    snapshot: &UserSessionState,
    clear_pending: bool,
    next: Option<T>,
    global: fn(Option<T>) -> CommandOutcome,
    set_active: F,
    updated: impl FnOnce(&UserSessionState) -> String,
) -> Result<CommandOutcome>
where
    F: FnOnce(Option<T>) -> Fut,
    Fut: Future<Output = Result<R>>,
{
    let CmdCtx {
        openid, session, ..
    } = ctx;
    if !snapshot.foreground.saved {
        if clear_pending {
            session.set_pending_setting(openid, None).await?;
        }
        return Ok(global(next));
    }
    set_active(next).await?;
    if clear_pending {
        session.set_pending_setting(openid, None).await?;
    }
    let snapshot = session.snapshot_for_user(openid).await?;
    Ok(CommandOutcome::reply(updated(&snapshot)))
}

pub(super) async fn handle_model(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        default_model,
        runtime_profile,
        is_busy,
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    if args
        .first()
        .is_some_and(|arg| arg.eq_ignore_ascii_case("status"))
    {
        let active_override = snapshot.effective_settings().model_override;
        return Ok(CommandOutcome::reply(t!(
            "commands.model.status",
            effective = effective_model(&snapshot, default_model, runtime_profile),
            override_value = active_override.as_deref().unwrap_or("inherit"),
            locale = lang.as_str()
        )));
    }
    if is_busy {
        return Ok(busy_reply(lang.as_str()));
    }
    if args.is_empty() {
        return interactive::enter_model_prompt(&snapshot, ctx, lang.as_str()).await;
    }
    let known_models =
        list_codex_model_entries(runtime_profile, &interactive::model_extras(&snapshot));
    let value = args.join(" ");
    let next = if matches!(value.as_str(), "default" | "inherit") {
        None
    } else if let Some(resolved) = interactive::resolve_model_input(&value, &known_models) {
        Some(resolved)
    } else {
        Some(value.clone())
    };
    apply_active_or_global(
        ctx,
        &snapshot,
        false,
        next,
        CommandOutcome::SetGlobalModel,
        |value| session.set_model_override_for_active(openid, value),
        |snap: &UserSessionState| {
            t!(
                "commands.model.updated",
                model = effective_model(snap, default_model, runtime_profile),
                locale = lang.as_str()
            )
            .into_owned()
        },
    )
    .await
}

pub(super) async fn handle_fast(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        runtime_profile,
        ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    if args.is_empty() {
        return interactive::enter_simple_prompt(
            ctx,
            lang.as_str(),
            "commands.fast.prompt_current",
            "commands.fast.prompt_header",
            ServiceTier::fast_label(runtime_profile.service_tier),
            PendingSetting::Fast,
        )
        .await;
    }
    if args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::reply(t!(
            "commands.fast.status",
            value = ServiceTier::fast_label(runtime_profile.service_tier),
            locale = lang.as_str()
        )));
    }
    // Same gate as /model, /reasoning and /context: the turn-end profile
    // write-back would silently clobber a mid-turn tier change.
    if ctx.is_busy {
        return Ok(busy_reply(lang.as_str()));
    }
    let value = args.join(" ");
    let next = interactive::resolve_fast_input(&value)
        .ok_or_else(|| anyhow!(t!("commands.fast.invalid", locale = lang.as_str()).into_owned()))?;
    Ok(CommandOutcome::SetGlobalFast(next))
}

pub(super) async fn handle_context(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        runtime_profile,
        is_busy,
        ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    if args
        .first()
        .is_some_and(|arg| arg.eq_ignore_ascii_case("status"))
    {
        return Ok(CommandOutcome::reply(t!(
            "commands.context.status",
            value = effective_context_label(&snapshot, runtime_profile),
            locale = lang.as_str()
        )));
    }
    if is_busy {
        return Ok(busy_reply(lang.as_str()));
    }
    if args.is_empty() {
        return interactive::enter_simple_prompt(
            ctx,
            lang.as_str(),
            "commands.context.prompt_current",
            "commands.context.prompt_header",
            effective_context_label(&snapshot, runtime_profile),
            PendingSetting::Context,
        )
        .await;
    }
    let value = args.join(" ");
    let next = interactive::resolve_context_input(&value).ok_or_else(|| {
        anyhow!(t!("commands.context.invalid", locale = lang.as_str()).into_owned())
    })?;
    apply_active_or_global(
        ctx,
        &snapshot,
        false,
        next,
        CommandOutcome::SetGlobalContext,
        |value| session.set_context_mode_for_active(openid, value),
        |snap: &UserSessionState| {
            t!(
                "commands.context.updated",
                value = effective_context_label(snap, runtime_profile),
                locale = lang.as_str()
            )
            .into_owned()
        },
    )
    .await
}

pub(super) async fn handle_reasoning(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        runtime_profile,
        is_busy,
        ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    if args
        .first()
        .is_some_and(|arg| arg.eq_ignore_ascii_case("status"))
    {
        return Ok(CommandOutcome::reply(t!(
            "commands.reasoning.status",
            value = effective_reasoning(&snapshot, runtime_profile),
            locale = lang.as_str()
        )));
    }
    if is_busy {
        return Ok(busy_reply(lang.as_str()));
    }
    if args.is_empty() {
        return interactive::enter_simple_prompt(
            ctx,
            lang.as_str(),
            "commands.reasoning.prompt_current",
            "commands.reasoning.prompt_header",
            effective_reasoning(&snapshot, runtime_profile),
            PendingSetting::Reasoning,
        )
        .await;
    }
    let value = args.join(" ");
    let next = interactive::resolve_reasoning_input(&value).ok_or_else(|| {
        anyhow!(t!("commands.reasoning.invalid", locale = lang.as_str()).into_owned())
    })?;
    apply_active_or_global(
        ctx,
        &snapshot,
        false,
        next,
        CommandOutcome::SetGlobalReasoning,
        |value| session.set_reasoning_for_active(openid, value),
        |snap: &UserSessionState| {
            t!(
                "commands.reasoning.updated",
                value = effective_reasoning(snap, runtime_profile),
                locale = lang.as_str()
            )
            .into_owned()
        },
    )
    .await
}

pub(super) async fn handle_verbose(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    if args.is_empty() {
        return interactive::enter_simple_prompt(
            ctx,
            lang.as_str(),
            "commands.verbose.prompt_current",
            "commands.verbose.prompt_header",
            if snapshot.settings.verbose {
                "on"
            } else {
                "off"
            },
            PendingSetting::Verbose,
        )
        .await;
    }
    if args[0].eq_ignore_ascii_case("status") {
        let key = if snapshot.settings.verbose {
            "commands.verbose.status_on"
        } else {
            "commands.verbose.status_off"
        };
        return Ok(CommandOutcome::reply_t(key, lang.as_str()));
    }
    let enabled = match args[0].to_ascii_lowercase().as_str() {
        "on" | "true" => true,
        "off" | "false" => false,
        _ => {
            return Ok(CommandOutcome::reply_t(
                "commands.verbose.invalid",
                lang.as_str(),
            ));
        }
    };
    session
        .update_settings_for_user(openid, |state| state.verbose = enabled)
        .await?;
    let key = if enabled {
        "commands.verbose.updated_on"
    } else {
        "commands.verbose.updated_off"
    };
    Ok(CommandOutcome::reply_t(key, lang.as_str()))
}

pub(super) async fn handle_approvals(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    if args.is_empty() {
        let snapshot = session.snapshot_for_user(openid).await?;
        let lang = snapshot.settings.language.clone();
        let zh = lang.starts_with("zh");
        let current = approval_label(snapshot.settings.approval_policy_override, zh);
        let header = if zh {
            format!("当前审批策略：{current}\n回复以下选项切换：")
        } else {
            format!("Current approval policy: {current}\nReply with:")
        };
        let body = if zh {
            "/approvals on           按需审批（默认）\n/approvals strict       严格（unless-trusted）\n/approvals off          关闭审批"
        } else {
            "/approvals on           on-request (default)\n/approvals strict       unless-trusted\n/approvals off          never ask"
        };
        session
            .set_pending_setting(openid, Some(PendingSetting::Approvals))
            .await?;
        return Ok(CommandOutcome::reply(format!("{header}\n{body}")));
    }
    handle_approvals_arg(&args.join(" "), openid, session).await
}

pub(super) async fn handle_approvals_arg(
    text: &str,
    openid: &str,
    session: &SessionStore,
) -> Result<CommandOutcome> {
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    let zh = lang.starts_with("zh");
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("status") || trimmed.is_empty() {
        let current = approval_label(snapshot.settings.approval_policy_override, zh);
        let msg = if zh {
            format!("当前审批策略：{current}")
        } else {
            format!("approval policy: {current}")
        };
        return Ok(CommandOutcome::reply(msg));
    }
    let next = ApprovalPolicySetting::parse(trimmed);
    let Some(next) = next else {
        let msg = if zh {
            format!("无法识别的审批选项：{trimmed}。可选值：on | strict | off")
        } else {
            format!("unrecognized approval option: {trimmed}. Try: on | strict | off")
        };
        return Ok(CommandOutcome::reply(msg));
    };
    session
        .update_settings_for_user(openid, |state| {
            state.approval_policy_override = Some(next);
        })
        .await?;
    session.set_pending_setting(openid, None).await?;
    let label = approval_label(Some(next), zh);
    let msg = if zh {
        format!("已切换审批策略：{label}")
    } else {
        format!("approval policy updated: {label}")
    };
    Ok(CommandOutcome::reply(msg))
}

pub(super) fn approval_label(setting: Option<ApprovalPolicySetting>, zh: bool) -> String {
    match setting {
        None => {
            if zh {
                "按需（on-request，默认）".to_string()
            } else {
                "on-request (default)".to_string()
            }
        }
        Some(s) => if zh { s.label_zh() } else { s.label_en() }.to_string(),
    }
}

pub(super) async fn handle_plan(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    if args.is_empty() {
        let snapshot = session.snapshot_for_user(openid).await?;
        let lang = snapshot.settings.language.clone();
        let zh = lang.starts_with("zh");
        let current = if snapshot.settings.plan_mode {
            if zh { "已开启" } else { "on" }
        } else if zh {
            "已关闭"
        } else {
            "off"
        };
        let header = if zh {
            format!("Plan 模式：{current}\n回复以下选项切换：")
        } else {
            format!("Plan mode: {current}\nReply with:")
        };
        let body = if zh {
            "/plan on    让 Codex 只读制定计划\n/plan off   恢复默认模式"
        } else {
            "/plan on    enter plan mode (read-only planning)\n/plan off   return to default mode"
        };
        session
            .set_pending_setting(openid, Some(PendingSetting::Plan))
            .await?;
        return Ok(CommandOutcome::reply(format!("{header}\n{body}")));
    }
    handle_plan_arg(&args.join(" "), openid, session).await
}

pub(super) async fn handle_execute_plan(ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    let zh = lang.starts_with("zh");
    if snapshot.settings.pending_plan.is_none() {
        let msg = if zh {
            "当前没有待执行的计划。"
        } else {
            "No pending plan to execute."
        };
        return Ok(CommandOutcome::reply(msg.to_string()));
    }
    session
        .update_settings_for_user(openid, |state| {
            state.plan_mode = false;
            state.pending_plan = None;
        })
        .await?;
    // Return a direct reply acknowledging the switch. The user's next QQ
    // message carries the plan verbatim into the follow-up turn; we only
    // surface a short confirmation here.
    let confirm = if zh {
        "已退出 Plan 模式并批准计划。你可以直接回复 “开始” 或描述下一步，我会按计划执行。"
    } else {
        "Plan approved. Reply with a follow-up (e.g. \"go\") and I'll implement the plan."
    };
    Ok(CommandOutcome::reply(confirm.to_string()))
}

pub(super) async fn handle_keep_planning(ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    let zh = lang.starts_with("zh");
    session
        .update_settings_for_user(openid, |state| {
            state.plan_mode = true;
            state.pending_plan = None;
        })
        .await?;
    let msg = if zh {
        "已保留 Plan 模式，继续打磨计划。下一条消息将继续规划。"
    } else {
        "Staying in plan mode. Next message continues planning."
    };
    Ok(CommandOutcome::reply(msg.to_string()))
}

pub(super) async fn handle_cancel_plan(ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    let zh = lang.starts_with("zh");
    session
        .update_settings_for_user(openid, |state| {
            state.pending_plan = None;
        })
        .await?;
    let msg = if zh {
        "已丢弃当前计划。"
    } else {
        "Pending plan discarded."
    };
    Ok(CommandOutcome::reply(msg.to_string()))
}

pub(super) async fn handle_plan_arg(
    text: &str,
    openid: &str,
    session: &SessionStore,
) -> Result<CommandOutcome> {
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    let zh = lang.starts_with("zh");
    let trimmed = text.trim().to_ascii_lowercase();
    let enabled = match trimmed.as_str() {
        "on" | "开" | "开启" | "true" | "yes" => true,
        "off" | "关" | "关闭" | "false" | "no" => false,
        "status" | "" => {
            let msg = if snapshot.settings.plan_mode {
                if zh {
                    "Plan 模式：已开启"
                } else {
                    "Plan mode: on"
                }
            } else if zh {
                "Plan 模式：已关闭"
            } else {
                "Plan mode: off"
            };
            return Ok(CommandOutcome::reply(msg.to_string()));
        }
        other => {
            let msg = if zh {
                format!("无法识别的 plan 选项：{other}。可选值：on | off")
            } else {
                format!("unrecognized plan option: {other}. Try: on | off")
            };
            return Ok(CommandOutcome::reply(msg));
        }
    };
    session
        .update_settings_for_user(openid, |state| state.plan_mode = enabled)
        .await?;
    session.set_pending_setting(openid, None).await?;
    let msg = if enabled {
        if zh {
            "已进入 Plan 模式。Codex 将在只读沙箱中先制定计划，随后发 <proposed_plan>，你可以用 /实施 批准执行。"
        } else {
            "Plan mode on. Codex will plan in a read-only sandbox, then emit <proposed_plan>. Approve with /execute-plan."
        }
    } else if zh {
        "已退出 Plan 模式。"
    } else {
        "Plan mode off."
    };
    Ok(CommandOutcome::reply(msg.to_string()))
}

pub(super) async fn handle_lang(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let current_lang = snapshot.settings.language.clone();
    if args.is_empty() {
        return interactive::enter_simple_prompt(
            ctx,
            current_lang.as_str(),
            "commands.lang.prompt_current",
            "commands.lang.prompt_header",
            current_lang.as_str(),
            PendingSetting::Lang,
        )
        .await;
    }
    if args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::reply(t!(
            "commands.lang.status",
            lang = current_lang.as_str(),
            locale = current_lang.as_str()
        )));
    }
    let requested = args[0];
    let normalized = normalize_lang(requested);
    if !is_supported_lang(requested) {
        return Ok(CommandOutcome::reply(t!(
            "commands.lang.unsupported",
            lang = requested,
            locale = current_lang.as_str()
        )));
    }
    session
        .update_settings_for_user(openid, |state| {
            state.language = normalized.to_string();
        })
        .await?;
    Ok(CommandOutcome::reply(t!(
        "commands.lang.updated",
        lang = normalized,
        locale = normalized
    )))
}
