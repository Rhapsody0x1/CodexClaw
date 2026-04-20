use std::{collections::BTreeMap, future::Future, path::PathBuf, pin::Pin};

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use rust_i18n::t;

use crate::{
    codex::runtime::CodexRuntimeProfile,
    normalize_lang,
    session::{
        state::{CommandAlias, ContextMode, ReasoningEffort, ServiceTier, UserSessionState},
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
    "self-update",
    "alias",
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
        if !text.trim_start().starts_with('/') {
            return Ok(CommandOutcome::Continue);
        }
        let trimmed = text.trim();
        let mut parts = trimmed.split_whitespace();
        let raw_command = parts.next().unwrap_or_default().to_ascii_lowercase();
        let command = canonicalize_core_command(&raw_command).to_string();
        let rest = parts.collect::<Vec<_>>();
        match command.as_str() {
            "/help" => Ok(CommandOutcome::Reply(CommandReply {
                text: help_text(&session.snapshot_for_user(openid).await?.settings.language),
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
            "/interrupt" => {
                let lang = session
                    .snapshot_for_user(openid)
                    .await?
                    .settings
                    .language
                    .clone();
                Ok(CommandOutcome::CancelCurrent(
                    t!("errors.interrupt_requested", locale = lang.as_str()).into_owned(),
                ))
            }
            "/self-update" => Ok(CommandOutcome::SelfUpdate),
            "/alias" => handle_alias(&rest, openid, session).await,
            other => {
                let alias_name = other.trim_start_matches('/').to_ascii_lowercase();
                if !alias_name.is_empty()
                    && let Some(alias) = session.get_command_alias(openid, &alias_name).await?
                {
                    return expand_alias(
                        &alias,
                        openid,
                        session,
                        default_model,
                        runtime_profile,
                        is_busy,
                        alias_depth,
                    )
                    .await;
                }
                Ok(CommandOutcome::Continue)
            }
        }
    })
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
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
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
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.fast.status",
                value = effective_fast_label(&snapshot, runtime_profile),
                locale = lang.as_str()
            )
            .into_owned(),
        }));
    }
    let next = if args[0].eq_ignore_ascii_case("inherit") {
        None
    } else {
        Some(ServiceTier::parse(args[0]).ok_or_else(|| {
            anyhow!(t!("commands.fast.usage", locale = lang.as_str()).into_owned())
        })?)
    };
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
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.context.status",
                value = effective_context_label(&snapshot, runtime_profile),
                locale = lang.as_str()
            )
            .into_owned(),
        }));
    }
    let next = if args[0].eq_ignore_ascii_case("inherit") {
        None
    } else {
        Some(ContextMode::parse(args[0]).ok_or_else(|| {
            anyhow!(t!("commands.context.usage", locale = lang.as_str()).into_owned())
        })?)
    };
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
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!(
                "commands.reasoning.status",
                value = effective_reasoning(&snapshot, runtime_profile),
                locale = lang.as_str()
            )
            .into_owned(),
        }));
    }
    let next = if args[0].eq_ignore_ascii_case("inherit") {
        None
    } else {
        Some(ReasoningEffort::parse(args[0]).ok_or_else(|| {
            anyhow!(t!("commands.reasoning.invalid", locale = lang.as_str()).into_owned())
        })?)
    };
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
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        let snapshot = session.snapshot_for_user(openid).await?;
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
    let lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    let Some(alias) = args.first() else {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!("commands.fg.usage", locale = lang.as_str()).into_owned(),
        }));
    };
    let switched = session.foreground_from_background(openid, alias).await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    let preview = foreground_last_user_message(session, openid, &snapshot).await?;
    let parked = switched
        .parked_alias
        .map(|value| {
            t!(
                "commands.fg.parked",
                alias = value.as_str(),
                locale = lang.as_str()
            )
            .into_owned()
        })
        .unwrap_or_default();
    Ok(CommandOutcome::Reply(CommandReply {
        text: t!(
            "commands.fg.switched",
            parked = parked,
            alias = *alias,
            runtime = format_effective_runtime_text(
                &snapshot,
                default_model,
                runtime_profile,
                preview.as_deref()
            ),
            locale = lang.as_str()
        )
        .into_owned(),
    }))
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
    let Some(selector) = args.first() else {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!("commands.resume.usage", locale = lang.as_str()).into_owned(),
        }));
    };
    let sessions = session
        .list_disk_sessions(openid, SessionListScope::All)
        .await?;
    let target = resolve_selector(
        selector,
        &sessions,
        &session.last_sessions_view(openid).await?,
        lang.as_str(),
    )?;
    let switched = session.resume_disk_session(openid, &target).await?;
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
    Ok(CommandOutcome::Reply(CommandReply {
        text: t!(
            "commands.resume.restored",
            parked = parked,
            summary = session_summary(&target, lang.as_str()),
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
    let Some(selector) = args.first() else {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: t!("commands.loadbg.usage", locale = lang.as_str()).into_owned(),
        }));
    };
    let sessions = session
        .list_disk_sessions(openid, SessionListScope::All)
        .await?;
    let target = resolve_selector(
        selector,
        &sessions,
        &session.last_sessions_view(openid).await?,
        lang.as_str(),
    )?;
    let alias = session
        .load_disk_session_to_background(openid, &target, args.get(1).copied())
        .await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: t!(
            "commands.loadbg.loaded",
            alias = alias.as_str(),
            summary = session_summary(&target, lang.as_str()),
            locale = lang.as_str()
        )
        .into_owned(),
    }))
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
        session.set_last_projects_view(openid, project_keys).await?;
        session.set_last_sessions_view(openid, Vec::new()).await?;
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
        session
            .set_last_import_projects_view(openid, project_keys)
            .await?;
        session
            .set_last_import_sessions_view(openid, Vec::new())
            .await?;
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
    let window = if usage.window > 0 {
        usage.window
    } else {
        effective_context_window(state, runtime_profile)
    };
    let percent = if window == 0 {
        0
    } else {
        ((usage.total_tokens as f64 / window as f64) * 100.0).round() as u64
    };
    t!(
        "commands.status.context_usage",
        percent = percent,
        used = format_tokens_compact(usage.total_tokens),
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
        "/状态" | "/会话" | "/session" => "/status",
        "/模型" => "/model",
        "/快速" => "/fast",
        "/上下文" => "/context",
        "/思考" => "/reasoning",
        "/别名" => "/alias",
        "/语言" => "/lang",
        other => other,
    }
}

async fn handle_lang(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
) -> Result<CommandOutcome> {
    let current_lang = session
        .snapshot_for_user(openid)
        .await?
        .settings
        .language
        .clone();
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
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
        t!("commands.help.section_advanced", locale = lang).into_owned(),
        t!("commands.help.entry_sessions", locale = lang).into_owned(),
        t!("commands.help.entry_import", locale = lang).into_owned(),
        t!("commands.help.entry_resume", locale = lang).into_owned(),
        t!("commands.help.entry_loadbg", locale = lang).into_owned(),
        t!("commands.help.entry_bg", locale = lang).into_owned(),
        t!("commands.help.entry_fg", locale = lang).into_owned(),
        t!("commands.help.entry_rename", locale = lang).into_owned(),
        t!("commands.help.entry_save", locale = lang).into_owned(),
        t!("commands.help.entry_model", locale = lang).into_owned(),
        t!("commands.help.entry_fast", locale = lang).into_owned(),
        t!("commands.help.entry_context", locale = lang).into_owned(),
        t!("commands.help.entry_reasoning", locale = lang).into_owned(),
        t!("commands.help.entry_verbose", locale = lang).into_owned(),
        t!("commands.help.entry_alias", locale = lang).into_owned(),
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

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use tokio::fs;

    use crate::{
        codex::runtime::CodexRuntimeProfile,
        session::{
            state::{ContextMode, ReasoningEffort, ServiceTier},
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
}
