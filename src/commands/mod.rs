use std::{collections::BTreeMap, future::Future, path::PathBuf, pin::Pin};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use rust_i18n::t;

use crate::{
    codex::{CodexModelEntry, CodexRuntimeProfile, list_codex_model_entries},
    session::{
        DiskSessionMeta, SessionListScope, SessionStore,
        state::{
            ApprovalPolicySetting, CommandAlias, ContextMode, PendingSetting, ReasoningEffort,
            ServiceTier, UserSessionState,
        },
    },
    util::lang::{is_supported_lang, normalize_lang},
};

mod alias;
mod cron_cmds;
mod interactive;
mod listing;
mod session_cmds;
mod settings_cmds;
#[cfg(test)]
mod tests;

use alias::*;
use cron_cmds::*;
use listing::*;
use session_cmds::*;
use settings_cmds::*;

#[derive(Debug, Clone)]
pub(crate) struct CommandReply {
    pub(crate) text: String,
}

pub(crate) enum CommandOutcome {
    Reply(CommandReply),
    Continue,
    CancelCurrent(String),
    StopCurrent(String),
    Compact,
    SelfUpdate,
    SetGlobalModel(Option<String>),
    SetGlobalReasoning(Option<ReasoningEffort>),
    SetGlobalFast(Option<ServiceTier>),
    SetGlobalContext(Option<ContextMode>),
    RetryResume,
    /// Resolve the user's next pending approval request with the given
    /// decision. App looks up its `pending_approvals[openid]` queue and
    /// sends the result back to the app-server via the broker.
    Approval(ApprovalIntent),
}

impl CommandOutcome {
    /// Wraps any string-ish value as a reply outcome, collapsing the
    /// `CommandOutcome::Reply(CommandReply { text })` boilerplate.
    fn reply(text: impl Into<String>) -> Self {
        CommandOutcome::Reply(CommandReply { text: text.into() })
    }

    /// Localized reply for translation keys without interpolation arguments.
    fn reply_t(key: &str, locale: &str) -> Self {
        Self::reply(t!(key, locale = locale))
    }

    /// Applies `f` to the text payload of the text-carrying variants
    /// (`Reply` / `CancelCurrent` / `StopCurrent`); every other variant is
    /// passed through unchanged.
    fn map_text(self, f: impl FnOnce(String) -> String) -> Self {
        match self {
            CommandOutcome::Reply(reply) => Self::reply(f(reply.text)),
            CommandOutcome::CancelCurrent(msg) => CommandOutcome::CancelCurrent(f(msg)),
            CommandOutcome::StopCurrent(msg) => CommandOutcome::StopCurrent(f(msg)),
            other => other,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ApprovalIntent {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

/// Shared per-dispatch context threaded through every command handler.
///
/// Bundles the values the dispatcher previously passed positionally, so all
/// handler signatures stay uniform regardless of which values they use.
#[derive(Clone, Copy)]
struct CmdCtx<'a> {
    openid: &'a str,
    session: &'a SessionStore,
    default_model: &'a str,
    runtime_profile: &'a CodexRuntimeProfile,
    is_busy: bool,
    /// Timezone every user-facing timestamp is rendered in.
    display_tz: chrono_tz::Tz,
}

/// Cheap locale lookup for handlers that only need the language string.
///
/// Every handler runs after the dispatcher's initial `snapshot_for_user`, so
/// the user record already exists and skipping the deep snapshot clone (and
/// its ensure-user side effect) is safe here.
async fn user_locale(session: &SessionStore, openid: &str) -> String {
    session
        .language_for_user(openid)
        .await
        .unwrap_or_else(crate::session::state::default_language)
}

pub(crate) async fn maybe_handle_command(
    text: &str,
    openid: &str,
    session: &SessionStore,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
    display_tz: chrono_tz::Tz,
) -> Result<CommandOutcome> {
    let ctx = CmdCtx {
        openid,
        session,
        default_model,
        runtime_profile,
        is_busy,
        display_tz,
    };
    maybe_handle_command_inner(text, ctx, 0).await
}

fn maybe_handle_command_inner<'a>(
    text: &'a str,
    ctx: CmdCtx<'a>,
    alias_depth: usize,
) -> Pin<Box<dyn Future<Output = Result<CommandOutcome>> + Send + 'a>> {
    Box::pin(async move {
        let CmdCtx {
            openid,
            session,
            default_model,
            runtime_profile,
            is_busy,
            ..
        } = ctx;
        let snapshot = session.snapshot_for_user(openid).await?;
        let lang_string = snapshot.settings.language.clone();
        let locale = lang_string.as_str();
        let pending_before = snapshot.pending_setting.clone();
        let is_slash_input = text.trim_start().starts_with('/');

        // Plain text while in an interactive setting is consumed by the
        // pending handler — never forwarded to Codex.
        if !is_slash_input {
            if let Some(pending) = pending_before {
                return interactive::consume_pending_input(pending, text, ctx).await;
            }
            return Ok(CommandOutcome::Continue);
        }

        let trimmed = text.trim();
        let mut parts = trimmed.split_whitespace();
        let raw_command = parts.next().unwrap_or_default().to_ascii_lowercase();
        let command = canonicalize_core_command(&raw_command).to_string();
        let rest = parts.collect::<Vec<_>>();

        if matches!(pending_before, Some(PendingSetting::ResumeRecovery)) {
            match command.as_str() {
                "/retry" => {
                    session.set_pending_setting(openid, None).await?;
                    return Ok(CommandOutcome::RetryResume);
                }
                "/cancel" => {
                    session.set_pending_setting(openid, None).await?;
                    return Ok(CommandOutcome::reply_t(
                        "commands.resume.recovery_cancelled",
                        locale,
                    ));
                }
                _ => {}
            }
        }

        // /back is a global escape: exits the current interactive setting, or
        // politely reports that nothing interactive was in progress.
        if command.as_str() == "/back" {
            if let Some(pending) = pending_before {
                session.set_pending_setting(openid, None).await?;
                return Ok(CommandOutcome::reply(t!(
                    "commands.back.exited",
                    cmd = pending.command_name(locale),
                    locale = locale
                )));
            }
            return Ok(CommandOutcome::reply_t("commands.back.idle", locale));
        }

        // Non-/back slash command while in an interactive setting: quietly
        // exit the pending state and prepend a notice to the eventual reply.
        let had_pending_picker = pending_before.is_some();
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

        // Guard against picker/approval crossfire: when a picker was active and
        // the user issues an approval command (the documented picker-escape is
        // /back, and pickers are not busy-gated so a turn can be paused awaiting
        // approval), exit the picker but do NOT dispatch the approval — that
        // would silently resolve a queued Codex approval and abort the turn.
        // Require the approval command to be re-issued.
        if had_pending_picker
            && matches!(
                command.as_str(),
                "/approve" | "/approve-session" | "/deny" | "/cancel"
            )
        {
            return Ok(CommandOutcome::reply(
                pending_exit_prefix.unwrap_or_default(),
            ));
        }

        let outcome_result: Result<CommandOutcome> = match command.as_str() {
            "/help" => {
                let full = rest
                    .first()
                    .is_some_and(|arg| matches!(*arg, "all" | "full" | "全部" | "完整"));
                Ok(CommandOutcome::reply(help_text(&lang_string, full)))
            }
            "/lang" => handle_lang(&rest, ctx).await,
            "/model" => handle_model(&rest, ctx).await,
            "/fast" => handle_fast(&rest, ctx).await,
            "/context" => handle_context(&rest, ctx).await,
            "/reasoning" => handle_reasoning(&rest, ctx).await,
            "/verbose" => handle_verbose(&rest, ctx).await,
            "/approvals" => handle_approvals(&rest, ctx).await,
            "/plan" => handle_plan(&rest, ctx).await,
            "/cron" => handle_cron(&rest, ctx).await,
            "/execute-plan" => handle_execute_plan(ctx).await,
            "/keep-planning" => handle_keep_planning(ctx).await,
            "/cancel-plan" => handle_cancel_plan(ctx).await,
            "/approve" => Ok(CommandOutcome::Approval(ApprovalIntent::Accept)),
            "/approve-session" => Ok(CommandOutcome::Approval(ApprovalIntent::AcceptForSession)),
            "/deny" => Ok(CommandOutcome::Approval(ApprovalIntent::Decline)),
            "/cancel" => Ok(CommandOutcome::Approval(ApprovalIntent::Cancel)),
            "/retry" => Ok(CommandOutcome::reply_t(
                "commands.resume.no_recovery",
                locale,
            )),
            "/status" => {
                let snapshot = session.snapshot_for_user(openid).await?;
                // Pre-fetch disk metadata so background rows can show when a
                // parked dialog was last touched and what it was about.
                let disk_meta = if snapshot.background.is_empty() {
                    std::collections::HashMap::new()
                } else {
                    session
                        .list_disk_sessions(crate::session::SessionListScope::All)
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|meta| (meta.id.clone(), (meta.updated_at, meta.title)))
                        .collect()
                };
                Ok(CommandOutcome::reply(build_status_text(
                    &snapshot,
                    default_model,
                    runtime_profile,
                    is_busy,
                    ctx.display_tz,
                    session.attachment_workspace_dir(),
                    &disk_meta,
                )))
            }
            "/sessions" => handle_sessions(&rest, ctx).await,
            "/import" => handle_import(&rest, ctx).await,
            "/new" => {
                // Strip the user's *actual* first token, not the canonical command:
                // `command` is the lowercased/aliased form ("/new"), but `trimmed`
                // still starts with what the user typed (e.g. "/新建" or "/New"), so
                // strip_prefix(command) would fail and silently drop the <dir> arg.
                let first_token = trimmed.split_whitespace().next().unwrap_or_default();
                let raw_args = trimmed[first_token.len()..].trim();
                handle_new(raw_args, ctx).await
            }
            "/bg" => handle_bg(&rest, ctx).await,
            "/fg" => handle_fg(&rest, ctx).await,
            "/resume" => handle_restore(RestoreMode::Resume, &rest, ctx).await,
            "/loadbg" => handle_restore(RestoreMode::Loadbg, &rest, ctx).await,
            "/save" => handle_save(ctx).await,
            "/rename" => handle_rename(&rest, ctx).await,
            "/stop" => handle_stop(ctx).await,
            "/interrupt" => Ok(CommandOutcome::CancelCurrent(
                t!("errors.interrupt_requested", locale = locale).into_owned(),
            )),
            "/compact" => Ok(CommandOutcome::Compact),
            "/self-update" => Ok(CommandOutcome::SelfUpdate),
            "/alias" => handle_alias(&rest, ctx).await,
            other => {
                let alias_name = other.trim_start_matches('/').to_ascii_lowercase();
                if !alias_name.is_empty()
                    && let Some(alias) = session.get_command_alias(openid, &alias_name).await?
                {
                    expand_alias(&alias, ctx, alias_depth).await
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

fn busy_reply(locale: &str) -> CommandOutcome {
    CommandOutcome::reply_t("errors.busy", locale)
}

fn prepend_pending_exit(prefix: String, outcome: CommandOutcome) -> CommandOutcome {
    outcome.map_text(|text| format!("{prefix}\n\n{text}"))
}
