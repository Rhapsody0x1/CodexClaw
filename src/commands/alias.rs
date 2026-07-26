use super::*;

pub(super) const MAX_ALIAS_DEPTH: usize = 3;

pub(super) const PROTECTED_COMMANDS: &[&str] = &[
    "help",
    "lang",
    "model",
    "fast",
    "context",
    "reasoning",
    "verbose",
    "status",
    "session",
    "sessions",
    "import",
    "new",
    "bg",
    "fg",
    "resume",
    "loadbg",
    "save",
    "rename",
    "stop",
    "interrupt",
    "compact",
    "self-update",
    "alias",
    "cron",
    "back",
    "approvals",
    "plan",
    "approve",
    "approve-session",
    "deny",
    "cancel",
    "execute-plan",
    "keep-planning",
    "cancel-plan",
    "retry",
    // Chinese aliases also protected
    "帮助",
    "状态",
    "会话",
    "模型",
    "快速",
    "上下文",
    "思考",
    "别名",
    "语言",
    "返回",
    "导入",
    "恢复",
    "载入后台",
    "后台",
    "前台",
    "保存",
    "新建",
    "停止",
    "中断",
    "压缩",
    "自更新",
    "详细",
    "重命名",
    "审批",
    "定时",
    "计划",
    "同意",
    "同意本会话",
    "拒绝",
    "取消",
    "实施",
    "继续规划",
    "取消计划",
    "重试",
];

pub(super) async fn expand_alias(
    alias: &CommandAlias,
    ctx: CmdCtx<'_>,
    alias_depth: usize,
) -> Result<CommandOutcome> {
    let lang = user_locale(ctx.session, ctx.openid).await;
    if alias_depth >= MAX_ALIAS_DEPTH {
        return Ok(CommandOutcome::reply(t!(
            "commands.alias.too_deep",
            max = MAX_ALIAS_DEPTH,
            locale = lang.as_str()
        )));
    }
    let mut parts: Vec<String> = Vec::new();
    parts.push(
        t!(
            "commands.alias.executed_header",
            name = alias.name.as_str(),
            locale = lang.as_str()
        )
        .into_owned(),
    );
    for step in &alias.commands {
        let outcome = maybe_handle_command_inner(step, ctx, alias_depth + 1).await?;
        match outcome {
            CommandOutcome::Reply(reply) => parts.push(reply.text),
            CommandOutcome::Continue => {
                parts.push(
                    t!(
                        "commands.alias.skipped_non_command",
                        step = step,
                        locale = lang.as_str()
                    )
                    .into_owned(),
                );
            }
            // Cancel/Stop terminate the alias with the collected output
            // prepended; every other outcome passes through untouched.
            other => {
                return Ok(other.map_text(|msg| {
                    parts.push(msg);
                    parts.join("\n")
                }));
            }
        }
    }
    Ok(CommandOutcome::reply(parts.join("\n")))
}

pub(super) async fn handle_alias(args: &[&str], ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    let locale = lang.as_str();

    let show_list = || async {
        let aliases = session.list_command_aliases(openid).await?;
        if aliases.is_empty() {
            return Ok::<CommandOutcome, anyhow::Error>(CommandOutcome::reply_t(
                "commands.alias.empty",
                locale,
            ));
        }
        let mut lines = vec![t!("commands.alias.list_header", locale = locale).into_owned()];
        for alias in aliases {
            let steps = alias.commands.join(" | ");
            lines.push(
                t!(
                    "commands.alias.list_item",
                    name = alias.name.as_str(),
                    steps = steps.as_str(),
                    locale = locale
                )
                .into_owned(),
            );
        }
        Ok(CommandOutcome::reply(lines.join("\n")))
    };

    if args.is_empty() {
        return show_list().await;
    }

    match args[0].to_ascii_lowercase().as_str() {
        "list" | "ls" => show_list().await,
        "remove" | "rm" | "delete" | "del" => {
            let Some(raw_name) = args.get(1) else {
                return Ok(CommandOutcome::reply_t("commands.alias.usage", locale));
            };
            let name = raw_name.trim_start_matches('/').trim().to_ascii_lowercase();
            let removed = session.remove_command_alias(openid, &name).await?;
            let key = if removed {
                "commands.alias.removed"
            } else {
                "commands.alias.not_found"
            };
            Ok(CommandOutcome::reply(t!(key, name = name, locale = locale)))
        }
        "add" => {
            let Some(raw_name) = args.get(1) else {
                return Ok(CommandOutcome::reply_t("commands.alias.usage", locale));
            };
            let Ok(name) = normalize_command_alias_name(raw_name) else {
                return Ok(CommandOutcome::reply_t(
                    "commands.alias.invalid_name",
                    locale,
                ));
            };
            if PROTECTED_COMMANDS.contains(&name.as_str()) {
                return Ok(CommandOutcome::reply(t!(
                    "commands.alias.protected",
                    name = name.as_str(),
                    locale = locale
                )));
            }
            if args.len() < 3 {
                return Ok(CommandOutcome::reply_t(
                    "commands.alias.empty_steps",
                    locale,
                ));
            }
            let joined = args[2..].join(" ");
            let commands: Vec<String> = joined
                .split('|')
                .map(|piece| piece.trim().to_string())
                .filter(|piece| !piece.is_empty())
                .collect();
            if commands.is_empty() {
                return Ok(CommandOutcome::reply_t(
                    "commands.alias.empty_steps",
                    locale,
                ));
            }
            let alias = CommandAlias {
                name: name.clone(),
                commands: commands.clone(),
                created_at: Utc::now(),
            };
            session.add_command_alias(openid, alias).await?;
            Ok(CommandOutcome::reply(t!(
                "commands.alias.added",
                name = name.as_str(),
                count = commands.len(),
                locale = locale
            )))
        }
        _ => Ok(CommandOutcome::reply_t("commands.alias.usage", locale)),
    }
}

pub(super) fn normalize_command_alias_name(input: &str) -> Result<String> {
    let raw = input.trim();
    let normalized = raw.to_ascii_lowercase();
    let is_valid = !raw.starts_with('/')
        && !normalized.is_empty()
        && normalized.chars().count() <= 20
        && !normalized.contains('|');
    anyhow::ensure!(is_valid, "invalid alias");
    Ok(normalized)
}

pub(super) fn canonicalize_core_command(command: &str) -> &str {
    match command {
        "/帮助" => "/help",
        "/状态" => "/status",
        "/会话" => "/sessions",
        "/模型" => "/model",
        "/快速" => "/fast",
        "/上下文" => "/context",
        "/思考" => "/reasoning",
        "/别名" => "/alias",
        "/语言" => "/lang",
        "/详细" => "/verbose",
        "/导入" => "/import",
        "/恢复" => "/resume",
        "/载入后台" => "/loadbg",
        "/后台" => "/bg",
        "/前台" => "/fg",
        "/保存" => "/save",
        "/新建" => "/new",
        "/停止" => "/stop",
        "/中断" => "/interrupt",
        "/压缩" => "/compact",
        "/自更新" => "/self-update",
        "/重命名" => "/rename",
        "/返回" => "/back",
        "/审批" => "/approvals",
        "/定时" => "/cron",
        "/计划" => "/plan",
        "/同意" => "/approve",
        "/同意本会话" => "/approve-session",
        "/拒绝" => "/deny",
        "/取消" => "/cancel",
        "/实施" => "/execute-plan",
        "/继续规划" => "/keep-planning",
        "/取消计划" => "/cancel-plan",
        "/重试" => "/retry",
        other => other,
    }
}
