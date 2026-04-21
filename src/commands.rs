use std::{collections::BTreeMap, future::Future, path::PathBuf, pin::Pin};

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use rust_i18n::t;

use crate::{
    codex::runtime::{CodexModelEntry, CodexRuntimeProfile, list_codex_model_entries},
    normalize_lang,
    session::{
        state::{
            CommandAlias, ContextMode, PendingSetting, ReasoningEffort, ServiceTier,
            UserSessionState,
        },
        store::{DiskSessionMeta, SessionListScope, SessionStore},
    },
};

const MAX_ALIAS_DEPTH: usize = 3;

const PROTECTED_COMMANDS: &[&str] = &[
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
    "back",
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
];

const PROJECT_KEY_SEP: char = '\u{1f}';

#[derive(Debug, Clone)]
pub struct CommandReply {
    pub text: String,
}

pub enum CommandOutcome {
    Reply(CommandReply),
    Continue,
    CancelCurrent(String),
    StopCurrent(String),
    Compact,
    SelfUpdate,
}

#[derive(Debug, Clone)]
struct ProjectBucket {
    path: String,
    count: usize,
    latest: Option<DateTime<Utc>>,
}

pub async fn maybe_handle_command(
    text: &str,
    openid: &str,
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> Result<CommandOutcome> {
    maybe_handle_command_inner(
        text,
        openid,
        session,
        default_model,
        runtime_profile,
        is_busy,
        0,
    )
    .await
}

fn maybe_handle_command_inner<'a>(
    text: &'a str,
    openid: &'a str,
    session: &'a SessionStore,
    default_model: &'a str,
    runtime_profile: &'a CodexRuntimeProfile,
    is_busy: bool,
    alias_depth: usize,
) -> Pin<Box<dyn Future<Output = Result<CommandOutcome>> + Send + 'a>> {
    Box::pin(async move {
        let snapshot = session.snapshot_for_user(openid).await?;
        let lang_string = snapshot.settings.language.clone();
        let locale = lang_string.as_str();
        let pending_before = snapshot.pending_setting.clone();
        let is_slash_input = text.trim_start().starts_with('/');

        // Plain text while in an interactive setting is consumed by the
        // pending handler — never forwarded to Codex.
        if !is_slash_input {
            if let Some(pending) = pending_before {
                return interactive::consume_pending_input(
                    pending,
                    text,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                )
                .await;
            }
            return Ok(CommandOutcome::Continue);
        }

        let trimmed = text.trim();
        let mut parts = trimmed.split_whitespace();
        let raw_command = parts.next().unwrap_or_default().to_ascii_lowercase();
        let command = canonicalize_core_command(&raw_command).to_string();
        let rest = parts.collect::<Vec<_>>();

        // /back is a global escape: exits the current interactive setting, or
        // politely reports that nothing interactive was in progress.
        if command.as_str() == "/back" {
            if let Some(pending) = pending_before {
                session.set_pending_setting(openid, None).await?;
                return Ok(CommandOutcome::Reply(CommandReply {
                    text: t!(
                        "commands.back.exited",
                        cmd = pending.command_name(locale),
                        locale = locale
                    )
                    .into_owned(),
                }));
            }
            return Ok(CommandOutcome::Reply(CommandReply {
                text: t!("commands.back.idle", locale = locale).into_owned(),
            }));
        }

        // Non-/back slash command while in an interactive setting: quietly
        // exit the pending state and prepend a notice to the eventual reply.
        let pending_exit_prefix = if let Some(pending) = pending_before {
            session.set_pending_setting(openid, None).await?;
            Some(
                t!(
                    "commands.back.exited",
                    cmd = pending.command_name(locale),
                    locale = locale
                )
                .into_owned(),
            )
        } else {
            None
        };

        let outcome_result: Result<CommandOutcome> = match command.as_str() {
            "/help" => Ok(CommandOutcome::Reply(CommandReply {
                text: help_text(&lang_string),
            })),
            "/lang" => handle_lang(&rest, openid, session).await,
            "/model" => {
                handle_model(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/fast" => {
                handle_fast(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/context" => {
                handle_context(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/reasoning" => {
                handle_reasoning(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/verbose" => {
                handle_verbose(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/status" => Ok(CommandOutcome::Reply(CommandReply {
                text: build_status_text(
                    &session.snapshot_for_user(openid).await?,
                    default_model,
                    runtime_profile,
                    is_busy,
                ),
            })),
            "/sessions" => {
                handle_sessions(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/import" => {
                handle_import(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/new" => {
                let raw_args = trimmed
                    .strip_prefix(command.as_str())
                    .unwrap_or_default()
                    .trim();
                handle_new(
                    raw_args,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/bg" => {
                handle_bg(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/fg" => {
                handle_fg(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/resume" => {
                handle_resume(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/loadbg" => {
                handle_loadbg(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/save" => handle_save(openid, session, default_model, runtime_profile, is_busy).await,
            "/rename" => {
                handle_rename(
                    &rest,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    is_busy,
                )
                .await
            }
            "/stop" => handle_stop(openid, session, default_model, runtime_profile).await,
            "/interrupt" => Ok(CommandOutcome::CancelCurrent(
                t!("errors.interrupt_requested", locale = locale).into_owned(),
            )),
            "/compact" => Ok(CommandOutcome::Compact),
            "/self-update" => Ok(CommandOutcome::SelfUpdate),
            "/alias" => handle_alias(&rest, openid, session).await,
            other => {
                let alias_name = other.trim_start_matches('/').to_ascii_lowercase();
                if !alias_name.is_empty()
                    && let Some(alias) = session.get_command_alias(openid, &alias_name).await?
                {
                    expand_alias(
                        &alias,
                        openid,
                        session,
                        default_model,
                        runtime_profile,
                        is_busy,
                        alias_depth,
                    )
                    .await
                } else {
                    Ok(CommandOutcome::Continue)
                }
            }
        };

        let outcome = outcome_result?;
        if let Some(prefix) = pending_exit_prefix {
            Ok(prepend_pending_exit(prefix, outcome))
        } else {
            Ok(outcome)
        }
    })
}

fn prepend_pending_exit(prefix: String, outcome: CommandOutcome) -> CommandOutcome {
    match outcome {
        CommandOutcome::Reply(reply) => CommandOutcome::Reply(CommandReply {
            text: format!("{prefix}\n\n{}", reply.text),
        }),
        CommandOutcome::CancelCurrent(msg) => {
            CommandOutcome::CancelCurrent(format!("{prefix}\n\n{msg}"))
        }
        CommandOutcome::StopCurrent(msg) => {
            CommandOutcome::StopCurrent(format!("{prefix}\n\n{msg}"))
        }
        CommandOutcome::Compact => CommandOutcome::Compact,
        CommandOutcome::SelfUpdate => CommandOutcome::SelfUpdate,
        CommandOutcome::Continue => CommandOutcome::Continue,
    }
}

async fn expand_alias(
    alias: &CommandAlias,
    openid: &str,
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
    alias_depth: usize,
) -> Result<CommandOutcome> {
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    if alias_depth >= MAX_ALIAS_DEPTH {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.alias.too_deep",
                max = MAX_ALIAS_DEPTH,
                locale = lang.as_str()
            )
            .into_owned(),
        }));
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
        let outcome = maybe_handle_command_inner(
            step,
            openid,
            session,
            default_model,
            runtime_profile,
            is_busy,
            alias_depth + 1,
        )
        .await?;
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
            CommandOutcome::CancelCurrent(msg) => {
                parts.push(msg);
                return Ok(CommandOutcome::CancelCurrent(parts.join("\n")));
            }
            CommandOutcome::StopCurrent(msg) => {
                parts.push(msg);
                return Ok(CommandOutcome::StopCurrent(parts.join("\n")));
            }
            CommandOutcome::Compact => return Ok(CommandOutcome::Compact),
            CommandOutcome::SelfUpdate => return Ok(CommandOutcome::SelfUpdate),
        }
    }
    Ok(CommandOutcome::Reply(CommandReply {
        text: parts.join("\n"),
    }))
}

async fn handle_alias(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
) -> Result<CommandOutcome> {
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    let locale = lang.as_str();

    let show_list = || async {
        let aliases = session.list_command_aliases(openid).await?;
        if aliases.is_empty() {
            return Ok::<CommandOutcome, anyhow::Error>(CommandOutcome::Reply(CommandReply {
                text: t!("commands.alias.empty", locale = locale).into_owned(),
            }));
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
        Ok(CommandOutcome::Reply(CommandReply {
            text: lines.join("\n"),
        }))
    };

    if args.is_empty() {
        return show_list().await;
    }

    match args[0].to_ascii_lowercase().as_str() {
        "list" | "ls" => show_list().await,
        "remove" | "rm" | "delete" | "del" => {
            let Some(raw_name) = args.get(1) else {
                return Ok(CommandOutcome::Reply(CommandReply {
                    text: t!("commands.alias.usage", locale = locale).into_owned(),
                }));
            };
            let name = raw_name.trim_start_matches('/').trim().to_ascii_lowercase();
            let removed = session.remove_command_alias(openid, &name).await?;
            let key = if removed {
                "commands.alias.removed"
            } else {
                "commands.alias.not_found"
            };
            Ok(CommandOutcome::Reply(CommandReply {
                text: t!(key, name = name, locale = locale).into_owned(),
            }))
        }
        "add" => {
            let Some(raw_name) = args.get(1) else {
                return Ok(CommandOutcome::Reply(CommandReply {
                    text: t!("commands.alias.usage", locale = locale).into_owned(),
                }));
            };
            let Ok(name) = normalize_command_alias_name(raw_name) else {
                return Ok(CommandOutcome::Reply(CommandReply {
                    text: t!("commands.alias.invalid_name", locale = locale).into_owned(),
                }));
            };
            if PROTECTED_COMMANDS.contains(&name.as_str()) {
                return Ok(CommandOutcome::Reply(CommandReply {
                    text: t!(
                        "commands.alias.protected",
                        name = name.as_str(),
                        locale = locale
                    )
                    .into_owned(),
                }));
            }
            if args.len() < 3 {
                return Ok(CommandOutcome::Reply(CommandReply {
                    text: t!("commands.alias.empty_steps", locale = locale).into_owned(),
                }));
            }
            let joined = args[2..].join(" ");
            let commands: Vec<String> = joined
                .split('|')
                .map(|piece| piece.trim().to_string())
                .filter(|piece| !piece.is_empty())
                .collect();
            if commands.is_empty() {
                return Ok(CommandOutcome::Reply(CommandReply {
                    text: t!("commands.alias.empty_steps", locale = locale).into_owned(),
                }));
            }
            let alias = CommandAlias {
                name: name.clone(),
                commands: commands.clone(),
                created_at: Utc::now(),
            };
            session.add_command_alias(openid, alias).await?;
            Ok(CommandOutcome::Reply(CommandReply {
                text: t!(
                    "commands.alias.added",
                    name = name.as_str(),
                    count = commands.len(),
                    locale = locale
                )
                .into_owned(),
            }))
        }
        _ => Ok(CommandOutcome::Reply(CommandReply {
            text: t!("commands.alias.usage", locale = locale).into_owned(),
        })),
    }
}

pub async fn handle_selector_callback(
    data: &str,
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> Result<Option<CommandOutcome>> {
    let _ = (data, session, default_model, runtime_profile, is_busy);
    Ok(None)
}

async fn handle_model(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    let known_models =
        list_codex_model_entries(runtime_profile, &interactive::model_extras(&snapshot));
    if args.is_empty() {
        return interactive::enter_model_prompt(
            &snapshot,
            openid,
            session,
            default_model,
            runtime_profile,
            lang.as_str(),
        )
        .await;
    }
    if args[0].eq_ignore_ascii_case("status") {
        let active_override = merged_settings(&snapshot).model_override;
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.model.status",
                effective = effective_model(&snapshot, default_model, runtime_profile),
                override_value = active_override.as_deref().unwrap_or("inherit"),
                locale = lang.as_str()
            )
            .into_owned(),
        }));
    }
    let value = args.join(" ");
    let next = if matches!(value.as_str(), "default" | "inherit") {
        None
    } else if let Some(resolved) = interactive::resolve_model_input(&value, &known_models) {
        Some(resolved)
    } else {
        Some(value.clone())
    };
    session
        .set_model_override_for_active(openid, next.clone())
        .await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: t!(
            "commands.model.updated",
            model = effective_model(&snapshot, default_model, runtime_profile),
            locale = lang.as_str()
        )
        .into_owned(),
    }))
}

async fn handle_fast(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    if args.is_empty() {
        return interactive::enter_fast_prompt(&snapshot, openid, session, runtime_profile).await;
    }
    if args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.fast.status",
                value = effective_fast_label(&snapshot, runtime_profile),
                locale = lang.as_str()
            )
            .into_owned(),
        }));
    }
    let value = args.join(" ");
    let next = interactive::resolve_fast_input(&value)
        .ok_or_else(|| anyhow!(t!("commands.fast.invalid", locale = lang.as_str()).into_owned()))?;
    session.set_service_tier_for_active(openid, next).await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: t!(
            "commands.fast.updated",
            value = effective_fast_label(&snapshot, runtime_profile),
            locale = lang.as_str()
        )
        .into_owned(),
    }))
}

async fn handle_context(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    if args.is_empty() {
        return interactive::enter_context_prompt(&snapshot, openid, session, runtime_profile)
            .await;
    }
    if args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.context.status",
                value = effective_context_label(&snapshot, runtime_profile),
                locale = lang.as_str()
            )
            .into_owned(),
        }));
    }
    let value = args.join(" ");
    let next = interactive::resolve_context_input(&value).ok_or_else(|| {
        anyhow!(t!("commands.context.invalid", locale = lang.as_str()).into_owned())
    })?;
    session.set_context_mode_for_active(openid, next).await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: t!(
            "commands.context.updated",
            value = effective_context_label(&snapshot, runtime_profile),
            locale = lang.as_str()
        )
        .into_owned(),
    }))
}

async fn handle_reasoning(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    if args.is_empty() {
        return interactive::enter_reasoning_prompt(&snapshot, openid, session, runtime_profile)
            .await;
    }
    if args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.reasoning.status",
                value = effective_reasoning(&snapshot, runtime_profile),
                locale = lang.as_str()
            )
            .into_owned(),
        }));
    }
    let value = args.join(" ");
    let next = interactive::resolve_reasoning_input(&value).ok_or_else(|| {
        anyhow!(t!("commands.reasoning.invalid", locale = lang.as_str()).into_owned())
    })?;
    session.set_reasoning_for_active(openid, next).await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: t!(
            "commands.reasoning.updated",
            value = effective_reasoning(&snapshot, runtime_profile),
            locale = lang.as_str()
        )
        .into_owned(),
    }))
}

async fn handle_verbose(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    _runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    if args.is_empty() {
        return interactive::enter_verbose_prompt(&snapshot, openid, session).await;
    }
    if args[0].eq_ignore_ascii_case("status") {
        let key = if snapshot.settings.verbose {
            "commands.verbose.status_on"
        } else {
            "commands.verbose.status_off"
        };
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(key, locale = lang.as_str()).into_owned(),
        }));
    }
    let enabled = match args[0].to_ascii_lowercase().as_str() {
        "on" | "true" => true,
        "off" | "false" => false,
        _ => {
            return Ok(CommandOutcome::Reply(CommandReply {
                text: t!("commands.verbose.invalid", locale = lang.as_str()).into_owned(),
            }));
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
    Ok(CommandOutcome::Reply(CommandReply {
        text: t!(key, locale = lang.as_str()).into_owned(),
    }))
}

async fn handle_new(
    raw_args: &str,
    openid: &str,
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
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
    Ok(CommandOutcome::Reply(CommandReply {
        text: format!("{parked}{}", lines.join("\n")),
    }))
}

async fn handle_bg(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    _runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    let moved = session
        .move_foreground_to_background(openid, args.first().copied())
        .await?;
    let text = if let Some(alias) = moved.parked_alias {
        t!(
            "commands.bg.moved",
            alias = alias.as_str(),
            locale = lang.as_str()
        )
        .into_owned()
    } else {
        t!("commands.bg.reset_empty", locale = lang.as_str()).into_owned()
    };
    Ok(CommandOutcome::Reply(CommandReply { text }))
}

async fn handle_fg(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let snapshot = session.snapshot_for_user(openid).await?;
    let lang = snapshot.settings.language.clone();
    if args.is_empty() {
        return interactive::enter_fg_prompt(&snapshot, openid, session).await;
    }
    interactive::switch_foreground(
        args[0],
        openid,
        session,
        default_model,
        runtime_profile,
        lang.as_str(),
    )
    .await
}

async fn handle_resume(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    if args.is_empty() {
        return interactive::enter_resume_projects_prompt(openid, session, lang.as_str()).await;
    }
    let selector = args[0];
    // Try project selector first (if we have a recent projects view), otherwise
    // fall back to session selector (legacy one-shot behavior).
    let projects_view = session.last_projects_view(openid).await?;
    if !projects_view.is_empty() {
        if let Ok(project_key) = resolve_project_selector(selector, &projects_view, lang.as_str()) {
            let page = args
                .get(1)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1);
            return interactive::enter_resume_sessions_prompt(
                openid,
                session,
                project_key,
                page,
                lang.as_str(),
            )
            .await;
        }
    }
    let sessions = session
        .list_disk_sessions(openid, SessionListScope::All)
        .await?;
    let target = resolve_selector(
        selector,
        &sessions,
        &session.last_sessions_view(openid).await?,
        lang.as_str(),
    )?;
    interactive::execute_resume(openid, session, default_model, runtime_profile, &target).await
}

async fn handle_loadbg(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    _runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    if args.is_empty() {
        return interactive::enter_loadbg_projects_prompt(openid, session, lang.as_str()).await;
    }
    let selector = args[0];
    let projects_view = session.last_projects_view(openid).await?;
    if !projects_view.is_empty() {
        if let Ok(project_key) = resolve_project_selector(selector, &projects_view, lang.as_str()) {
            let page = args
                .get(1)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1);
            return interactive::enter_loadbg_sessions_prompt(
                openid,
                session,
                project_key,
                page,
                None,
                lang.as_str(),
            )
            .await;
        }
    }
    let sessions = session
        .list_disk_sessions(openid, SessionListScope::All)
        .await?;
    let target = resolve_selector(
        selector,
        &sessions,
        &session.last_sessions_view(openid).await?,
        lang.as_str(),
    )?;
    interactive::execute_loadbg(openid, session, &target, args.get(1).copied()).await
}

async fn handle_save(
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    _runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    let changed = session.save_foreground(openid).await?;
    let key = if changed {
        "commands.save.updated"
    } else {
        "commands.save.already_saved"
    };
    Ok(CommandOutcome::Reply(CommandReply {
        text: t!(key, locale = lang.as_str()).into_owned(),
    }))
}

async fn handle_rename(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    _runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    if args.len() != 2 {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!("commands.rename.usage", locale = lang.as_str()).into_owned(),
        }));
    }
    session
        .rename_background_alias(openid, args[0], args[1])
        .await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: t!(
            "commands.rename.renamed",
            old = args[0],
            new = args[1],
            locale = lang.as_str()
        )
        .into_owned(),
    }))
}

async fn handle_sessions(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    _runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    if args.is_empty() || is_scope_token(args[0]) {
        let scope = if args.is_empty() {
            SessionListScope::All
        } else {
            parse_scope(args[0], lang.as_str())?
        };
        let sessions = session.list_disk_sessions(openid, scope).await?;
        let projects = collect_projects(&sessions);
        let (text, project_keys) = format_projects_list(&projects, lang.as_str());
        let has_projects = !project_keys.is_empty();
        session.set_last_projects_view(openid, project_keys).await?;
        session.set_last_sessions_view(openid, Vec::new()).await?;
        if has_projects {
            session
                .set_pending_setting(openid, Some(PendingSetting::SessionsProjects))
                .await?;
        }
        return Ok(CommandOutcome::Reply(CommandReply { text }));
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
    let all_sessions = session.list_disk_sessions(openid, scope).await?;
    let sessions = all_sessions
        .into_iter()
        .filter(|item| item.cwd.display().to_string() == project_path)
        .collect::<Vec<_>>();
    let (text, ids) = format_project_sessions_page(&project_path, &sessions, page, lang.as_str());
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
    Ok(CommandOutcome::Reply(CommandReply { text }))
}

async fn handle_import(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    _runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    let all = session.list_importable_sessions()?;
    let last_session_view = session.last_import_sessions_view(openid).await?;
    if let Some(selector) = args.first()
        && !last_session_view.is_empty()
        && let Ok(target) = resolve_selector(selector, &all, &last_session_view, lang.as_str())
    {
        let result = session.import_disk_session(&target).await?;
        let profile = result.profile;
        let action = if result.copied {
            t!("commands.import.imported", locale = lang.as_str())
        } else {
            t!("commands.import.refreshed", locale = lang.as_str())
        };
        session.set_pending_setting(openid, None).await?;
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.import.result",
                action = action.as_ref(),
                summary = session_summary(&target, lang.as_str()),
                workspace = profile.workspace_dir.display().to_string(),
                model = compact_imported_profile_summary(&profile, lang.as_str()),
                locale = lang.as_str()
            )
            .into_owned(),
        }));
    }

    if args.is_empty() {
        let projects = collect_projects(&all);
        let (text, project_keys) = format_import_projects_list(&projects, lang.as_str());
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
        return Ok(CommandOutcome::Reply(CommandReply { text }));
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
            let result = session.import_disk_session(&target).await?;
            let profile = result.profile;
            let action = if result.copied {
                t!("commands.import.imported", locale = lang.as_str())
            } else {
                t!("commands.import.refreshed", locale = lang.as_str())
            };
            session.set_pending_setting(openid, None).await?;
            return Ok(CommandOutcome::Reply(CommandReply {
                text: t!(
                    "commands.import.result",
                    action = action.as_ref(),
                    summary = session_summary(&target, lang.as_str()),
                    workspace = profile.workspace_dir.display().to_string(),
                    model = compact_imported_profile_summary(&profile, lang.as_str()),
                    locale = lang.as_str()
                )
                .into_owned(),
            }));
        }
    };
    let (_, project_path) = decode_project_key(&project_key)?;
    let project_sessions = all
        .into_iter()
        .filter(|item| item.cwd.display().to_string() == project_path)
        .collect::<Vec<_>>();
    let (text, ids) =
        format_import_project_sessions_page(&project_path, &project_sessions, page, lang.as_str());
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
    Ok(CommandOutcome::Reply(CommandReply { text }))
}

async fn handle_stop(
    openid: &str,
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
) -> Result<CommandOutcome> {
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    let locale = lang.as_str();
    let result = session.stop_foreground(openid).await?;
    let summary = if let Some(alias) = result.restored_alias.as_deref() {
        let snapshot = session.snapshot_for_user(openid).await?;
        let preview = foreground_last_user_message(session, openid, &snapshot).await?;
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

fn build_status_text(
    state: &UserSessionState,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> String {
    let effective = merged_settings(state);
    let lang = state.settings.language.as_str();
    let mut lines: Vec<String> = Vec::new();
    lines.push(
        t!(
            "commands.status.workspace",
            dir = state.foreground.workspace_dir.display().to_string(),
            locale = lang
        )
        .into_owned(),
    );
    lines.push(
        t!(
            "commands.status.model",
            summary = compact_runtime_summary(state, default_model, runtime_profile),
            locale = lang
        )
        .into_owned(),
    );
    lines.push(
        t!(
            if effective.verbose {
                "commands.status.verbose_on"
            } else {
                "commands.status.verbose_off"
            },
            locale = lang
        )
        .into_owned(),
    );
    lines.push(context_usage_line(state, runtime_profile, lang));
    if state.background.is_empty() {
        lines.push(t!("commands.status.bg_none", locale = lang).into_owned());
    } else {
        lines.push(
            t!(
                "commands.status.bg_header",
                count = state.background.len(),
                locale = lang
            )
            .into_owned(),
        );
        for alias in state.background.keys() {
            lines.push(format!("  - {alias}"));
        }
    }
    lines.push(
        t!(
            if is_busy {
                "commands.status.fg_busy"
            } else {
                "commands.status.fg_idle"
            },
            locale = lang
        )
        .into_owned(),
    );
    lines.push(t!("commands.status.lang", lang = lang, locale = lang).into_owned());
    lines.join("\n")
}

fn context_usage_line(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
    lang: &str,
) -> String {
    let Some(usage) = state.foreground.last_usage.as_ref() else {
        return t!("commands.status.context_unknown", locale = lang).into_owned();
    };
    let Some(used_tokens) = usage.context_tokens() else {
        return t!("commands.status.context_unknown", locale = lang).into_owned();
    };
    let window = if usage.window > 0 {
        usage.window
    } else {
        effective_context_window(state, runtime_profile)
    };
    let percent = usage.percent_remaining().unwrap_or_else(|| {
        if window == 0 {
            0
        } else {
            100_u64.saturating_sub(
                ((usage.total_tokens as f64 / window as f64) * 100.0).round() as u64,
            )
        }
    });
    t!(
        "commands.status.context_usage",
        percent = percent,
        used = format_tokens_compact(used_tokens),
        total = format_tokens_compact(window),
        locale = lang
    )
    .into_owned()
}

fn effective_context_window(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
) -> u64 {
    match merged_settings(state)
        .context_mode
        .or(runtime_profile.context_mode)
        .unwrap_or(ContextMode::Standard)
    {
        ContextMode::Standard => ContextMode::STANDARD_CONTEXT_WINDOW,
        ContextMode::OneM => 1_000_000,
    }
}

fn format_tokens_compact(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{}K", (value + 500) / 1_000)
    } else {
        value.to_string()
    }
}

async fn foreground_last_user_message(
    session: &SessionStore,
    openid: &str,
    snapshot: &UserSessionState,
) -> Result<Option<String>> {
    let Some(session_id) = snapshot.foreground.session_id.as_deref() else {
        return Ok(None);
    };
    let sessions = session
        .list_disk_sessions(openid, SessionListScope::All)
        .await?;
    Ok(sessions
        .into_iter()
        .find(|item| item.id == session_id)
        .and_then(|item| item.last_user_message))
}

fn format_effective_runtime_text(
    state: &UserSessionState,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    preview: Option<&str>,
) -> String {
    let lang = state.settings.language.as_str();
    let mut lines = Vec::new();
    if let Some(preview) = preview {
        lines.push(
            t!(
                "commands.runtime.last_user_message",
                preview = single_line(preview, 48),
                locale = lang
            )
            .into_owned(),
        );
    }
    lines.push(
        t!(
            "commands.runtime.model",
            summary = compact_runtime_summary(state, default_model, runtime_profile),
            locale = lang
        )
        .into_owned(),
    );
    lines.join("\n")
}

fn compact_runtime_summary(
    state: &UserSessionState,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
) -> String {
    compact_model_summary(
        effective_model(state, default_model, runtime_profile),
        effective_reasoning(state, runtime_profile),
        effective_context_token(state, runtime_profile),
        effective_tier_token(state, runtime_profile),
    )
}

fn compact_imported_profile_summary(
    profile: &crate::session::state::ImportedSessionProfile,
    lang: &str,
) -> String {
    let inherit_default = t!("commands.shared.inherit_default", locale = lang).into_owned();
    compact_model_summary(
        profile
            .model_override
            .clone()
            .unwrap_or_else(|| inherit_default.clone()),
        profile
            .reasoning_effort
            .map(|value| value.as_str())
            .unwrap_or(inherit_default.as_str()),
        profile.context_mode.map(|value| value.label().to_string()),
        profile.service_tier.map(|value| value.as_str().to_string()),
    )
}

fn compact_model_summary(
    model: String,
    reasoning: &str,
    context: Option<String>,
    tier: Option<String>,
) -> String {
    let mut parts = vec![model, reasoning.to_string()];
    if let Some(context) = context {
        parts.push(context);
    }
    if let Some(tier) = tier {
        parts.push(tier);
    }
    parts.join(" ")
}

fn effective_model(
    state: &UserSessionState,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
) -> String {
    let effective = merged_settings(state);
    effective
        .model_override
        .clone()
        .or_else(|| runtime_profile.configured_model.clone())
        .unwrap_or_else(|| default_model.to_string())
}

fn effective_reasoning(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
) -> &'static str {
    let effective = merged_settings(state);
    effective
        .reasoning_effort
        .or(runtime_profile.reasoning_effort)
        .unwrap_or(ReasoningEffort::Medium)
        .as_str()
}

fn effective_fast_label(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
) -> &'static str {
    let effective = merged_settings(state);
    match effective.service_tier.or(runtime_profile.service_tier) {
        Some(ServiceTier::Fast) => "on",
        Some(ServiceTier::Flex) => "off",
        None => "inherit",
    }
}

fn effective_tier_token(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
) -> Option<String> {
    let effective = merged_settings(state);
    match effective.service_tier.or(runtime_profile.service_tier) {
        Some(ServiceTier::Fast) => Some("fast".to_string()),
        Some(ServiceTier::Flex) => Some("flex".to_string()),
        None => None,
    }
}

fn effective_context_label(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
) -> &'static str {
    let effective = merged_settings(state);
    match effective.context_mode.or(runtime_profile.context_mode) {
        Some(mode) => mode.label(),
        None => "inherit",
    }
}

fn effective_context_token(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
) -> Option<String> {
    let effective = merged_settings(state);
    effective
        .context_mode
        .or(runtime_profile.context_mode)
        .map(|mode| mode.label().to_string())
}

fn merged_settings(state: &UserSessionState) -> crate::session::state::SessionSettings {
    state
        .settings
        .merged_with_profile(state.foreground.profile.as_ref())
}

fn parse_scope(value: &str, lang: &str) -> Result<SessionListScope> {
    match value.to_ascii_lowercase().as_str() {
        "all" => Ok(SessionListScope::All),
        _ => Err(anyhow!(
            t!("commands.sessions.scope_invalid", locale = lang).into_owned()
        )),
    }
}

fn is_scope_token(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "all")
}

fn collect_projects(sessions: &[DiskSessionMeta]) -> Vec<ProjectBucket> {
    let mut map = BTreeMap::new();
    for session in sessions {
        let key = session.cwd.display().to_string();
        let entry = map.entry(key.clone()).or_insert(ProjectBucket {
            path: key,
            count: 0,
            latest: None,
        });
        entry.count += 1;
        if entry.latest < session.updated_at {
            entry.latest = session.updated_at;
        }
    }
    let mut values = map.into_values().collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .latest
            .cmp(&left.latest)
            .then_with(|| left.path.cmp(&right.path))
    });
    values
}

fn format_projects_list(projects: &[ProjectBucket], lang: &str) -> (String, Vec<String>) {
    if projects.is_empty() {
        return (
            t!("commands.sessions.empty", locale = lang).into_owned(),
            Vec::new(),
        );
    }
    let mut lines = vec![
        t!(
            "commands.sessions.project_header",
            count = projects.len(),
            locale = lang
        )
        .into_owned(),
    ];
    let mut keys = Vec::new();
    for (index, project) in projects.iter().enumerate() {
        let latest = project
            .latest
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| t!("commands.shared.unknown", locale = lang).into_owned());
        lines.push(
            t!(
                "commands.sessions.project_row",
                index = index + 1,
                path = project.path.as_str(),
                sessions = project.count,
                latest = latest,
                locale = lang
            )
            .into_owned(),
        );
        keys.push(encode_project_key(SessionListScope::All, &project.path));
    }
    lines.push(String::new());
    lines.push(t!("commands.sessions.projects_footer", locale = lang).into_owned());
    (lines.join("\n"), keys)
}

fn format_import_projects_list(projects: &[ProjectBucket], lang: &str) -> (String, Vec<String>) {
    if projects.is_empty() {
        return (
            t!("commands.import.empty", locale = lang).into_owned(),
            Vec::new(),
        );
    }
    let mut lines = vec![
        t!(
            "commands.import.project_header",
            count = projects.len(),
            locale = lang
        )
        .into_owned(),
    ];
    let mut keys = Vec::new();
    for (index, project) in projects.iter().enumerate() {
        let latest = project
            .latest
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| t!("commands.shared.unknown", locale = lang).into_owned());
        lines.push(
            t!(
                "commands.import.project_row",
                index = index + 1,
                path = project.path.as_str(),
                sessions = project.count,
                latest = latest,
                locale = lang
            )
            .into_owned(),
        );
        keys.push(encode_project_key(SessionListScope::All, &project.path));
    }
    lines.push(String::new());
    lines.push(t!("commands.import.projects_footer", locale = lang).into_owned());
    (lines.join("\n"), keys)
}

fn format_project_sessions_page(
    project_path: &str,
    sessions: &[DiskSessionMeta],
    page: usize,
    lang: &str,
) -> (String, Vec<String>) {
    if sessions.is_empty() {
        return (
            t!(
                "commands.sessions.project_empty",
                path = project_path,
                locale = lang
            )
            .into_owned(),
            Vec::new(),
        );
    }
    let page_size = 12usize;
    let safe_page = page.max(1);
    let start = (safe_page - 1) * page_size;
    if start >= sessions.len() {
        return (
            t!(
                "commands.sessions.page_out_of_range",
                path = project_path,
                total = sessions.len(),
                size = page_size,
                page = safe_page,
                locale = lang
            )
            .into_owned(),
            Vec::new(),
        );
    }
    let end = (start + page_size).min(sessions.len());
    let total_pages = sessions.len().div_ceil(page_size);
    let mut lines = vec![
        t!(
            "commands.sessions.page_header",
            path = project_path,
            page = safe_page,
            total_pages = total_pages,
            total = sessions.len(),
            locale = lang
        )
        .into_owned(),
    ];
    let mut view_ids = Vec::new();
    for (offset, session) in sessions[start..end].iter().enumerate() {
        let index = offset + 1;
        let summary = session_summary(session, lang);
        let updated = session
            .updated_at
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| t!("commands.shared.unknown", locale = lang).into_owned());
        lines.push(
            t!(
                "commands.sessions.row",
                index = index,
                updated = updated,
                summary = summary,
                locale = lang
            )
            .into_owned(),
        );
        view_ids.push(session.id.clone());
    }
    lines.push(String::new());
    lines.push(t!("commands.sessions.page_footer", locale = lang).into_owned());
    (lines.join("\n"), view_ids)
}

fn format_import_project_sessions_page(
    project_path: &str,
    sessions: &[DiskSessionMeta],
    page: usize,
    lang: &str,
) -> (String, Vec<String>) {
    if sessions.is_empty() {
        return (
            t!(
                "commands.import.project_empty",
                path = project_path,
                locale = lang
            )
            .into_owned(),
            Vec::new(),
        );
    }
    let page_size = 12usize;
    let safe_page = page.max(1);
    let start = (safe_page - 1) * page_size;
    if start >= sessions.len() {
        return (
            t!(
                "commands.import.page_out_of_range",
                path = project_path,
                total = sessions.len(),
                size = page_size,
                page = safe_page,
                locale = lang
            )
            .into_owned(),
            Vec::new(),
        );
    }
    let end = (start + page_size).min(sessions.len());
    let mut lines = vec![
        t!(
            "commands.import.page_header",
            path = project_path,
            page = safe_page,
            total_pages = sessions.len().div_ceil(page_size),
            total = sessions.len(),
            locale = lang
        )
        .into_owned(),
    ];
    let mut view_ids = Vec::new();
    for (offset, session) in sessions[start..end].iter().enumerate() {
        let index = offset + 1;
        let summary = session_summary(session, lang);
        let updated = session
            .updated_at
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| t!("commands.shared.unknown", locale = lang).into_owned());
        lines.push(
            t!(
                "commands.import.row",
                index = index,
                updated = updated,
                summary = summary,
                locale = lang
            )
            .into_owned(),
        );
        view_ids.push(session.id.clone());
    }
    lines.push(String::new());
    lines.push(t!("commands.import.page_footer", locale = lang).into_owned());
    (lines.join("\n"), view_ids)
}

fn session_summary(session: &DiskSessionMeta, lang: &str) -> String {
    let fallback = t!("commands.sessions.no_summary", locale = lang);
    let raw = session
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.as_ref());
    single_line(raw, 72)
}

fn single_line(input: &str, max_chars: usize) -> String {
    let normalized = input.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut output = String::new();
    for (idx, ch) in normalized.chars().enumerate() {
        if idx >= max_chars {
            output.push_str("...");
            return output;
        }
        output.push(ch);
    }
    output
}

fn scope_label(scope: SessionListScope) -> &'static str {
    match scope {
        SessionListScope::All => "all",
        SessionListScope::Local => "local",
        SessionListScope::Global => "global",
    }
}

fn encode_project_key(scope: SessionListScope, path: &str) -> String {
    format!("{}{}{}", scope_label(scope), PROJECT_KEY_SEP, path)
}

fn decode_project_key(key: &str) -> Result<(SessionListScope, String)> {
    if let Some((scope_raw, path)) = key.split_once(PROJECT_KEY_SEP) {
        return Ok((parse_scope(scope_raw, "en")?, path.to_string()));
    }
    Ok((SessionListScope::All, key.to_string()))
}

fn resolve_project_selector(
    selector: &str,
    last_projects_view: &[String],
    lang: &str,
) -> Result<String> {
    if let Ok(index) = selector.parse::<usize>()
        && index >= 1
        && index <= last_projects_view.len()
    {
        return Ok(last_projects_view[index - 1].clone());
    }
    let normalized = selector.trim();
    if normalized.is_empty() {
        return Err(anyhow!(
            t!("commands.sessions.project_selector_empty", locale = lang).into_owned()
        ));
    }
    let matched = last_projects_view
        .iter()
        .filter(|key| {
            decode_project_key(key)
                .map(|(_, path)| path == normalized || path.starts_with(normalized))
                .unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();
    match matched.as_slice() {
        [single] => Ok(single.clone()),
        [] => Err(anyhow!(
            t!(
                "commands.sessions.project_not_found",
                selector = selector,
                locale = lang
            )
            .into_owned()
        )),
        _ => Err(anyhow!(
            t!("commands.sessions.project_ambiguous", locale = lang).into_owned()
        )),
    }
}

fn resolve_selector(
    selector: &str,
    sessions: &[DiskSessionMeta],
    last_view: &[String],
    lang: &str,
) -> Result<DiskSessionMeta> {
    if let Ok(index) = selector.parse::<usize>()
        && index >= 1
        && index <= last_view.len()
    {
        let target_id = &last_view[index - 1];
        if let Some(target) = sessions.iter().find(|value| value.id == *target_id) {
            return Ok(target.clone());
        }
    }
    if let Some(target) = sessions.iter().find(|value| value.id == selector) {
        return Ok(target.clone());
    }
    let matched = sessions
        .iter()
        .filter(|value| value.id.starts_with(selector))
        .cloned()
        .collect::<Vec<_>>();
    match matched.as_slice() {
        [single] => Ok(single.clone()),
        [] => Err(anyhow!(
            t!(
                "commands.sessions.session_not_found",
                selector = selector,
                locale = lang
            )
            .into_owned()
        )),
        _ => Err(anyhow!(
            t!("commands.sessions.session_ambiguous", locale = lang).into_owned()
        )),
    }
}

fn normalize_command_alias_name(input: &str) -> Result<String> {
    let raw = input.trim();
    let normalized = raw.to_ascii_lowercase();
    let is_valid = !raw.starts_with('/')
        && !normalized.is_empty()
        && normalized.chars().count() <= 20
        && !normalized.contains('|');
    anyhow::ensure!(is_valid, "invalid alias");
    Ok(normalized)
}

pub(crate) fn canonicalize_core_command(command: &str) -> &str {
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
        other => other,
    }
}

async fn handle_lang(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
) -> Result<CommandOutcome> {
    let snapshot = session.snapshot_for_user(openid).await?;
    let current_lang = snapshot.settings.language.clone();
    if args.is_empty() {
        return interactive::enter_lang_prompt(&snapshot, openid, session).await;
    }
    if args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.lang.status",
                lang = current_lang.as_str(),
                locale = current_lang.as_str()
            )
            .into_owned(),
        }));
    }
    let requested = args[0];
    let normalized = normalize_lang(requested);
    let is_known = matches!(
        requested.trim().to_ascii_lowercase().as_str(),
        "en" | "zh" | "zh-cn" | "zh_cn" | "cn" | "chinese"
    ) || requested.trim() == "中文";
    if !is_known {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.lang.unsupported",
                lang = requested,
                locale = current_lang.as_str()
            )
            .into_owned(),
        }));
    }
    session
        .update_settings_for_user(openid, |state| {
            state.language = normalized.to_string();
        })
        .await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: t!(
            "commands.lang.updated",
            lang = normalized,
            locale = normalized
        )
        .into_owned(),
    }))
}

fn help_text(lang: &str) -> String {
    let lang = normalize_lang(lang);
    let lines = vec![
        t!("commands.help.header", locale = lang).into_owned(),
        String::new(),
        t!("commands.help.section_basic", locale = lang).into_owned(),
        t!("commands.help.entry_help", locale = lang).into_owned(),
        t!("commands.help.entry_status", locale = lang).into_owned(),
        t!("commands.help.entry_new", locale = lang).into_owned(),
        t!("commands.help.entry_stop", locale = lang).into_owned(),
        t!("commands.help.entry_interrupt", locale = lang).into_owned(),
        t!("commands.help.entry_lang", locale = lang).into_owned(),
        String::new(),
        t!("commands.help.section_model_settings", locale = lang).into_owned(),
        t!("commands.help.entry_model", locale = lang).into_owned(),
        t!("commands.help.entry_reasoning", locale = lang).into_owned(),
        t!("commands.help.entry_fast", locale = lang).into_owned(),
        t!("commands.help.entry_context", locale = lang).into_owned(),
        String::new(),
        t!("commands.help.section_session_management", locale = lang).into_owned(),
        t!("commands.help.entry_sessions", locale = lang).into_owned(),
        t!("commands.help.entry_import", locale = lang).into_owned(),
        t!("commands.help.entry_resume", locale = lang).into_owned(),
        t!("commands.help.entry_save", locale = lang).into_owned(),
        String::new(),
        t!("commands.help.section_advanced", locale = lang).into_owned(),
        t!("commands.help.entry_compact", locale = lang).into_owned(),
        t!("commands.help.entry_fg", locale = lang).into_owned(),
        t!("commands.help.entry_bg", locale = lang).into_owned(),
        t!("commands.help.entry_loadbg", locale = lang).into_owned(),
        t!("commands.help.entry_rename", locale = lang).into_owned(),
        t!("commands.help.entry_alias", locale = lang).into_owned(),
        t!("commands.help.entry_verbose", locale = lang).into_owned(),
        t!("commands.help.entry_self_update", locale = lang).into_owned(),
    ];
    lines.join("\n")
}

fn resolve_new_workspace(
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

mod interactive {
    //! Helpers backing the multi-step `/model`, `/reasoning`, `/fast`,
    //! `/context`, `/verbose`, `/lang`, `/sessions`, `/import`, `/fg`,
    //! `/resume` and `/loadbg` pickers. These functions are invoked both
    //! when the user enters an interactive command with no arguments
    //! (`enter_*_prompt`) and when the user's next message is consumed while
    //! the pending state is active (`consume_pending_input`).
    use super::*;

    struct ReasoningChoice {
        value: &'static str,
        aliases: &'static [&'static str],
    }

    const REASONING_CHOICES: &[ReasoningChoice] = &[
        ReasoningChoice {
            value: "low",
            aliases: &["低"],
        },
        ReasoningChoice {
            value: "medium",
            aliases: &["中"],
        },
        ReasoningChoice {
            value: "high",
            aliases: &["高"],
        },
        ReasoningChoice {
            value: "xhigh",
            aliases: &["超高"],
        },
    ];

    #[derive(Debug, PartialEq, Eq)]
    pub enum FuzzyOutcome {
        Exact(String),
        Ambiguous(Vec<String>),
        None,
    }

    /// Case-insensitive exact / substring match with a uniqueness requirement.
    pub fn fuzzy_match_unique(input: &str, candidates: &[String]) -> FuzzyOutcome {
        let needle = input.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return FuzzyOutcome::None;
        }
        for candidate in candidates {
            if candidate.to_ascii_lowercase() == needle {
                return FuzzyOutcome::Exact(candidate.clone());
            }
        }
        let matches: Vec<String> = candidates
            .iter()
            .filter(|c| c.to_ascii_lowercase().contains(&needle))
            .cloned()
            .collect();
        match matches.len() {
            0 => FuzzyOutcome::None,
            1 => FuzzyOutcome::Exact(matches.into_iter().next().unwrap()),
            _ => FuzzyOutcome::Ambiguous(matches),
        }
    }

    fn dedupe_matches(values: Vec<String>) -> Vec<String> {
        values.into_iter().fold(Vec::new(), |mut acc, value| {
            if !acc.iter().any(|existing| existing == &value) {
                acc.push(value);
            }
            acc
        })
    }

    fn model_matches_exact(entry: &CodexModelEntry, needle: &str) -> bool {
        entry.name.eq_ignore_ascii_case(needle)
            || entry
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(needle))
    }

    fn model_matches_prefix(entry: &CodexModelEntry, needle: &str) -> bool {
        let needle = needle.to_ascii_lowercase();
        entry.name.to_ascii_lowercase().starts_with(&needle)
            || entry
                .aliases
                .iter()
                .any(|alias| alias.to_ascii_lowercase().starts_with(&needle))
    }

    fn match_model_input(input: &str, models: &[CodexModelEntry]) -> FuzzyOutcome {
        let needle = input.trim();
        if needle.is_empty() {
            return FuzzyOutcome::None;
        }

        let exact = dedupe_matches(
            models
                .iter()
                .filter(|entry| model_matches_exact(entry, needle))
                .map(|entry| entry.name.clone())
                .collect(),
        );
        match exact.len() {
            1 => return FuzzyOutcome::Exact(exact.into_iter().next().unwrap()),
            value if value > 1 => return FuzzyOutcome::Ambiguous(exact),
            _ => {}
        }

        let prefixes = dedupe_matches(
            models
                .iter()
                .filter(|entry| model_matches_prefix(entry, needle))
                .map(|entry| entry.name.clone())
                .collect(),
        );
        match prefixes.len() {
            0 => FuzzyOutcome::None,
            1 => FuzzyOutcome::Exact(prefixes.into_iter().next().unwrap()),
            _ => FuzzyOutcome::Ambiguous(prefixes),
        }
    }

    pub fn resolve_model_input(input: &str, models: &[CodexModelEntry]) -> Option<String> {
        match match_model_input(input, models) {
            FuzzyOutcome::Exact(choice) => Some(choice),
            FuzzyOutcome::Ambiguous(_) | FuzzyOutcome::None => None,
        }
    }

    fn choice_matches_exact(value: &str, aliases: &[&str], needle: &str) -> bool {
        value.eq_ignore_ascii_case(needle)
            || aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(needle))
    }

    fn match_reasoning_input(input: &str) -> FuzzyOutcome {
        let needle = input.trim();
        if needle.is_empty() {
            return FuzzyOutcome::None;
        }

        if needle.eq_ignore_ascii_case("inherit")
            || needle.eq_ignore_ascii_case("default")
            || needle == "继承"
            || needle == "默认"
        {
            return FuzzyOutcome::Exact("inherit".to_string());
        }

        let exact = dedupe_matches(
            REASONING_CHOICES
                .iter()
                .filter(|choice| choice_matches_exact(choice.value, choice.aliases, needle))
                .map(|choice| choice.value.to_string())
                .collect(),
        );
        match exact.len() {
            0 => FuzzyOutcome::None,
            1 => FuzzyOutcome::Exact(exact.into_iter().next().unwrap()),
            _ => FuzzyOutcome::Ambiguous(exact),
        }
    }

    pub fn resolve_reasoning_input(input: &str) -> Option<Option<ReasoningEffort>> {
        match match_reasoning_input(input) {
            FuzzyOutcome::Exact(choice) if choice == "inherit" => Some(None),
            FuzzyOutcome::Exact(choice) => ReasoningEffort::parse_supported(&choice).map(Some),
            FuzzyOutcome::Ambiguous(_) | FuzzyOutcome::None => None,
        }
    }

    pub(super) fn resolve_fast_input(input: &str) -> Option<Option<ServiceTier>> {
        let value = input.trim();
        if value.is_empty() {
            return None;
        }
        match value.to_ascii_lowercase().as_str() {
            "inherit" | "default" => Some(None),
            "on" => Some(Some(ServiceTier::Fast)),
            "off" => Some(Some(ServiceTier::Flex)),
            _ => match value {
                "默认" => Some(None),
                "开" => Some(Some(ServiceTier::Fast)),
                "关" => Some(Some(ServiceTier::Flex)),
                _ => None,
            },
        }
    }

    pub(super) fn resolve_context_input(input: &str) -> Option<Option<ContextMode>> {
        let value = input.trim();
        if value.is_empty() {
            return None;
        }
        match value.to_ascii_lowercase().as_str() {
            "inherit" | "default" => Some(None),
            "standard" | "272k" => Some(Some(ContextMode::Standard)),
            "1m" => Some(Some(ContextMode::OneM)),
            _ => match value {
                "默认" => Some(None),
                "标准" => Some(Some(ContextMode::Standard)),
                "长" => Some(Some(ContextMode::OneM)),
                _ => None,
            },
        }
    }

    fn verbose_options() -> &'static [&'static str] {
        &["on", "off"]
    }

    fn lang_options() -> &'static [&'static str] {
        &["en", "zh"]
    }

    fn as_string_vec(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    fn hint(locale: &str) -> String {
        t!("commands.interactive.hint", locale = locale).into_owned()
    }

    fn paragraph_hint_block(locale: &str) -> Vec<String> {
        vec![String::new(), hint(locale)]
    }

    fn format_markdown_aliases(aliases: &[String], locale: &str) -> String {
        let separator = if locale.eq_ignore_ascii_case("zh") {
            "、"
        } else {
            ", "
        };
        aliases
            .iter()
            .map(|alias| format!("*{alias}*"))
            .collect::<Vec<_>>()
            .join(separator)
    }

    fn format_model_prompt_item(entry: &CodexModelEntry, locale: &str) -> Vec<String> {
        let mut lines = vec![
            t!(
                "commands.model.prompt_item_name",
                name = entry.name.as_str(),
                locale = locale
            )
            .into_owned(),
        ];
        if let Some(description) = entry.description_for_locale(locale) {
            lines.push(
                t!(
                    "commands.model.prompt_item_description",
                    description = description,
                    locale = locale
                )
                .into_owned(),
            );
        }
        if !entry.aliases.is_empty() {
            lines.push(
                t!(
                    "commands.model.prompt_item_aliases",
                    aliases = format_markdown_aliases(&entry.aliases, locale),
                    locale = locale
                )
                .into_owned(),
            );
        }
        lines
    }

    fn join_prompt_blocks(blocks: Vec<Vec<String>>) -> String {
        blocks
            .into_iter()
            .map(|block| block.join("\n"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn ambiguous_reply(locale: &str, input: &str, matches: &[String]) -> String {
        t!(
            "commands.interactive.ambiguous",
            input = input,
            matches = matches.join(", "),
            locale = locale
        )
        .into_owned()
    }

    fn no_match_reply(locale: &str, input: &str) -> String {
        t!(
            "commands.interactive.no_match",
            input = input,
            locale = locale
        )
        .into_owned()
    }

    pub(super) fn model_extras(snapshot: &UserSessionState) -> Vec<String> {
        merged_settings(snapshot)
            .model_override
            .map(|value| vec![value])
            .unwrap_or_default()
    }

    // ---- Entry points (command with no args) -------------------------------

    pub async fn enter_model_prompt(
        snapshot: &UserSessionState,
        openid: &str,
        session: &SessionStore,
        default_model: &str,
        runtime_profile: &CodexRuntimeProfile,
        locale: &str,
    ) -> Result<CommandOutcome> {
        let models = list_codex_model_entries(runtime_profile, &model_extras(snapshot));
        let current = effective_model(snapshot, default_model, runtime_profile);
        let mut sections = vec![vec![
            t!(
                "commands.model.prompt_current",
                current = format!("`{}`", current),
                locale = locale
            )
            .into_owned(),
            t!("commands.model.prompt_header", locale = locale).into_owned(),
        ]];
        for entry in &models {
            sections.push(format_model_prompt_item(entry, locale));
        }
        sections.push(paragraph_hint_block(locale));
        session
            .set_pending_setting(openid, Some(PendingSetting::Model))
            .await?;
        Ok(CommandOutcome::Reply(CommandReply {
            text: join_prompt_blocks(sections),
        }))
    }

    pub async fn enter_reasoning_prompt(
        snapshot: &UserSessionState,
        openid: &str,
        session: &SessionStore,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<CommandOutcome> {
        let locale = snapshot.settings.language.as_str();
        let text = format!(
            "{}\n{}\n{}",
            t!(
                "commands.reasoning.prompt_current",
                current = effective_reasoning(snapshot, runtime_profile),
                locale = locale
            ),
            t!("commands.reasoning.prompt_header", locale = locale),
            hint(locale),
        );
        session
            .set_pending_setting(openid, Some(PendingSetting::Reasoning))
            .await?;
        Ok(CommandOutcome::Reply(CommandReply { text }))
    }

    pub async fn enter_fast_prompt(
        snapshot: &UserSessionState,
        openid: &str,
        session: &SessionStore,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<CommandOutcome> {
        let locale = snapshot.settings.language.as_str();
        let text = format!(
            "{}\n{}\n{}",
            t!(
                "commands.fast.prompt_current",
                current = effective_fast_label(snapshot, runtime_profile),
                locale = locale
            ),
            t!("commands.fast.prompt_header", locale = locale),
            hint(locale),
        );
        session
            .set_pending_setting(openid, Some(PendingSetting::Fast))
            .await?;
        Ok(CommandOutcome::Reply(CommandReply { text }))
    }

    pub async fn enter_context_prompt(
        snapshot: &UserSessionState,
        openid: &str,
        session: &SessionStore,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<CommandOutcome> {
        let locale = snapshot.settings.language.as_str();
        let text = format!(
            "{}\n{}\n{}",
            t!(
                "commands.context.prompt_current",
                current = effective_context_label(snapshot, runtime_profile),
                locale = locale
            ),
            t!("commands.context.prompt_header", locale = locale),
            hint(locale),
        );
        session
            .set_pending_setting(openid, Some(PendingSetting::Context))
            .await?;
        Ok(CommandOutcome::Reply(CommandReply { text }))
    }

    pub async fn enter_verbose_prompt(
        snapshot: &UserSessionState,
        openid: &str,
        session: &SessionStore,
    ) -> Result<CommandOutcome> {
        let locale = snapshot.settings.language.as_str();
        let current = if snapshot.settings.verbose {
            "on"
        } else {
            "off"
        };
        let text = format!(
            "{}\n{}\n{}",
            t!(
                "commands.verbose.prompt_current",
                current = current,
                locale = locale
            ),
            t!("commands.verbose.prompt_header", locale = locale),
            hint(locale),
        );
        session
            .set_pending_setting(openid, Some(PendingSetting::Verbose))
            .await?;
        Ok(CommandOutcome::Reply(CommandReply { text }))
    }

    pub async fn enter_lang_prompt(
        snapshot: &UserSessionState,
        openid: &str,
        session: &SessionStore,
    ) -> Result<CommandOutcome> {
        let locale = snapshot.settings.language.as_str();
        let text = format!(
            "{}\n{}\n{}",
            t!(
                "commands.lang.prompt_current",
                current = locale,
                locale = locale
            ),
            t!("commands.lang.prompt_header", locale = locale),
            hint(locale),
        );
        session
            .set_pending_setting(openid, Some(PendingSetting::Lang))
            .await?;
        Ok(CommandOutcome::Reply(CommandReply { text }))
    }

    pub async fn enter_fg_prompt(
        snapshot: &UserSessionState,
        openid: &str,
        session: &SessionStore,
    ) -> Result<CommandOutcome> {
        let locale = snapshot.settings.language.as_str();
        if snapshot.background.is_empty() {
            return Ok(CommandOutcome::Reply(CommandReply {
                text: t!("commands.fg.prompt_empty", locale = locale).into_owned(),
            }));
        }
        let mut lines = vec![t!("commands.fg.prompt_header", locale = locale).into_owned()];
        for alias in snapshot.background.keys() {
            lines.push(
                t!(
                    "commands.fg.prompt_item",
                    alias = alias.as_str(),
                    locale = locale
                )
                .into_owned(),
            );
        }
        lines.push(String::new());
        lines.push(hint(locale));
        session
            .set_pending_setting(openid, Some(PendingSetting::Fg))
            .await?;
        Ok(CommandOutcome::Reply(CommandReply {
            text: lines.join("\n"),
        }))
    }

    pub async fn enter_resume_projects_prompt(
        openid: &str,
        session: &SessionStore,
        locale: &str,
    ) -> Result<CommandOutcome> {
        let sessions = session
            .list_disk_sessions(openid, SessionListScope::All)
            .await?;
        let projects = collect_projects(&sessions);
        let (text, project_keys) = format_projects_list(&projects, locale);
        let has_projects = !project_keys.is_empty();
        session.set_last_projects_view(openid, project_keys).await?;
        session.set_last_sessions_view(openid, Vec::new()).await?;
        if has_projects {
            session
                .set_pending_setting(openid, Some(PendingSetting::ResumeProjects))
                .await?;
        }
        Ok(CommandOutcome::Reply(CommandReply { text }))
    }

    pub async fn enter_resume_sessions_prompt(
        openid: &str,
        session: &SessionStore,
        project_key: String,
        page: usize,
        locale: &str,
    ) -> Result<CommandOutcome> {
        let (scope, project_path) = decode_project_key(&project_key)?;
        let all_sessions = session.list_disk_sessions(openid, scope).await?;
        let project_sessions = all_sessions
            .into_iter()
            .filter(|item| item.cwd.display().to_string() == project_path)
            .collect::<Vec<_>>();
        let (text, ids) =
            format_project_sessions_page(&project_path, &project_sessions, page, locale);
        session.set_last_sessions_view(openid, ids).await?;
        session
            .set_pending_setting(
                openid,
                Some(PendingSetting::ResumeSessions { project_key, page }),
            )
            .await?;
        Ok(CommandOutcome::Reply(CommandReply { text }))
    }

    pub async fn enter_loadbg_projects_prompt(
        openid: &str,
        session: &SessionStore,
        locale: &str,
    ) -> Result<CommandOutcome> {
        let sessions = session
            .list_disk_sessions(openid, SessionListScope::All)
            .await?;
        let projects = collect_projects(&sessions);
        let (text, project_keys) = format_projects_list(&projects, locale);
        let has_projects = !project_keys.is_empty();
        session.set_last_projects_view(openid, project_keys).await?;
        session.set_last_sessions_view(openid, Vec::new()).await?;
        if has_projects {
            session
                .set_pending_setting(openid, Some(PendingSetting::LoadbgProjects))
                .await?;
        }
        Ok(CommandOutcome::Reply(CommandReply { text }))
    }

    pub async fn enter_loadbg_sessions_prompt(
        openid: &str,
        session: &SessionStore,
        project_key: String,
        page: usize,
        alias: Option<String>,
        locale: &str,
    ) -> Result<CommandOutcome> {
        let (scope, project_path) = decode_project_key(&project_key)?;
        let all_sessions = session.list_disk_sessions(openid, scope).await?;
        let project_sessions = all_sessions
            .into_iter()
            .filter(|item| item.cwd.display().to_string() == project_path)
            .collect::<Vec<_>>();
        let (text, ids) =
            format_project_sessions_page(&project_path, &project_sessions, page, locale);
        session.set_last_sessions_view(openid, ids).await?;
        session
            .set_pending_setting(
                openid,
                Some(PendingSetting::LoadbgSessions {
                    project_key,
                    page,
                    alias,
                }),
            )
            .await?;
        Ok(CommandOutcome::Reply(CommandReply { text }))
    }

    // ---- Actions -----------------------------------------------------------

    pub async fn switch_foreground(
        alias: &str,
        openid: &str,
        session: &SessionStore,
        default_model: &str,
        runtime_profile: &CodexRuntimeProfile,
        locale: &str,
    ) -> Result<CommandOutcome> {
        let switched = session.foreground_from_background(openid, alias).await?;
        let snapshot = session.snapshot_for_user(openid).await?;
        let preview = foreground_last_user_message(session, openid, &snapshot).await?;
        let parked = switched
            .parked_alias
            .map(|value| {
                t!(
                    "commands.fg.parked",
                    alias = value.as_str(),
                    locale = locale
                )
                .into_owned()
            })
            .unwrap_or_default();
        session.set_pending_setting(openid, None).await?;
        Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.fg.switched",
                parked = parked,
                alias = alias,
                runtime = format_effective_runtime_text(
                    &snapshot,
                    default_model,
                    runtime_profile,
                    preview.as_deref()
                ),
                locale = locale
            )
            .into_owned(),
        }))
    }

    pub async fn execute_resume(
        openid: &str,
        session: &SessionStore,
        default_model: &str,
        runtime_profile: &CodexRuntimeProfile,
        target: &DiskSessionMeta,
    ) -> Result<CommandOutcome> {
        let lang = session
            .snapshot_for_user(openid)
            .await?
            .settings
            .language
            .clone();
        let switched = session.resume_disk_session(openid, target).await?;
        let imported_profile = session.imported_profile_for_session(&target.id).await?;
        let workspace_display = imported_profile
            .as_ref()
            .map(|value| value.workspace_dir.display().to_string())
            .unwrap_or_else(|| target.cwd.display().to_string());
        let snapshot = session.snapshot_for_user(openid).await?;
        let parked = switched
            .parked_alias
            .map(|value| {
                t!(
                    "commands.resume.parked",
                    alias = value.as_str(),
                    locale = lang.as_str()
                )
                .into_owned()
            })
            .unwrap_or_default();
        session.set_pending_setting(openid, None).await?;
        Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.resume.restored",
                parked = parked,
                summary = session_summary(target, lang.as_str()),
                workspace = workspace_display,
                runtime = format_effective_runtime_text(
                    &snapshot,
                    default_model,
                    runtime_profile,
                    target.last_user_message.as_deref(),
                ),
                locale = lang.as_str()
            )
            .into_owned(),
        }))
    }

    pub async fn execute_loadbg(
        openid: &str,
        session: &SessionStore,
        target: &DiskSessionMeta,
        alias: Option<&str>,
    ) -> Result<CommandOutcome> {
        let lang = session
            .snapshot_for_user(openid)
            .await?
            .settings
            .language
            .clone();
        let new_alias = session
            .load_disk_session_to_background(openid, target, alias)
            .await?;
        session.set_pending_setting(openid, None).await?;
        Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.loadbg.loaded",
                alias = new_alias.as_str(),
                summary = session_summary(target, lang.as_str()),
                locale = lang.as_str()
            )
            .into_owned(),
        }))
    }

    // ---- Pending-input consumption ----------------------------------------

    pub async fn consume_pending_input(
        pending: PendingSetting,
        text: &str,
        openid: &str,
        session: &SessionStore,
        default_model: &str,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<CommandOutcome> {
        match pending {
            PendingSetting::Model => {
                consume_model(text, openid, session, default_model, runtime_profile).await
            }
            PendingSetting::Reasoning => {
                consume_reasoning(text, openid, session, runtime_profile).await
            }
            PendingSetting::Fast => consume_fast(text, openid, session, runtime_profile).await,
            PendingSetting::Context => {
                consume_context(text, openid, session, runtime_profile).await
            }
            PendingSetting::Verbose => consume_verbose(text, openid, session).await,
            PendingSetting::Lang => consume_lang(text, openid, session).await,
            PendingSetting::SessionsProjects => {
                consume_sessions_projects(text, openid, session).await
            }
            PendingSetting::SessionsSessions { project_key, page } => {
                consume_sessions_sessions(text, openid, session, project_key, page).await
            }
            PendingSetting::ImportProjects => consume_import_projects(text, openid, session).await,
            PendingSetting::ImportSessions { project_key, page } => {
                consume_import_sessions(text, openid, session, project_key, page).await
            }
            PendingSetting::Fg => {
                consume_fg(text, openid, session, default_model, runtime_profile).await
            }
            PendingSetting::ResumeProjects => consume_resume_projects(text, openid, session).await,
            PendingSetting::ResumeSessions { project_key, page } => {
                consume_resume_sessions(
                    text,
                    openid,
                    session,
                    project_key,
                    page,
                    default_model,
                    runtime_profile,
                )
                .await
            }
            PendingSetting::LoadbgProjects => consume_loadbg_projects(text, openid, session).await,
            PendingSetting::LoadbgSessions {
                project_key,
                page,
                alias,
            } => consume_loadbg_sessions(text, openid, session, project_key, page, alias).await,
        }
    }

    async fn consume_model(
        text: &str,
        openid: &str,
        session: &SessionStore,
        default_model: &str,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<CommandOutcome> {
        let snapshot = session.snapshot_for_user(openid).await?;
        let locale = snapshot.settings.language.clone();
        let locale = locale.as_str();
        let input = text.trim();
        let models = list_codex_model_entries(runtime_profile, &model_extras(&snapshot));
        match if input.eq_ignore_ascii_case("inherit") || input.eq_ignore_ascii_case("default") {
            FuzzyOutcome::Exact("inherit".to_string())
        } else {
            match_model_input(input, &models)
        } {
            FuzzyOutcome::Exact(choice) => {
                let next = if choice.eq_ignore_ascii_case("inherit") {
                    None
                } else {
                    Some(choice.clone())
                };
                session.set_model_override_for_active(openid, next).await?;
                session.set_pending_setting(openid, None).await?;
                let snapshot = session.snapshot_for_user(openid).await?;
                Ok(CommandOutcome::Reply(CommandReply {
                    text: t!(
                        "commands.model.updated",
                        model = effective_model(&snapshot, default_model, runtime_profile),
                        locale = locale
                    )
                    .into_owned(),
                }))
            }
            FuzzyOutcome::Ambiguous(matches) => Ok(CommandOutcome::Reply(CommandReply {
                text: ambiguous_reply(locale, input, &matches),
            })),
            FuzzyOutcome::None => Ok(CommandOutcome::Reply(CommandReply {
                text: no_match_reply(locale, input),
            })),
        }
    }

    async fn consume_reasoning(
        text: &str,
        openid: &str,
        session: &SessionStore,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<CommandOutcome> {
        let snapshot = session.snapshot_for_user(openid).await?;
        let locale = snapshot.settings.language.clone();
        let locale = locale.as_str();
        let input = text.trim();
        match match_reasoning_input(input) {
            FuzzyOutcome::Exact(choice) => {
                let next = if choice == "inherit" {
                    None
                } else {
                    ReasoningEffort::parse_supported(&choice)
                };
                session.set_reasoning_for_active(openid, next).await?;
                session.set_pending_setting(openid, None).await?;
                let snapshot = session.snapshot_for_user(openid).await?;
                Ok(CommandOutcome::Reply(CommandReply {
                    text: t!(
                        "commands.reasoning.updated",
                        value = effective_reasoning(&snapshot, runtime_profile),
                        locale = locale
                    )
                    .into_owned(),
                }))
            }
            FuzzyOutcome::Ambiguous(matches) => Ok(CommandOutcome::Reply(CommandReply {
                text: ambiguous_reply(locale, input, &matches),
            })),
            FuzzyOutcome::None => Ok(CommandOutcome::Reply(CommandReply {
                text: no_match_reply(locale, input),
            })),
        }
    }

    async fn consume_fast(
        text: &str,
        openid: &str,
        session: &SessionStore,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<CommandOutcome> {
        let snapshot = session.snapshot_for_user(openid).await?;
        let locale = snapshot.settings.language.clone();
        let locale = locale.as_str();
        let input = text.trim();
        match resolve_fast_input(input) {
            Some(next) => {
                session.set_service_tier_for_active(openid, next).await?;
                session.set_pending_setting(openid, None).await?;
                let snapshot = session.snapshot_for_user(openid).await?;
                Ok(CommandOutcome::Reply(CommandReply {
                    text: t!(
                        "commands.fast.updated",
                        value = effective_fast_label(&snapshot, runtime_profile),
                        locale = locale
                    )
                    .into_owned(),
                }))
            }
            None => Ok(CommandOutcome::Reply(CommandReply {
                text: no_match_reply(locale, input),
            })),
        }
    }

    async fn consume_context(
        text: &str,
        openid: &str,
        session: &SessionStore,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<CommandOutcome> {
        let snapshot = session.snapshot_for_user(openid).await?;
        let locale = snapshot.settings.language.clone();
        let locale = locale.as_str();
        let input = text.trim();
        match resolve_context_input(input) {
            Some(next) => {
                session.set_context_mode_for_active(openid, next).await?;
                session.set_pending_setting(openid, None).await?;
                let snapshot = session.snapshot_for_user(openid).await?;
                Ok(CommandOutcome::Reply(CommandReply {
                    text: t!(
                        "commands.context.updated",
                        value = effective_context_label(&snapshot, runtime_profile),
                        locale = locale
                    )
                    .into_owned(),
                }))
            }
            None => Ok(CommandOutcome::Reply(CommandReply {
                text: no_match_reply(locale, input),
            })),
        }
    }

    async fn consume_verbose(
        text: &str,
        openid: &str,
        session: &SessionStore,
    ) -> Result<CommandOutcome> {
        let snapshot = session.snapshot_for_user(openid).await?;
        let locale = snapshot.settings.language.clone();
        let locale = locale.as_str();
        let candidates = as_string_vec(verbose_options());
        let input = text.trim();
        match fuzzy_match_unique(input, &candidates) {
            FuzzyOutcome::Exact(choice) => {
                let enabled = choice == "on";
                session
                    .update_settings_for_user(openid, |state| state.verbose = enabled)
                    .await?;
                session.set_pending_setting(openid, None).await?;
                let key = if enabled {
                    "commands.verbose.updated_on"
                } else {
                    "commands.verbose.updated_off"
                };
                Ok(CommandOutcome::Reply(CommandReply {
                    text: t!(key, locale = locale).into_owned(),
                }))
            }
            FuzzyOutcome::Ambiguous(matches) => Ok(CommandOutcome::Reply(CommandReply {
                text: ambiguous_reply(locale, input, &matches),
            })),
            FuzzyOutcome::None => Ok(CommandOutcome::Reply(CommandReply {
                text: no_match_reply(locale, input),
            })),
        }
    }

    async fn consume_lang(
        text: &str,
        openid: &str,
        session: &SessionStore,
    ) -> Result<CommandOutcome> {
        let snapshot = session.snapshot_for_user(openid).await?;
        let current_locale = snapshot.settings.language.clone();
        let candidates = as_string_vec(lang_options());
        let input = text.trim();
        match fuzzy_match_unique(input, &candidates) {
            FuzzyOutcome::Exact(choice) => {
                let normalized = normalize_lang(&choice);
                session
                    .update_settings_for_user(openid, |state| {
                        state.language = normalized.to_string();
                    })
                    .await?;
                session.set_pending_setting(openid, None).await?;
                Ok(CommandOutcome::Reply(CommandReply {
                    text: t!(
                        "commands.lang.updated",
                        lang = normalized,
                        locale = normalized
                    )
                    .into_owned(),
                }))
            }
            FuzzyOutcome::Ambiguous(matches) => Ok(CommandOutcome::Reply(CommandReply {
                text: ambiguous_reply(current_locale.as_str(), input, &matches),
            })),
            FuzzyOutcome::None => Ok(CommandOutcome::Reply(CommandReply {
                text: no_match_reply(current_locale.as_str(), input),
            })),
        }
    }

    async fn consume_fg(
        text: &str,
        openid: &str,
        session: &SessionStore,
        default_model: &str,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<CommandOutcome> {
        let snapshot = session.snapshot_for_user(openid).await?;
        let locale = snapshot.settings.language.clone();
        let locale = locale.as_str();
        let candidates: Vec<String> = snapshot.background.keys().cloned().collect();
        let input = text.trim();
        match fuzzy_match_unique(input, &candidates) {
            FuzzyOutcome::Exact(alias) => {
                switch_foreground(
                    &alias,
                    openid,
                    session,
                    default_model,
                    runtime_profile,
                    locale,
                )
                .await
            }
            FuzzyOutcome::Ambiguous(matches) => Ok(CommandOutcome::Reply(CommandReply {
                text: ambiguous_reply(locale, input, &matches),
            })),
            FuzzyOutcome::None => Ok(CommandOutcome::Reply(CommandReply {
                text: no_match_reply(locale, input),
            })),
        }
    }

    async fn consume_sessions_projects(
        text: &str,
        openid: &str,
        session: &SessionStore,
    ) -> Result<CommandOutcome> {
        let locale = session
            .snapshot_for_user(openid)
            .await?
            .settings
            .language
            .clone();
        let projects_view = session.last_projects_view(openid).await?;
        let project_key =
            match resolve_project_selector(text.trim(), &projects_view, locale.as_str()) {
                Ok(value) => value,
                Err(err) => {
                    return Ok(CommandOutcome::Reply(CommandReply {
                        text: err.to_string(),
                    }));
                }
            };
        let (scope, project_path) = decode_project_key(&project_key)?;
        let all_sessions = session.list_disk_sessions(openid, scope).await?;
        let sessions = all_sessions
            .into_iter()
            .filter(|item| item.cwd.display().to_string() == project_path)
            .collect::<Vec<_>>();
        let (text_out, ids) =
            format_project_sessions_page(&project_path, &sessions, 1, locale.as_str());
        session.set_last_sessions_view(openid, ids).await?;
        session
            .set_pending_setting(
                openid,
                Some(PendingSetting::SessionsSessions {
                    project_key,
                    page: 1,
                }),
            )
            .await?;
        Ok(CommandOutcome::Reply(CommandReply { text: text_out }))
    }

    async fn consume_sessions_sessions(
        _text: &str,
        openid: &str,
        session: &SessionStore,
        project_key: String,
        page: usize,
    ) -> Result<CommandOutcome> {
        // /sessions is view-only; at this depth, remind the user how to act.
        let locale = session
            .snapshot_for_user(openid)
            .await?
            .settings
            .language
            .clone();
        let text = t!("commands.sessions.page_footer", locale = locale.as_str()).into_owned();
        session
            .set_pending_setting(
                openid,
                Some(PendingSetting::SessionsSessions { project_key, page }),
            )
            .await?;
        Ok(CommandOutcome::Reply(CommandReply { text }))
    }

    async fn consume_import_projects(
        text: &str,
        openid: &str,
        session: &SessionStore,
    ) -> Result<CommandOutcome> {
        let locale = session
            .snapshot_for_user(openid)
            .await?
            .settings
            .language
            .clone();
        let projects_view = session.last_import_projects_view(openid).await?;
        let project_key =
            match resolve_project_selector(text.trim(), &projects_view, locale.as_str()) {
                Ok(value) => value,
                Err(err) => {
                    return Ok(CommandOutcome::Reply(CommandReply {
                        text: err.to_string(),
                    }));
                }
            };
        let (_, project_path) = decode_project_key(&project_key)?;
        let all = session.list_importable_sessions()?;
        let project_sessions = all
            .into_iter()
            .filter(|item| item.cwd.display().to_string() == project_path)
            .collect::<Vec<_>>();
        let (text_out, ids) = format_import_project_sessions_page(
            &project_path,
            &project_sessions,
            1,
            locale.as_str(),
        );
        session.set_last_import_sessions_view(openid, ids).await?;
        session
            .set_pending_setting(
                openid,
                Some(PendingSetting::ImportSessions {
                    project_key,
                    page: 1,
                }),
            )
            .await?;
        Ok(CommandOutcome::Reply(CommandReply { text: text_out }))
    }

    async fn consume_import_sessions(
        text: &str,
        openid: &str,
        session: &SessionStore,
        project_key: String,
        page: usize,
    ) -> Result<CommandOutcome> {
        let locale = session
            .snapshot_for_user(openid)
            .await?
            .settings
            .language
            .clone();
        let all = session.list_importable_sessions()?;
        let last_view = session.last_import_sessions_view(openid).await?;
        let target = match resolve_selector(text.trim(), &all, &last_view, locale.as_str()) {
            Ok(value) => value,
            Err(err) => {
                // Keep the pending state alive so the user can retry.
                session
                    .set_pending_setting(
                        openid,
                        Some(PendingSetting::ImportSessions { project_key, page }),
                    )
                    .await?;
                return Ok(CommandOutcome::Reply(CommandReply {
                    text: err.to_string(),
                }));
            }
        };
        let result = session.import_disk_session(&target).await?;
        let profile = result.profile;
        let action = if result.copied {
            t!("commands.import.imported", locale = locale.as_str())
        } else {
            t!("commands.import.refreshed", locale = locale.as_str())
        };
        session.set_pending_setting(openid, None).await?;
        Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.import.result",
                action = action.as_ref(),
                summary = session_summary(&target, locale.as_str()),
                workspace = profile.workspace_dir.display().to_string(),
                model = compact_imported_profile_summary(&profile, locale.as_str()),
                locale = locale.as_str()
            )
            .into_owned(),
        }))
    }

    async fn consume_resume_projects(
        text: &str,
        openid: &str,
        session: &SessionStore,
    ) -> Result<CommandOutcome> {
        let locale = session
            .snapshot_for_user(openid)
            .await?
            .settings
            .language
            .clone();
        let projects_view = session.last_projects_view(openid).await?;
        let project_key =
            match resolve_project_selector(text.trim(), &projects_view, locale.as_str()) {
                Ok(value) => value,
                Err(err) => {
                    return Ok(CommandOutcome::Reply(CommandReply {
                        text: err.to_string(),
                    }));
                }
            };
        enter_resume_sessions_prompt(openid, session, project_key, 1, locale.as_str()).await
    }

    async fn consume_resume_sessions(
        text: &str,
        openid: &str,
        session: &SessionStore,
        project_key: String,
        page: usize,
        default_model: &str,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<CommandOutcome> {
        let locale = session
            .snapshot_for_user(openid)
            .await?
            .settings
            .language
            .clone();
        let sessions = session
            .list_disk_sessions(openid, SessionListScope::All)
            .await?;
        let last_view = session.last_sessions_view(openid).await?;
        let target = match resolve_selector(text.trim(), &sessions, &last_view, locale.as_str()) {
            Ok(value) => value,
            Err(err) => {
                session
                    .set_pending_setting(
                        openid,
                        Some(PendingSetting::ResumeSessions { project_key, page }),
                    )
                    .await?;
                return Ok(CommandOutcome::Reply(CommandReply {
                    text: err.to_string(),
                }));
            }
        };
        execute_resume(openid, session, default_model, runtime_profile, &target).await
    }

    async fn consume_loadbg_projects(
        text: &str,
        openid: &str,
        session: &SessionStore,
    ) -> Result<CommandOutcome> {
        let locale = session
            .snapshot_for_user(openid)
            .await?
            .settings
            .language
            .clone();
        let projects_view = session.last_projects_view(openid).await?;
        let project_key =
            match resolve_project_selector(text.trim(), &projects_view, locale.as_str()) {
                Ok(value) => value,
                Err(err) => {
                    return Ok(CommandOutcome::Reply(CommandReply {
                        text: err.to_string(),
                    }));
                }
            };
        enter_loadbg_sessions_prompt(openid, session, project_key, 1, None, locale.as_str()).await
    }

    async fn consume_loadbg_sessions(
        text: &str,
        openid: &str,
        session: &SessionStore,
        project_key: String,
        page: usize,
        alias: Option<String>,
    ) -> Result<CommandOutcome> {
        let locale = session
            .snapshot_for_user(openid)
            .await?
            .settings
            .language
            .clone();
        let sessions = session
            .list_disk_sessions(openid, SessionListScope::All)
            .await?;
        let last_view = session.last_sessions_view(openid).await?;
        let target = match resolve_selector(text.trim(), &sessions, &last_view, locale.as_str()) {
            Ok(value) => value,
            Err(err) => {
                session
                    .set_pending_setting(
                        openid,
                        Some(PendingSetting::LoadbgSessions {
                            project_key,
                            page,
                            alias,
                        }),
                    )
                    .await?;
                return Ok(CommandOutcome::Reply(CommandReply {
                    text: err.to_string(),
                }));
            }
        };
        execute_loadbg(openid, session, &target, alias.as_deref()).await
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn fuzzy_exact_wins_over_substring() {
            let candidates = vec![
                "gpt-5.4".to_string(),
                "gpt-5.4-mini".to_string(),
                "gpt-5.3-codex".to_string(),
            ];
            assert_eq!(
                fuzzy_match_unique("gpt-5.4", &candidates),
                FuzzyOutcome::Exact("gpt-5.4".to_string())
            );
        }

        #[test]
        fn fuzzy_substring_requires_uniqueness() {
            let candidates = vec!["gpt-5.4".to_string(), "gpt-5.4-mini".to_string()];
            match fuzzy_match_unique("5.4", &candidates) {
                FuzzyOutcome::Ambiguous(matches) => {
                    assert_eq!(matches.len(), 2);
                }
                other => panic!("expected ambiguous, got {other:?}"),
            }
            assert_eq!(
                fuzzy_match_unique("mini", &candidates),
                FuzzyOutcome::Exact("gpt-5.4-mini".to_string())
            );
            assert_eq!(fuzzy_match_unique("nope", &candidates), FuzzyOutcome::None);
        }

        #[test]
        fn fuzzy_is_case_insensitive() {
            let candidates = vec!["Medium".to_string(), "Low".to_string()];
            assert_eq!(
                fuzzy_match_unique("MED", &candidates),
                FuzzyOutcome::Exact("Medium".to_string())
            );
        }

        #[test]
        fn fuzzy_empty_input_returns_none() {
            let candidates = vec!["on".to_string(), "off".to_string()];
            assert_eq!(fuzzy_match_unique("   ", &candidates), FuzzyOutcome::None);
        }

        #[test]
        fn model_alias_exact_match_prefers_canonical_name() {
            let candidates = vec![
                CodexModelEntry {
                    name: "gpt-5.4".to_string(),
                    aliases: vec!["54".to_string(), "5.4".to_string()],
                    description: None,
                    description_zh: None,
                    description_en: None,
                },
                CodexModelEntry {
                    name: "gpt-5.4-mini".to_string(),
                    aliases: vec!["mini".to_string(), "54m".to_string()],
                    description: None,
                    description_zh: None,
                    description_en: None,
                },
            ];
            assert_eq!(
                match_model_input("mini", &candidates),
                FuzzyOutcome::Exact("gpt-5.4-mini".to_string())
            );
            assert_eq!(
                match_model_input("54", &candidates),
                FuzzyOutcome::Exact("gpt-5.4".to_string())
            );
        }

        #[test]
        fn model_match_uses_unique_prefix_not_substring() {
            let candidates = vec![
                CodexModelEntry {
                    name: "gpt-5.4".to_string(),
                    aliases: vec!["54".to_string()],
                    description: None,
                    description_zh: None,
                    description_en: None,
                },
                CodexModelEntry {
                    name: "gpt-5.4-mini".to_string(),
                    aliases: vec!["mini".to_string(), "54m".to_string()],
                    description: None,
                    description_zh: None,
                    description_en: None,
                },
            ];
            assert_eq!(match_model_input("4-m", &candidates), FuzzyOutcome::None);
            assert_eq!(
                match_model_input("gpt-5.4-m", &candidates),
                FuzzyOutcome::Exact("gpt-5.4-mini".to_string())
            );
        }

        #[test]
        fn reasoning_alias_exact_match_prefers_supported_values() {
            assert_eq!(
                match_reasoning_input("高"),
                FuzzyOutcome::Exact("high".to_string())
            );
            assert_eq!(
                match_reasoning_input("超高"),
                FuzzyOutcome::Exact("xhigh".to_string())
            );
            assert_eq!(
                match_reasoning_input("默认"),
                FuzzyOutcome::Exact("inherit".to_string())
            );
            assert_eq!(
                match_reasoning_input("继承"),
                FuzzyOutcome::Exact("inherit".to_string())
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use tokio::fs;

    use crate::{
        codex::runtime::CodexRuntimeProfile,
        session::{
            state::{ContextMode, ReasoningEffort, ServiceTier, TokenUsageSnapshot},
            store::SessionStore,
        },
    };

    use super::{CommandOutcome, maybe_handle_command};

    #[tokio::test]
    async fn new_command_keeps_settings() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .set_foreground_session_id("u1", Some("thread".into()))
            .await
            .unwrap();
        session
            .update_settings_for_user("u1", |state| {
                state.model_override = Some("gpt-x".into());
                state.verbose = true;
            })
            .await
            .unwrap();
        let outcome = maybe_handle_command(
            "/new",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
        let snapshot = session.snapshot_for_user("u1").await.unwrap();
        assert!(snapshot.foreground.session_id.is_none());
        assert_eq!(snapshot.settings.model_override.as_deref(), Some("gpt-x"));
        assert!(snapshot.settings.verbose);
    }

    #[tokio::test]
    async fn new_command_accepts_manual_workspace() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace_root = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace_root.path(),
        )
        .await
        .unwrap();
        let outcome = maybe_handle_command(
            "/new custom folder",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();

        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply");
        };
        let snapshot = session.snapshot_for_user("u1").await.unwrap();
        let expected =
            std::fs::canonicalize(data.path().join("session/workspace/custom folder")).unwrap();
        assert_eq!(snapshot.foreground.workspace_dir, expected);
        assert!(reply.text.to_lowercase().contains("workdir"));
        assert!(reply.text.contains("custom folder"));
    }

    #[tokio::test]
    async fn new_command_reports_effective_runtime_settings() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .update_settings_for_user("u1", |state| {
                state.model_override = Some("gpt-global".into());
                state.reasoning_effort = Some(ReasoningEffort::High);
                state.context_mode = Some(ContextMode::OneM);
                state.service_tier = Some(ServiceTier::Fast);
            })
            .await
            .unwrap();

        let outcome = maybe_handle_command(
            "/new",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();

        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply");
        };
        assert!(reply.text.contains("gpt-global high 1M fast"));
    }

    #[tokio::test]
    async fn resume_command_reports_profile_and_last_user_message_preview() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session_dir = global_home.path().join("sessions/2026/04/11");
        fs::create_dir_all(&session_dir).await.unwrap();
        fs::write(
            session_dir.join("rollout-2026-04-11T00-00-00-thread-1.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"thread-1","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp/project-a"}}
{"type":"turn_context","payload":{"cwd":"/tmp/project-a","model":"gpt-5.4","effort":"high"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":950000}}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"You are CodexClaw running behind QQ official bot.\n\nUser message:\n请帮我处理发布失败"}]}}
"#,
        )
        .await
        .unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        let outcome = maybe_handle_command(
            "/resume thread-1",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();

        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply");
        };
        assert!(reply.text.contains("请帮我处理发布失败"));
        assert!(reply.text.contains("gpt-5.4 high 1M"));
    }

    #[tokio::test]
    async fn stop_command_restores_most_recent_background_dialog() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .bind_foreground_session_profile(
                "u1",
                Some("thread-older".into()),
                crate::session::state::DialogProfile {
                    model_override: Some("gpt-older".into()),
                    reasoning_effort: Some(ReasoningEffort::Low),
                    service_tier: None,
                    context_mode: Some(ContextMode::Standard),
                },
            )
            .await
            .unwrap();
        session
            .move_foreground_to_background("u1", Some("older"))
            .await
            .unwrap();
        session
            .bind_foreground_session_profile(
                "u1",
                Some("thread-newer".into()),
                crate::session::state::DialogProfile {
                    model_override: Some("gpt-newer".into()),
                    reasoning_effort: Some(ReasoningEffort::High),
                    service_tier: None,
                    context_mode: Some(ContextMode::OneM),
                },
            )
            .await
            .unwrap();
        session
            .move_foreground_to_background("u1", Some("newer"))
            .await
            .unwrap();
        session
            .set_foreground_session_id("u1", Some("thread-current".into()))
            .await
            .unwrap();

        let outcome = maybe_handle_command(
            "/stop",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();

        let CommandOutcome::StopCurrent(reply) = outcome else {
            panic!("expected stop current");
        };
        assert!(reply.contains("`newer`"));
        assert!(reply.contains("gpt-newer high 1M"));
    }

    #[tokio::test]
    async fn stop_command_ends_session() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .set_foreground_session_id("u1", Some("thread".into()))
            .await
            .unwrap();
        let outcome = maybe_handle_command(
            "/stop",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::StopCurrent(_)));
        let snapshot = session.snapshot_for_user("u1").await.unwrap();
        assert!(snapshot.foreground.session_id.is_none());
    }

    #[tokio::test]
    async fn interrupt_command_does_not_end_session() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .set_foreground_session_id("u1", Some("thread".into()))
            .await
            .unwrap();
        let outcome = maybe_handle_command(
            "/interrupt",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            true,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::CancelCurrent(_)));
        let snapshot = session.snapshot_for_user("u1").await.unwrap();
        assert_eq!(snapshot.foreground.session_id.as_deref(), Some("thread"));
    }

    #[tokio::test]
    async fn compact_command_routes_to_manual_compaction() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        let outcome = maybe_handle_command(
            "/compact",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Compact));

        let zh_outcome = maybe_handle_command(
            "/压缩",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        assert!(matches!(zh_outcome, CommandOutcome::Compact));
    }

    #[tokio::test]
    async fn sessions_command_supports_project_then_session_view() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session_dir = global_home.path().join("sessions/2026/04/11");
        fs::create_dir_all(&session_dir).await.unwrap();
        fs::write(
            session_dir.join("rollout-2026-04-11T00-00-00-thread-a.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"thread-a","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp/p1"}}"#,
        )
        .await
        .unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .set_foreground_session_id("u1", Some("thread-a".into()))
            .await
            .unwrap();
        session.save_foreground("u1").await.unwrap();

        let project_list = maybe_handle_command(
            "/sessions",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(project_reply) = project_list else {
            panic!("expected /sessions to reply with project list");
        };
        assert!(project_reply.text.to_lowercase().contains("projects"));

        let session_list = maybe_handle_command(
            "/sessions 1",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(session_reply) = session_list else {
            panic!("expected /sessions 1 to reply with session list");
        };
        assert!(session_reply.text.to_lowercase().contains("no summary"));
        assert!(!session_reply.text.contains("thread-a"));
    }

    #[tokio::test]
    async fn lang_switch_affects_help_output() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        // Default (en) help
        let outcome = maybe_handle_command(
            "/help",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected /help to reply");
        };
        assert!(reply.text.starts_with("# Command Guide"));
        assert!(reply.text.contains("## Basic Commands"));
        assert!(reply.text.contains("## Model Settings"));
        assert!(reply.text.contains("## Session Management"));

        // Switch to zh, verify Chinese
        let outcome = maybe_handle_command(
            "/lang zh",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected /lang zh to reply");
        };
        assert!(reply.text.contains("zh"));

        // Chinese command alias: /帮助
        let outcome = maybe_handle_command(
            "/帮助",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected /帮助 to reply");
        };
        assert!(reply.text.starts_with("# 命令指南"));
        assert!(reply.text.contains("## 基础命令"));
        assert!(reply.text.contains("## 模型设置命令"));
        assert!(reply.text.contains("## 会话管理命令"));
    }

    #[tokio::test]
    async fn alias_add_and_expand_executes_each_step() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        let add_out = maybe_handle_command(
            "/alias add expert /model gpt-5.4 | /reasoning xhigh | /verbose on",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = add_out else {
            panic!("expected /alias add to reply");
        };
        assert!(reply.text.contains("expert"));

        let aliases = session.list_command_aliases("u1").await.unwrap();
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].commands.len(), 3);

        let invoke = maybe_handle_command(
            "/expert",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = invoke else {
            panic!("expected /expert to reply");
        };
        // verbose step should have flipped user state
        let snapshot = session.snapshot_for_user("u1").await.unwrap();
        assert!(snapshot.settings.verbose);
        assert_eq!(
            snapshot.settings.reasoning_effort,
            Some(ReasoningEffort::Xhigh)
        );
        assert!(reply.text.contains("expert"));

        // Protected name rejected
        let protected = maybe_handle_command(
            "/alias add help /status",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = protected else {
            panic!("expected /alias add help to reply");
        };
        assert!(reply.text.to_lowercase().contains("built-in") || reply.text.contains("内置命令"));

        let protected = maybe_handle_command(
            "/alias add compact /status",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = protected else {
            panic!("expected /alias add compact to reply");
        };
        assert!(reply.text.to_lowercase().contains("built-in") || reply.text.contains("内置命令"));
    }

    #[tokio::test]
    async fn alias_names_are_normalized_to_lowercase() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        let add_out = maybe_handle_command(
            "/alias add Expert /verbose on",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = add_out else {
            panic!("expected /alias add to reply");
        };
        assert!(reply.text.contains("/expert"));

        let invoke = maybe_handle_command(
            "/EXPERT",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        assert!(matches!(invoke, CommandOutcome::Reply(_)));
        assert!(
            session
                .snapshot_for_user("u1")
                .await
                .unwrap()
                .settings
                .verbose
        );
    }

    #[tokio::test]
    async fn lang_switch_affects_foreground_switch_messages() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .set_foreground_session_id("u1", Some("thread".into()))
            .await
            .unwrap();
        let _ = maybe_handle_command(
            "/bg focus",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let _ = maybe_handle_command(
            "/lang en",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();

        let outcome = maybe_handle_command(
            "/fg focus",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected /fg to reply");
        };
        assert!(reply.text.contains("Switched to background session"));
    }

    #[tokio::test]
    async fn alias_recursion_capped_at_max_depth() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        // Three-step recursion chain: a -> b -> c -> a (cycle)
        for (name, step) in [("a", "/b"), ("b", "/c"), ("c", "/a")] {
            let add = format!("/alias add {name} {step}");
            let _ = maybe_handle_command(
                &add,
                "u1",
                &session,
                "default",
                &CodexRuntimeProfile::default(),
                false,
            )
            .await
            .unwrap();
        }
        let outcome = maybe_handle_command(
            "/a",
            "u1",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected alias cycle to terminate with reply");
        };
        // Must have produced some text and terminated (no panic / stack overflow)
        assert!(!reply.text.is_empty());
    }

    #[tokio::test]
    async fn model_empty_args_enters_interactive_prompt() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        let outcome = maybe_handle_command(
            "/model",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected /model to prompt");
        };
        assert!(reply.text.to_lowercase().contains("gpt-5.4"));
        assert!(reply.text.contains("`gpt-5.4`"));
        assert!(reply.text.contains("aliases") || reply.text.contains("别名"));
        assert!(
            reply
                .text
                .contains("Most capable general-purpose model currently available")
                || reply.text.contains("目前最先进的通用型模型")
        );
        let snapshot = session.snapshot_for_user("u1").await.unwrap();
        assert!(matches!(
            snapshot.pending_setting,
            Some(crate::session::state::PendingSetting::Model)
        ));
    }

    #[tokio::test]
    async fn model_prompt_keeps_hint_out_of_markdown_sublist() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .update_settings_for_user("u1", |state| {
                state.language = "zh".into();
            })
            .await
            .unwrap();

        let outcome = maybe_handle_command(
            "/model",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected /model to prompt");
        };

        assert!(
            reply.text.contains("\n\n请输入一个值，或 `/返回` 取消。"),
            "hint should be separated from the markdown list: {}",
            reply.text
        );
    }

    #[tokio::test]
    async fn pending_model_fuzzy_match_applies_and_clears() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        let _ = maybe_handle_command(
            "/model",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();

        // Ambiguous prefix: "gpt-5." hits multiple canonical models.
        let outcome = maybe_handle_command(
            "gpt-5.",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply for ambiguous match");
        };
        assert!(
            reply.text.to_lowercase().contains("multiple") || reply.text.contains("匹配到多个")
        );
        let snapshot = session.snapshot_for_user("u1").await.unwrap();
        assert!(
            snapshot.pending_setting.is_some(),
            "pending must stay on ambiguous input"
        );

        // Unique fuzzy: "mini" hits only gpt-5.4-mini
        let outcome = maybe_handle_command(
            "mini",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected apply reply");
        };
        assert!(reply.text.contains("gpt-5.4-mini"));
        let snapshot = session.snapshot_for_user("u1").await.unwrap();
        assert!(
            snapshot.pending_setting.is_none(),
            "pending should clear on apply"
        );
        assert_eq!(
            snapshot.settings.model_override.as_deref(),
            Some("gpt-5.4-mini")
        );
    }

    #[tokio::test]
    async fn back_exits_pending_and_reports_idle_otherwise() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        // idle /back → "not in any interactive setting"
        let outcome = maybe_handle_command(
            "/back",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply");
        };
        assert!(
            reply.text.to_lowercase().contains("not currently") || reply.text.contains("当前没有")
        );

        // Enter reasoning pending, then /back exits it.
        let _ = maybe_handle_command(
            "/reasoning",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        assert!(
            session
                .snapshot_for_user("u1")
                .await
                .unwrap()
                .pending_setting
                .is_some()
        );
        let outcome = maybe_handle_command(
            "/back",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply");
        };
        assert!(reply.text.contains("/reasoning"));
        assert!(
            session
                .snapshot_for_user("u1")
                .await
                .unwrap()
                .pending_setting
                .is_none()
        );
    }

    #[tokio::test]
    async fn other_command_during_pending_clears_and_prefixes() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        let _ = maybe_handle_command(
            "/model",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let outcome = maybe_handle_command(
            "/status",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply");
        };
        assert!(
            reply.text.contains("/model"),
            "exit notice must name the prior command"
        );
        assert!(reply.text.to_lowercase().contains("workdir"));
        assert!(
            session
                .snapshot_for_user("u1")
                .await
                .unwrap()
                .pending_setting
                .is_none()
        );
    }

    #[tokio::test]
    async fn status_uses_context_window_remaining_format() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .set_foreground_usage(
                "u1",
                TokenUsageSnapshot {
                    total_tokens: 13_700,
                    window: 272_000,
                    input_tokens: 0,
                    cached_input_tokens: 0,
                    output_tokens: 0,
                    updated_at: chrono::Utc::now(),
                },
            )
            .await
            .unwrap();

        let outcome = maybe_handle_command(
            "/status",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply");
        };
        assert!(
            reply.text.contains("99% left"),
            "unexpected status: {}",
            reply.text
        );
        assert!(reply.text.contains("14K used / 272K"));
    }

    #[tokio::test]
    async fn status_hides_implausible_legacy_cumulative_usage() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .set_foreground_usage(
                "u1",
                TokenUsageSnapshot {
                    total_tokens: 19_668_612,
                    window: 1_000_000,
                    input_tokens: 19_568_077,
                    cached_input_tokens: 18_968_448,
                    output_tokens: 100_535,
                    updated_at: chrono::Utc::now(),
                },
            )
            .await
            .unwrap();

        let outcome = maybe_handle_command(
            "/status",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply");
        };
        assert!(
            reply.text.contains("context window: —"),
            "unexpected status: {}",
            reply.text
        );
    }

    #[tokio::test]
    async fn chinese_command_aliases_route_correctly() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        // /模型 should enter the same interactive Model pending as /model.
        let outcome = maybe_handle_command(
            "/模型",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
        assert!(matches!(
            session
                .snapshot_for_user("u1")
                .await
                .unwrap()
                .pending_setting,
            Some(crate::session::state::PendingSetting::Model)
        ));

        let _ = maybe_handle_command(
            "/语言 zh",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();

        let _ = maybe_handle_command(
            "/模型",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();

        // /返回 clears.
        let outcome = maybe_handle_command(
            "/返回",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply");
        };
        assert!(reply.text.contains("/模型"));
        assert!(!reply.text.contains("/model"));
        assert!(
            session
                .snapshot_for_user("u1")
                .await
                .unwrap()
                .pending_setting
                .is_none()
        );
    }

    #[tokio::test]
    async fn reasoning_prompt_uses_supported_values_and_aliases() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .update_settings_for_user("u1", |state| {
                state.language = "zh".into();
            })
            .await
            .unwrap();

        let outcome = maybe_handle_command(
            "/reasoning",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected /reasoning to prompt");
        };
        assert!(reply.text.contains("当前思考深度：medium"));
        assert!(
            reply
                .text
                .contains("low (低), medium (中), high (高), xhigh (超高), inherit (默认)")
        );
        assert!(reply.text.contains("\n请输入一个值，或 `/返回` 取消。"));
        assert!(!reply.text.contains("- `low`"));
        assert!(!reply.text.contains("恢复默认"));
    }

    #[tokio::test]
    async fn reasoning_prompt_uses_compact_three_line_layout() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .update_settings_for_user("u1", |state| {
                state.language = "zh".into();
            })
            .await
            .unwrap();

        let outcome = maybe_handle_command(
            "/reasoning",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected /reasoning to prompt");
        };
        assert_eq!(
            reply.text.lines().count(),
            3,
            "unexpected prompt: {}",
            reply.text
        );
    }

    #[tokio::test]
    async fn pending_reasoning_alias_applies_supported_value() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        let _ = maybe_handle_command(
            "/reasoning",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();

        let outcome = maybe_handle_command(
            "高",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected apply reply");
        };
        assert!(reply.text.contains("high"));
        let snapshot = session.snapshot_for_user("u1").await.unwrap();
        assert_eq!(
            snapshot.settings.reasoning_effort,
            Some(ReasoningEffort::High)
        );
        assert!(snapshot.pending_setting.is_none());
    }

    #[tokio::test]
    async fn direct_fast_and_context_commands_accept_chinese_aliases() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        let fast = maybe_handle_command(
            "/fast 开",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(fast_reply) = fast else {
            panic!("expected /fast 开 to reply");
        };
        assert!(fast_reply.text.contains("fast"));

        let context = maybe_handle_command(
            "/context 长",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(context_reply) = context else {
            panic!("expected /context 长 to reply");
        };
        assert!(context_reply.text.contains("1M"));

        let snapshot = session.snapshot_for_user("u1").await.unwrap();
        assert_eq!(snapshot.settings.service_tier, Some(ServiceTier::Fast));
        assert_eq!(snapshot.settings.context_mode, Some(ContextMode::OneM));
    }

    #[tokio::test]
    async fn fg_prompt_keeps_hint_out_of_markdown_sublist() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .set_foreground_session_id("u1", Some("thread".into()))
            .await
            .unwrap();
        let _ = maybe_handle_command(
            "/bg focus",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        session
            .update_settings_for_user("u1", |state| {
                state.language = "zh".into();
            })
            .await
            .unwrap();

        let outcome = maybe_handle_command(
            "/fg",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected /fg to prompt");
        };
        assert!(
            reply.text.contains("\n\n请输入一个值，或 `/返回` 取消。"),
            "hint should be separated from the markdown list: {}",
            reply.text
        );
    }

    #[tokio::test]
    async fn help_groups_commands_in_requested_order() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        session
            .update_settings_for_user("u1", |state| {
                state.language = "zh".into();
            })
            .await
            .unwrap();

        let outcome = maybe_handle_command(
            "/帮助",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected /帮助 to reply");
        };

        let model_section = reply
            .text
            .find("## 模型设置命令")
            .expect("missing model settings section");
        let session_section = reply
            .text
            .find("## 会话管理命令")
            .expect("missing session management section");
        let advanced_section = reply
            .text
            .find("## 高级命令")
            .expect("missing advanced section");
        let model_index = reply.text.find("`/模型`").expect("missing /模型");
        let reasoning_index = reply.text.find("`/思考`").expect("missing /思考");
        let fast_index = reply.text.find("`/快速`").expect("missing /快速");
        let context_index = reply.text.find("`/上下文`").expect("missing /上下文");
        let sessions_index = reply.text.find("`/会话`").expect("missing /会话");
        let import_index = reply.text.find("`/导入`").expect("missing /导入");
        let resume_index = reply.text.find("`/恢复`").expect("missing /恢复");
        let save_index = reply.text.find("`/保存`").expect("missing /保存");
        let fg_index = reply.text.find("`/前台`").expect("missing /前台");
        let bg_index = reply.text.find("`/后台`").expect("missing /后台");
        let loadbg_index = reply.text.find("`/载入后台`").expect("missing /载入后台");
        let rename_index = reply.text.find("`/重命名`").expect("missing /重命名");
        let alias_index = reply.text.find("`/别名`").expect("missing /别名");
        let verbose_index = reply.text.find("`/详细`").expect("missing /详细");
        let compact_index = reply.text.find("`/压缩`").expect("missing /压缩");
        let self_update_index = reply.text.find("`/自更新`").expect("missing /自更新");

        assert!(model_section < session_section && session_section < advanced_section);
        assert!(
            model_index < reasoning_index
                && reasoning_index < fast_index
                && fast_index < context_index
        );
        assert!(
            sessions_index < import_index
                && import_index < resume_index
                && resume_index < save_index
        );
        assert!(
            compact_index < fg_index
                && fg_index < bg_index
                && bg_index < loadbg_index
                && loadbg_index < rename_index
                && rename_index < alias_index
                && alias_index < verbose_index
                && verbose_index < self_update_index
        );
    }

    #[tokio::test]
    async fn help_entry_rendered_in_active_language_only() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        // English: /help should show English command names, no Chinese.
        let outcome = maybe_handle_command(
            "/help",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply");
        };
        assert!(reply.text.contains("`/model`"));
        assert!(!reply.text.contains("`/模型`"));
        assert!(reply.text.contains("`/compact`"));
        assert!(!reply.text.contains("`/压缩`"));

        // Switch to zh and the same /help should mirror the behavior.
        let _ = maybe_handle_command(
            "/lang zh",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let outcome = maybe_handle_command(
            "/help",
            "u1",
            &session,
            "gpt-5.4",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        let CommandOutcome::Reply(reply) = outcome else {
            panic!("expected reply");
        };
        assert!(reply.text.contains("`/模型`"));
        assert!(!reply.text.contains("`/model`"));
        assert!(reply.text.contains("`/压缩`"));
        assert!(!reply.text.contains("`/compact`"));
    }
}
