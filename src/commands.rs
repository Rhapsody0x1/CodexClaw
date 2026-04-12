use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};

use crate::{
    codex::runtime::CodexRuntimeProfile,
    session::{
        state::{ContextMode, ReasoningEffort, ServiceTier, UserSessionState},
        store::{DiskSessionMeta, SessionListScope, SessionStore},
    },
};

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
    if !text.trim_start().starts_with('/') {
        return Ok(CommandOutcome::Continue);
    }
    let trimmed = text.trim();
    let mut parts = trimmed.split_whitespace();
    let command = parts.next().unwrap_or_default().to_ascii_lowercase();
    let rest = parts.collect::<Vec<_>>();
    match command.as_str() {
        "/help" => Ok(CommandOutcome::Reply(CommandReply { text: help_text() })),
        "/bind" => Ok(CommandOutcome::Reply(CommandReply {
            text: "绑定/授权限制已禁用，所有私聊用户都可直接使用机器人。".to_string(),
        })),
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
        "/plan" => {
            handle_plan(
                &rest,
                openid,
                session,
                default_model,
                runtime_profile,
                is_busy,
            )
            .await
        }
        "/status" | "/session" => Ok(CommandOutcome::Reply(CommandReply {
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
            "已请求停止当前运行。".to_string(),
        )),
        "/self-update" => Ok(CommandOutcome::SelfUpdate),
        _ => Ok(CommandOutcome::Continue),
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
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        let active_override = merged_settings(&snapshot).model_override;
        return Ok(CommandOutcome::Reply(CommandReply {
            text: format!(
                "model: {}\nmodel_override: {}\n提示：完整会话状态请用 `/status`。",
                effective_model(&snapshot, default_model, runtime_profile),
                active_override.as_deref().unwrap_or("inherit"),
            ),
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
        text: format!(
            "模型已更新为：{}",
            effective_model(&snapshot, default_model, runtime_profile)
        ),
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
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: format!(
                "fast: {}\n提示：完整会话状态请用 `/status`。",
                effective_fast_label(&snapshot, runtime_profile)
            ),
        }));
    }
    let next = if args[0].eq_ignore_ascii_case("inherit") {
        None
    } else {
        Some(
            ServiceTier::parse(args[0])
                .ok_or_else(|| anyhow!("用法：`/fast [on|off|inherit|status]`"))?,
        )
    };
    session.set_service_tier_for_active(openid, next).await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: format!(
            "fast 已更新为：{}",
            effective_fast_label(&snapshot, runtime_profile)
        ),
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
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: format!(
                "context: {}\n提示：完整会话状态请用 `/status`。",
                effective_context_label(&snapshot, runtime_profile)
            ),
        }));
    }
    let next = if args[0].eq_ignore_ascii_case("inherit") {
        None
    } else {
        Some(
            ContextMode::parse(args[0])
                .ok_or_else(|| anyhow!("用法：`/context [1m|standard|inherit|status]`"))?,
        )
    };
    session.set_context_mode_for_active(openid, next).await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: format!(
            "上下文模式已更新为：{}",
            effective_context_label(&snapshot, runtime_profile)
        ),
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
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: format!(
                "reasoning: {}\n提示：完整会话状态请用 `/status`。",
                effective_reasoning(&snapshot, runtime_profile)
            ),
        }));
    }
    let next = if args[0].eq_ignore_ascii_case("inherit") {
        None
    } else {
        Some(ReasoningEffort::parse(args[0]).ok_or_else(|| {
            anyhow!("无效思考深度：可选 none|minimal|low|medium|high|xhigh|inherit")
        })?)
    };
    session.set_reasoning_for_active(openid, next).await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: format!(
            "思考深度已更新为：{}",
            effective_reasoning(&snapshot, runtime_profile)
        ),
    }))
}

async fn handle_plan(
    args: &[&str],
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    _runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        let snapshot = session.snapshot_for_user(openid).await?;
        return Ok(CommandOutcome::Reply(CommandReply {
            text: format!(
                "plan: {}\n提示：完整会话状态请用 `/status`。",
                if snapshot.settings.plan_mode {
                    "on"
                } else {
                    "off"
                }
            ),
        }));
    }
    let enabled = match args[0].to_ascii_lowercase().as_str() {
        "on" => true,
        "off" => false,
        _ => {
            return Ok(CommandOutcome::Reply(CommandReply {
                text: "用法：`/plan [on|off|status]`".to_string(),
            }));
        }
    };
    session
        .update_settings_for_user(openid, |state| state.plan_mode = enabled)
        .await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: format!(
            "plan 已更新为：{}",
            if snapshot.settings.plan_mode {
                "on"
            } else {
                "off"
            }
        ),
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
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        let snapshot = session.snapshot_for_user(openid).await?;
        return Ok(CommandOutcome::Reply(CommandReply {
            text: format!(
                "verbose: {}\n提示：完整会话状态请用 `/status`。",
                if snapshot.settings.verbose {
                    "on"
                } else {
                    "off"
                }
            ),
        }));
    }
    let enabled = match args[0].to_ascii_lowercase().as_str() {
        "on" | "true" => true,
        "off" | "false" => false,
        _ => {
            return Ok(CommandOutcome::Reply(CommandReply {
                text: "用法：`/verbose [on|off|status]`".to_string(),
            }));
        }
    };
    session
        .update_settings_for_user(openid, |state| state.verbose = enabled)
        .await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: format!(
            "verbose 已更新为：{}",
            if snapshot.settings.verbose {
                "on"
            } else {
                "off"
            }
        ),
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
        .map(|alias| format!("已将原前台会话转入后台：`{alias}`\n"))
        .unwrap_or_default();
    let snapshot = session.snapshot_for_user(openid).await?;
    let mut lines = vec![if raw_args.trim().is_empty() {
        "已创建新的临时前台会话。".to_string()
    } else {
        format!(
            "已创建新的临时前台会话。\n工作目录: `{}`",
            snapshot.foreground.workspace_dir.display()
        )
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
    let moved = session
        .move_foreground_to_background(openid, args.first().copied())
        .await?;
    let text = if let Some(alias) = moved.parked_alias {
        format!("前台会话已转为后台：`{alias}`。")
    } else {
        "当前前台是空白临时会话，已重置为新的临时会话。".to_string()
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
    let Some(alias) = args.first() else {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: "用法：`/fg <alias>`".to_string(),
        }));
    };
    let switched = session.foreground_from_background(openid, alias).await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    let preview = foreground_last_user_message(session, openid, &snapshot).await?;
    let parked = switched
        .parked_alias
        .map(|value| format!("原前台已转入后台：`{value}`。\n"))
        .unwrap_or_default();
    Ok(CommandOutcome::Reply(CommandReply {
        text: format!(
            "{}已切换到后台会话 `{}`。\n{}",
            parked,
            alias,
            format_effective_runtime_text(
                &snapshot,
                default_model,
                runtime_profile,
                preview.as_deref()
            )
        ),
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
    let Some(selector) = args.first() else {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: "用法：`/resume <编号或会话ID>`（先用 `/sessions` 查看项目，再 `/sessions <项目编号>` 查看会话）"
                .to_string(),
        }));
    };
    let sessions = session
        .list_disk_sessions(openid, SessionListScope::All)
        .await?;
    let target = resolve_selector(
        selector,
        &sessions,
        &session.last_sessions_view(openid).await?,
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
        .map(|value| format!("原前台已转入后台：`{value}`。\n"))
        .unwrap_or_default();
    Ok(CommandOutcome::Reply(CommandReply {
        text: format!(
            "{}已恢复会话：{}（workspace: `{}`）。\n{}",
            parked,
            session_summary(&target),
            workspace_display,
            format_effective_runtime_text(
                &snapshot,
                default_model,
                runtime_profile,
                target.last_user_message.as_deref(),
            ),
        ),
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
    let Some(selector) = args.first() else {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: "用法：`/loadbg <编号或会话ID> [alias]`".to_string(),
        }));
    };
    let sessions = session
        .list_disk_sessions(openid, SessionListScope::All)
        .await?;
    let target = resolve_selector(
        selector,
        &sessions,
        &session.last_sessions_view(openid).await?,
    )?;
    let alias = session
        .load_disk_session_to_background(openid, &target, args.get(1).copied())
        .await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: format!(
            "已加载会话到后台标签 `{}`：{}。",
            alias,
            session_summary(&target),
        ),
    }))
}

async fn handle_save(
    openid: &str,
    session: &SessionStore,
    _default_model: &str,
    _runtime_profile: &CodexRuntimeProfile,
    _is_busy: bool,
) -> Result<CommandOutcome> {
    let changed = session.save_foreground(openid).await?;
    let prefix = if changed {
        "前台会话已标记为持久保存。"
    } else {
        "前台会话已处于保存状态。"
    };
    Ok(CommandOutcome::Reply(CommandReply {
        text: prefix.to_string(),
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
    if args.len() != 2 {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: "用法：`/rename <old_alias> <new_alias>`".to_string(),
        }));
    }
    session
        .rename_background_alias(openid, args[0], args[1])
        .await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: format!("后台标签已重命名：`{}` -> `{}`", args[0], args[1]),
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
    if args.is_empty() || is_scope_token(args[0]) {
        let scope = if args.is_empty() {
            SessionListScope::All
        } else {
            parse_scope(args[0])?
        };
        let sessions = session.list_disk_sessions(openid, scope).await?;
        let projects = collect_projects(&sessions);
        let (text, project_keys) = format_projects_list(&projects);
        session.set_last_projects_view(openid, project_keys).await?;
        session.set_last_sessions_view(openid, Vec::new()).await?;
        return Ok(CommandOutcome::Reply(CommandReply { text }));
    }

    let selector = args[0];
    let page = args
        .get(1)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    let project_key =
        resolve_project_selector(selector, &session.last_projects_view(openid).await?)?;
    let (scope, project_path) = decode_project_key(&project_key)?;
    let all_sessions = session.list_disk_sessions(openid, scope).await?;
    let sessions = all_sessions
        .into_iter()
        .filter(|item| item.cwd.display().to_string() == project_path)
        .collect::<Vec<_>>();
    let (text, ids) = format_project_sessions_page(&project_path, &sessions, page);
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
    let all = session.list_importable_sessions()?;
    let last_session_view = session.last_import_sessions_view(openid).await?;
    if let Some(selector) = args.first()
        && !last_session_view.is_empty()
        && let Ok(target) = resolve_selector(selector, &all, &last_session_view)
    {
        let result = session.import_disk_session(&target).await?;
        let profile = result.profile;
        let action = if result.copied {
            "已导入会话"
        } else {
            "会话已存在，已刷新导入配置"
        };
        return Ok(CommandOutcome::Reply(CommandReply {
            text: format!(
                "{action}：{}\n工作目录: `{}`\n模型: {}",
                session_summary(&target),
                profile.workspace_dir.display(),
                compact_imported_profile_summary(&profile),
            ),
        }));
    }

    if args.is_empty() {
        let projects = collect_projects(&all);
        let (text, project_keys) = format_import_projects_list(&projects);
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
    let project_key = match resolve_project_selector(selector, &import_projects) {
        Ok(value) => value,
        Err(_) => {
            let target = resolve_selector(selector, &all, &last_session_view)?;
            let result = session.import_disk_session(&target).await?;
            let profile = result.profile;
            let action = if result.copied {
                "已导入会话"
            } else {
                "会话已存在，已刷新导入配置"
            };
            return Ok(CommandOutcome::Reply(CommandReply {
                text: format!(
                    "{action}：{}\n工作目录: `{}`\n模型: {}",
                    session_summary(&target),
                    profile.workspace_dir.display(),
                    compact_imported_profile_summary(&profile),
                ),
            }));
        }
    };
    let (_, project_path) = decode_project_key(&project_key)?;
    let project_sessions = all
        .into_iter()
        .filter(|item| item.cwd.display().to_string() == project_path)
        .collect::<Vec<_>>();
    let (text, ids) = format_import_project_sessions_page(&project_path, &project_sessions, page);
    session.set_last_import_sessions_view(openid, ids).await?;
    Ok(CommandOutcome::Reply(CommandReply { text }))
}

async fn handle_stop(
    openid: &str,
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
) -> Result<CommandOutcome> {
    let result = session.stop_foreground(openid).await?;
    let summary = if let Some(alias) = result.restored_alias.as_deref() {
        let snapshot = session.snapshot_for_user(openid).await?;
        let preview = foreground_last_user_message(session, openid, &snapshot).await?;
        let prefix = if !result.had_session {
            "前台没有可结束的会话。".to_string()
        } else if result.saved {
            "前台会话已结束并保留。".to_string()
        } else if result.dropped_unsaved {
            "前台会话已结束并丢弃（未保存）。".to_string()
        } else {
            "前台会话已结束。".to_string()
        };
        format!(
            "{prefix} 已自动切回最近的后台会话 `{alias}`。\n{}",
            format_effective_runtime_text(
                &snapshot,
                default_model,
                runtime_profile,
                preview.as_deref()
            )
        )
    } else if !result.had_session {
        "前台没有可结束的会话，已重置为新的临时会话。".to_string()
    } else if result.saved {
        "前台会话已结束并保留。已创建新的临时前台会话。".to_string()
    } else if result.dropped_unsaved {
        "前台会话已结束并丢弃（未保存）。已创建新的临时前台会话。".to_string()
    } else {
        "前台会话已结束。已创建新的临时前台会话。".to_string()
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
    let bg_aliases = if state.background.is_empty() {
        "无".to_string()
    } else {
        state
            .background
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "工作目录: {}\n模型: {}\n详细输出: {}\n计划模式: {}\n后台对话: {}\n前台状态: {}",
        state.foreground.workspace_dir.display(),
        compact_runtime_summary(state, default_model, runtime_profile),
        if effective.verbose {
            "开启"
        } else {
            "关闭"
        },
        if effective.plan_mode {
            "开启"
        } else {
            "关闭"
        },
        bg_aliases,
        if is_busy { "运行中" } else { "空闲" },
    )
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
    let mut lines = Vec::new();
    if let Some(preview) = preview {
        lines.push(format!("最近用户消息: {}", single_line(preview, 48)));
    }
    lines.push(format!(
        "模型: {}",
        compact_runtime_summary(state, default_model, runtime_profile)
    ));
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
) -> String {
    compact_model_summary(
        profile
            .model_override
            .clone()
            .unwrap_or_else(|| "继承默认".to_string()),
        profile
            .reasoning_effort
            .map(|value| value.as_str())
            .unwrap_or("继承默认"),
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

fn parse_scope(value: &str) -> Result<SessionListScope> {
    match value.to_ascii_lowercase().as_str() {
        "all" => Ok(SessionListScope::All),
        _ => Err(anyhow!("范围仅支持 all")),
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

fn format_projects_list(projects: &[ProjectBucket]) -> (String, Vec<String>) {
    if projects.is_empty() {
        return (
            "没有可用会话。可先发起对话并使用 `/save` 或 `/bg` 保存。".to_string(),
            Vec::new(),
        );
    }
    let mut lines = vec![format!("项目列表 total={}", projects.len())];
    let mut keys = Vec::new();
    for (index, project) in projects.iter().enumerate() {
        let latest = project
            .latest
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!(
            "{}. {} | sessions={} | latest={}",
            index + 1,
            project.path,
            project.count,
            latest,
        ));
        keys.push(encode_project_key(SessionListScope::All, &project.path));
    }
    lines.push("\n使用 `/sessions <项目编号> [page]` 查看项目中的会话。".to_string());
    (lines.join("\n"), keys)
}

fn format_import_projects_list(projects: &[ProjectBucket]) -> (String, Vec<String>) {
    if projects.is_empty() {
        return (
            "系统 `~/.codex/sessions` 中没有可导入会话。".to_string(),
            Vec::new(),
        );
    }
    let mut lines = vec![format!("可导入项目 total={}", projects.len())];
    let mut keys = Vec::new();
    for (index, project) in projects.iter().enumerate() {
        let latest = project
            .latest
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!(
            "{}. {} | sessions={} | latest={}",
            index + 1,
            project.path,
            project.count,
            latest,
        ));
        keys.push(encode_project_key(SessionListScope::All, &project.path));
    }
    lines.push("\n使用 `/import <项目编号> [page]` 查看该项目中的会话。".to_string());
    (lines.join("\n"), keys)
}

fn format_project_sessions_page(
    project_path: &str,
    sessions: &[DiskSessionMeta],
    page: usize,
) -> (String, Vec<String>) {
    if sessions.is_empty() {
        return (format!("项目 `{}` 下暂无会话。", project_path), Vec::new());
    }
    let page_size = 12usize;
    let safe_page = page.max(1);
    let start = (safe_page - 1) * page_size;
    if start >= sessions.len() {
        return (
            format!(
                "页码超出范围：项目 `{}` 共 {} 条，会话页大小 {}，当前页 {}。",
                project_path,
                sessions.len(),
                page_size,
                safe_page
            ),
            Vec::new(),
        );
    }
    let end = (start + page_size).min(sessions.len());
    let mut lines = vec![format!(
        "项目会话 project={} page={}/{} (total={})",
        project_path,
        safe_page,
        sessions.len().div_ceil(page_size),
        sessions.len(),
    )];
    let mut view_ids = Vec::new();
    for (offset, session) in sessions[start..end].iter().enumerate() {
        let index = offset + 1;
        let summary = session_summary(session);
        let updated = session
            .updated_at
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!("{}. {} | {}", index, updated, summary));
        view_ids.push(session.id.clone());
    }
    lines.push(
        "\n使用 `/resume <编号|会话ID>` 恢复前台，或 `/loadbg <编号|会话ID> [alias]` 加载到后台。"
            .to_string(),
    );
    (lines.join("\n"), view_ids)
}

fn format_import_project_sessions_page(
    project_path: &str,
    sessions: &[DiskSessionMeta],
    page: usize,
) -> (String, Vec<String>) {
    if sessions.is_empty() {
        return (
            format!("项目 `{}` 下暂无可导入会话。", project_path),
            Vec::new(),
        );
    }
    let page_size = 12usize;
    let safe_page = page.max(1);
    let start = (safe_page - 1) * page_size;
    if start >= sessions.len() {
        return (
            format!(
                "页码超出范围：项目 `{}` 共 {} 条，会话页大小 {}，当前页 {}。",
                project_path,
                sessions.len(),
                page_size,
                safe_page
            ),
            Vec::new(),
        );
    }
    let end = (start + page_size).min(sessions.len());
    let mut lines = vec![format!(
        "可导入会话 project={} page={}/{} (total={})",
        project_path,
        safe_page,
        sessions.len().div_ceil(page_size),
        sessions.len(),
    )];
    let mut view_ids = Vec::new();
    for (offset, session) in sessions[start..end].iter().enumerate() {
        let index = offset + 1;
        let summary = session_summary(session);
        let updated = session
            .updated_at
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!("{index}. [{updated}] {summary}"));
        view_ids.push(session.id.clone());
    }
    lines.push("\n使用 `/import <编号|会话ID>` 导入会话。".to_string());
    (lines.join("\n"), view_ids)
}

fn session_summary(session: &DiskSessionMeta) -> String {
    let raw = session
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("无摘要");
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
        return Ok((parse_scope(scope_raw)?, path.to_string()));
    }
    Ok((SessionListScope::All, key.to_string()))
}

fn resolve_project_selector(selector: &str, last_projects_view: &[String]) -> Result<String> {
    if let Ok(index) = selector.parse::<usize>()
        && index >= 1
        && index <= last_projects_view.len()
    {
        return Ok(last_projects_view[index - 1].clone());
    }
    let normalized = selector.trim();
    if normalized.is_empty() {
        return Err(anyhow!("项目选择不能为空"));
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
            "未找到项目：{selector}（先用 `/sessions` 查看项目列表）"
        )),
        _ => Err(anyhow!("项目前缀不唯一，请使用完整路径或编号")),
    }
}

fn resolve_selector(
    selector: &str,
    sessions: &[DiskSessionMeta],
    last_view: &[String],
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
        [] => Err(anyhow!("未找到会话：{selector}")),
        _ => Err(anyhow!("会话前缀不唯一，请使用完整ID")),
    }
}

fn help_text() -> String {
    [
        "可用命令：",
        "/status",
        "/sessions [all]",
        "/sessions <项目编号> [page]",
        "/import",
        "/import <项目编号> [page]",
        "/import <编号|会话ID>",
        "/resume <编号|会话ID>",
        "/loadbg <编号|会话ID> [alias]",
        "/bg [alias]",
        "/fg <alias>",
        "/rename <old_alias> <new_alias>",
        "/save",
        "/new [workspace]",
        "/stop",
        "/interrupt",
        "/self-update",
        "/model <name|inherit|status>",
        "/fast [on|off|inherit|status]",
        "/context [1m|standard|inherit|status]",
        "/reasoning [none|minimal|low|medium|high|xhigh|inherit|status]",
        "/verbose [on|off|status]",
        "/plan [on|off|status]",
    ]
    .join("\n")
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
                state.plan_mode = true;
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
        assert!(snapshot.settings.plan_mode);
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
        assert!(reply.text.contains("工作目录"));
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
        assert!(reply.text.contains("模型: gpt-global high 1M fast"));
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
        assert!(reply.text.contains("最近用户消息: 请帮我处理发布失败"));
        assert!(reply.text.contains("模型: gpt-5.4 high 1M"));
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
        assert!(reply.contains("已自动切回最近的后台会话 `newer`"));
        assert!(reply.contains("模型: gpt-newer high 1M"));
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
        assert!(project_reply.text.contains("项目列表"));

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
        assert!(session_reply.text.contains("无摘要"));
        assert!(!session_reply.text.contains("thread-a"));
    }
}
