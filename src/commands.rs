use anyhow::Result;

use crate::{
    codex::runtime::CodexRuntimeProfile,
    session::{
        state::{ContextMode, ReasoningEffort, ServiceTier, SessionState},
        store::SessionStore,
    },
};

#[derive(Debug, Clone)]
pub struct CommandReply {
    pub text: String,
}

pub enum CommandOutcome {
    Reply(CommandReply),
    Continue,
    StopCurrent(String),
}

pub async fn maybe_handle_command(
    text: &str,
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
        "/help" => Ok(CommandOutcome::Reply(CommandReply {
            text: help_text(),
        })),
        "/bind" => Ok(CommandOutcome::Reply(CommandReply {
            text: "绑定/授权限制已禁用，所有私聊用户都可直接使用机器人。".to_string(),
        })),
        "/model" => handle_model(&rest, session, default_model, runtime_profile, is_busy).await,
        "/fast" => handle_fast(&rest, session, default_model, runtime_profile, is_busy).await,
        "/context" => handle_context(&rest, session, default_model, runtime_profile, is_busy).await,
        "/reasoning" => {
            handle_reasoning(&rest, session, default_model, runtime_profile, is_busy).await
        }
        "/verbose" => handle_verbose(&rest, session, default_model, runtime_profile, is_busy).await,
        "/plan" => handle_plan(&rest, session, default_model, runtime_profile, is_busy).await,
        "/status" => Ok(CommandOutcome::Reply(CommandReply {
            text: build_status_text(
                &session.snapshot().await,
                default_model,
                runtime_profile,
                is_busy,
            ),
        })),
        "/session" => handle_session(&rest, session, default_model, runtime_profile, is_busy).await,
        "/new" => handle_new(session, default_model, runtime_profile, is_busy).await,
        "/stop" | "/interrupt" => Ok(CommandOutcome::StopCurrent(
            "已请求停止当前运行。".to_string(),
        )),
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
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> Result<CommandOutcome> {
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(selector_reply(
            session,
            default_model,
            runtime_profile,
            is_busy,
            "模型与运行时设置".to_string(),
        )
        .await));
    }
    let value = args.join(" ");
    let next = if matches!(value.as_str(), "default" | "inherit") {
        None
    } else {
        Some(value.clone())
    };
    session
        .update_settings(|state| state.settings.model_override = next.clone())
        .await?;
    Ok(CommandOutcome::Reply(selector_reply(
        session,
        default_model,
        runtime_profile,
        is_busy,
        format!("模型已更新为：{}", next.as_deref().unwrap_or(default_model)),
    )
    .await))
}

async fn handle_fast(
    args: &[&str],
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> Result<CommandOutcome> {
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(selector_reply(
            session,
            default_model,
            runtime_profile,
            is_busy,
            "Fast mode 设置".to_string(),
        )
        .await));
    }
    let next = if args[0].eq_ignore_ascii_case("inherit") {
        None
    } else {
        Some(ServiceTier::parse(args[0]).ok_or_else(|| {
            anyhow::anyhow!("用法：`/fast [on|off|inherit|status]`")
        })?)
    };
    session
        .update_settings(|state| state.settings.service_tier = next)
        .await?;
    Ok(CommandOutcome::Reply(selector_reply(
        session,
        default_model,
        runtime_profile,
        is_busy,
        "Fast mode 已更新。".to_string(),
    )
    .await))
}

async fn handle_context(
    args: &[&str],
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> Result<CommandOutcome> {
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(selector_reply(
            session,
            default_model,
            runtime_profile,
            is_busy,
            "上下文模式设置".to_string(),
        )
        .await));
    }
    let next = if args[0].eq_ignore_ascii_case("inherit") {
        None
    } else {
        Some(ContextMode::parse(args[0]).ok_or_else(|| {
            anyhow::anyhow!("用法：`/context [1m|standard|inherit|status]`")
        })?)
    };
    session
        .update_settings(|state| state.settings.context_mode = next)
        .await?;
    Ok(CommandOutcome::Reply(selector_reply(
        session,
        default_model,
        runtime_profile,
        is_busy,
        "上下文模式已更新。".to_string(),
    )
    .await))
}

async fn handle_reasoning(
    args: &[&str],
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> Result<CommandOutcome> {
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(selector_reply(
            session,
            default_model,
            runtime_profile,
            is_busy,
            "思考深度设置".to_string(),
        )
        .await));
    }
    let next = if args[0].eq_ignore_ascii_case("inherit") {
        None
    } else {
        Some(ReasoningEffort::parse(args[0]).ok_or_else(|| {
            anyhow::anyhow!("无效思考深度：可选 none|minimal|low|medium|high|xhigh|inherit")
        })?)
    };
    session
        .update_settings(|state| state.settings.reasoning_effort = next)
        .await?;
    Ok(CommandOutcome::Reply(selector_reply(
        session,
        default_model,
        runtime_profile,
        is_busy,
        "思考深度已更新。".to_string(),
    )
    .await))
}

async fn handle_plan(
    args: &[&str],
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> Result<CommandOutcome> {
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: build_status_text(
                &session.snapshot().await,
                default_model,
                runtime_profile,
                is_busy,
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
        .update_settings(|state| state.settings.plan_mode = enabled)
        .await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: build_status_text(
            &session.snapshot().await,
            default_model,
            runtime_profile,
            is_busy,
        ),
    }))
}

async fn handle_verbose(
    args: &[&str],
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> Result<CommandOutcome> {
    if args.is_empty() || args[0].eq_ignore_ascii_case("status") {
        return Ok(CommandOutcome::Reply(CommandReply {
            text: build_status_text(
                &session.snapshot().await,
                default_model,
                runtime_profile,
                is_busy,
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
        .update_settings(|state| state.settings.verbose = enabled)
        .await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: build_status_text(
            &session.snapshot().await,
            default_model,
            runtime_profile,
            is_busy,
        ),
    }))
}

async fn handle_session(
    args: &[&str],
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> Result<CommandOutcome> {
    if args.first() == Some(&"new") {
        return handle_new(session, default_model, runtime_profile, is_busy).await;
    }
    Ok(CommandOutcome::Reply(CommandReply {
        text: build_status_text(
            &session.snapshot().await,
            default_model,
            runtime_profile,
            is_busy,
        ),
    }))
}

async fn handle_new(
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> Result<CommandOutcome> {
    session.reset_for_new_session().await?;
    Ok(CommandOutcome::Reply(CommandReply {
        text: format!(
            "已重置当前会话上下文。\n\n{}",
            build_status_text(
                &session.snapshot().await,
                default_model,
                runtime_profile,
                is_busy,
            )
        ),
    }))
}

async fn selector_reply(
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
    header: String,
) -> CommandReply {
    let snapshot = session.snapshot().await;
    CommandReply {
        text: format!(
            "{}\n\n{}\n\n{}",
            header,
            build_status_text(&snapshot, default_model, runtime_profile, is_busy),
            model_help_text()
        ),
    }
}

fn build_status_text(
    state: &SessionState,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> String {
    let effective_model = state
        .settings
        .model_override
        .clone()
        .or_else(|| runtime_profile.configured_model.clone())
        .unwrap_or_else(|| default_model.to_string());
    let effective_reasoning = state
        .settings
        .reasoning_effort
        .or(runtime_profile.reasoning_effort)
        .unwrap_or(ReasoningEffort::Medium)
        .as_str();
    let fast_label = match state.settings.service_tier.or(runtime_profile.service_tier) {
        Some(ServiceTier::Fast) => "on",
        Some(ServiceTier::Flex) => "off",
        None => "inherit",
    };
    let context_label = match state.settings.context_mode.or(runtime_profile.context_mode) {
        Some(mode) => mode.label(),
        None => "inherit",
    };
    let provider = runtime_profile
        .model_provider
        .as_deref()
        .unwrap_or("default");
    format!(
        "session_id: {}\nmodel: {}\nfast: {}\ncontext: {}\nreasoning: {}\nverbose: {}\nplan: {}\nprovider: {}\nstate: {}",
        state.session_id.as_deref().unwrap_or("none"),
        effective_model,
        fast_label,
        context_label,
        effective_reasoning,
        if state.settings.verbose { "on" } else { "off" },
        if state.settings.plan_mode { "on" } else { "off" },
        provider,
        if is_busy { "running" } else { "idle" }
    )
}

fn model_help_text() -> String {
    [
        "可用设置：",
        "/model gpt-5.4",
        "/model gpt-5-codex",
        "/model gpt-5.1-codex-mini",
        "/fast on|off|inherit",
        "/context 1m|standard|inherit",
        "/reasoning low|medium|high|xhigh|inherit",
        "/verbose on|off",
        "/new",
    ]
    .join("\n")
}

fn help_text() -> String {
    [
        "可用命令：",
        "/model",
        "/model <name|inherit>",
        "/fast [on|off|inherit|status]",
        "/context [1m|standard|inherit|status]",
        "/reasoning [none|minimal|low|medium|high|xhigh|inherit|status]",
        "/verbose [on|off|status]",
        "/plan [on|off|status]",
        "/status",
        "/new",
        "/stop",
        "/interrupt",
        "/session",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::{
        codex::runtime::CodexRuntimeProfile,
        session::{state::ContextMode, store::SessionStore},
    };

    use super::{CommandOutcome, maybe_handle_command};

    #[tokio::test]
    async fn session_new_resets_only_session_id() {
        let dir = tempdir().unwrap();
        let session = SessionStore::load_or_init(dir.path()).await.unwrap();
        session
            .update_settings(|state| {
                state.session_id = Some("thread".into());
                state.settings.model_override = Some("gpt-x".into());
                state.settings.plan_mode = true;
            })
            .await
            .unwrap();
        let outcome = maybe_handle_command(
            "/new",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
        let snapshot = session.snapshot().await;
        assert!(snapshot.session_id.is_none());
        assert_eq!(snapshot.settings.model_override.as_deref(), Some("gpt-x"));
        assert!(snapshot.settings.plan_mode);
    }

    #[tokio::test]
    async fn context_command_updates_override() {
        let dir = tempdir().unwrap();
        let session = SessionStore::load_or_init(dir.path()).await.unwrap();
        let outcome = maybe_handle_command(
            "/context 1m",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
        assert_eq!(session.snapshot().await.settings.context_mode, Some(ContextMode::OneM));
    }

    #[tokio::test]
    async fn verbose_command_updates_setting() {
        let dir = tempdir().unwrap();
        let session = SessionStore::load_or_init(dir.path()).await.unwrap();
        let outcome = maybe_handle_command(
            "/verbose on",
            &session,
            "default",
            &CodexRuntimeProfile::default(),
            false,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
        assert!(session.snapshot().await.settings.verbose);
    }
}
