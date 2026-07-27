//! The inbound QQ message path: normalization (attachment download, quote
//! extraction) and turning `CommandOutcome` decisions into effects.

use std::{path::PathBuf, sync::atomic::Ordering};

use anyhow::Result;
use rust_i18n::t;
use tracing::{debug, info, warn};

use crate::{
    codex::{
        CodexRuntimeProfile, read_codex_runtime_profile_from_path,
        write_context_mode_to_config_path, write_model_to_config_path,
        write_reasoning_effort_to_config_path, write_service_tier_to_config_path,
    },
    commands::{ApprovalIntent, CommandActivity, CommandOutcome, maybe_handle_command},
    message::{IncomingAttachment, IncomingMessage, QuotedMessage},
    qq::{C2CMessageEvent, MSG_TYPE_QUOTE, MessageAttachment, MsgElement},
    session::state::{ContextMode, ServiceTier},
};

use super::{ActiveTurnContext, App};

impl App {
    pub async fn handle_c2c_event(&self, event: C2CMessageEvent) -> Result<()> {
        let normalized = self.normalize_message(event).await?;
        let profile_path = self.runtime_profile_path();
        let runtime_profile = read_codex_runtime_profile_from_path(&profile_path);
        info!(
            sender_openid = %normalized.sender_openid,
            message_id = %normalized.message_id,
            text = %normalized.text,
            images = normalized.images.len(),
            files = normalized.files.len(),
            quote = normalized.quote.is_some(),
            "received normalized c2c message"
        );
        self.flush_pending_scheduler_deliveries(&normalized.sender_openid, &normalized.message_id)
            .await;
        let trimmed_command = normalized.text.trim();
        if matches!(trimmed_command, "/stop" | "/停止")
            && crate::scheduler::pending_for_owner(
                &self.config.general.data_dir,
                &normalized.sender_openid,
            )
            .await?
            .is_some()
        {
            // The interactive job may have a codex turn mid-flight; stop it
            // before restoring the parked dialog, or it keeps streaming into
            // the restored conversation.
            self.cancel_active_turn().await;
            crate::scheduler::finish_job_for_owner(
                &self.scheduler_ctx,
                &normalized.sender_openid,
                "stopped",
            )
            .await?;
            let lang = self.command_locale(&normalized.sender_openid).await;
            self.reply_text(
                &normalized.sender_openid,
                &normalized.message_id,
                &t!(
                    "scheduler.interactive.stop_confirmed",
                    locale = lang.as_str()
                ),
            )
            .await?;
            return Ok(());
        }

        // Cancel the running turn *before* /stop mutates the session state, so
        // the abort tail races a foreground that has already moved on (the CAS
        // binding drops it) instead of streaming into the swapped dialog.
        // Alias-expanded stops still get cancelled via dispatch_outcome.
        if matches!(trimmed_command, "/stop" | "/停止") {
            self.cancel_active_turn().await;
        }

        let has_active_turn = self
            .active_openid
            .lock()
            .await
            .as_ref()
            .is_some_and(|active| active.openid == normalized.sender_openid);
        let command_outcome = match maybe_handle_command(
            &normalized.text,
            &normalized.sender_openid,
            &self.session,
            &self.config.general.default_model,
            &runtime_profile,
            CommandActivity {
                is_busy: self.busy.load(Ordering::SeqCst),
                has_active_turn,
            },
            self.display_tz,
        )
        .await
        {
            Ok(outcome) => outcome,
            // Command failures carry user-facing text (e.g. "后台会话 `x` 不
            // 存在"). Propagating them only reaches the dispatch loop's warn!,
            // so the user watches their slash command vanish; reply instead.
            Err(err) => {
                let lang = self.command_locale(&normalized.sender_openid).await;
                let text = render_command_error(&err, lang.as_str());
                self.reply_text(&normalized.sender_openid, &normalized.message_id, &text)
                    .await?;
                return Ok(());
            }
        };
        if normalized.text.trim_start().starts_with('/')
            && !matches!(command_outcome, CommandOutcome::RetryResume)
        {
            self.pending_resume_messages
                .lock()
                .await
                .remove(&normalized.sender_openid);
        }

        self.dispatch_outcome(command_outcome, normalized, runtime_profile)
            .await
    }

    /// Turn a `CommandOutcome` decision into effects: a direct reply, a turn
    /// cancellation, a global-setting write, an approval resolution, or a full
    /// codex turn (`Continue`).
    async fn dispatch_outcome(
        &self,
        outcome: CommandOutcome,
        normalized: IncomingMessage,
        runtime_profile: CodexRuntimeProfile,
    ) -> Result<()> {
        let openid = &normalized.sender_openid;
        let message_id = &normalized.message_id;
        match outcome {
            CommandOutcome::Reply(reply) => {
                info!(
                    sender_openid = %openid,
                    message_id = %message_id,
                    "handled as direct command"
                );
                self.reply_text(openid, message_id, &reply.text).await
            }
            CommandOutcome::CancelCurrent(message) | CommandOutcome::StopCurrent(message) => {
                self.cancel_active_turn().await;
                self.reply_text(openid, message_id, &message).await
            }
            CommandOutcome::SelfUpdate => self.handle_self_update_command(openid, message_id).await,
            CommandOutcome::Compact => {
                self.handle_compact_command(openid, message_id, &runtime_profile)
                    .await
            }
            CommandOutcome::RetryResume => {
                let retry_message = self.pending_resume_messages.lock().await.remove(openid);
                let Some(mut retry_message) = retry_message else {
                    let lang = self.command_locale(openid).await;
                    return self
                        .reply_text(
                            openid,
                            message_id,
                            &t!("commands.resume.no_recovery", locale = lang.as_str()),
                        )
                        .await;
                };
                retry_message.message_id = normalized.message_id.clone();
                self.run_normal_message(retry_message, runtime_profile)
                    .await
            }
            CommandOutcome::SetGlobalModel(value) => {
                self.apply_global_setting(
                    openid,
                    message_id,
                    |path| write_model_to_config_path(path, value.as_deref()),
                    |profile, locale| {
                        let effective_model = profile
                            .configured_model
                            .clone()
                            .unwrap_or_else(|| self.config.general.default_model.clone());
                        t!(
                            "commands.model.updated",
                            model = effective_model,
                            locale = locale
                        )
                        .into_owned()
                    },
                )
                .await
            }
            CommandOutcome::SetGlobalReasoning(value) => {
                self.apply_global_setting(
                    openid,
                    message_id,
                    |path| write_reasoning_effort_to_config_path(path, value),
                    |profile, locale| {
                        let effective_reasoning = profile
                            .reasoning_effort
                            .unwrap_or(self.config.general.default_reasoning_effort)
                            .as_str();
                        t!(
                            "commands.reasoning.updated",
                            value = effective_reasoning,
                            locale = locale
                        )
                        .into_owned()
                    },
                )
                .await
            }
            CommandOutcome::SetGlobalFast(value) => {
                self.apply_global_setting(
                    openid,
                    message_id,
                    |path| write_service_tier_to_config_path(path, value),
                    |profile, locale| {
                        t!(
                            "commands.fast.updated",
                            value = ServiceTier::fast_label(profile.service_tier),
                            locale = locale
                        )
                        .into_owned()
                    },
                )
                .await
            }
            CommandOutcome::SetGlobalContext(value) => {
                self.apply_global_setting(
                    openid,
                    message_id,
                    |path| write_context_mode_to_config_path(path, value),
                    |profile, locale| {
                        t!(
                            "commands.context.updated",
                            value = global_context_label(profile.context_mode),
                            locale = locale
                        )
                        .into_owned()
                    },
                )
                .await
            }
            CommandOutcome::Approval(intent) => {
                let resolved = self.resolve_pending_approval(openid, intent).await;
                let zh = self.command_locale(openid).await.starts_with("zh");
                let msg = if resolved {
                    match intent {
                        ApprovalIntent::Accept => {
                            if zh {
                                "已放行本次请求。"
                            } else {
                                "approval granted."
                            }
                        }
                        ApprovalIntent::AcceptForSession => {
                            if zh {
                                "已放行本次请求，后续类似命令将自动放行。"
                            } else {
                                "approval granted; similar commands will be auto-approved."
                            }
                        }
                        ApprovalIntent::Decline => {
                            if zh {
                                "已拒绝本次请求。"
                            } else {
                                "approval declined."
                            }
                        }
                        ApprovalIntent::Cancel => {
                            if zh {
                                "已拒绝并要求终止当前回合。"
                            } else {
                                "declined and asked codex to abort the turn."
                            }
                        }
                    }
                } else if zh {
                    "当前没有待处理的审批请求。"
                } else {
                    "no pending approval to respond to."
                };
                self.reply_text(openid, message_id, msg).await
            }
            CommandOutcome::Continue => self.run_normal_message(normalized, runtime_profile).await,
        }
    }

    /// Shared body of the four `SetGlobal*` outcomes: resolve the user's
    /// locale, apply `write` to the global codex config, re-read the runtime
    /// profile, and reply with `render`'s confirmation message.
    async fn apply_global_setting(
        &self,
        openid: &str,
        message_id: &str,
        write: impl FnOnce(&std::path::Path) -> Result<()>,
        render: impl FnOnce(&CodexRuntimeProfile, &str) -> String,
    ) -> Result<()> {
        let lang = self.command_locale(openid).await;
        let profile_path = self.runtime_profile_path();
        write(&profile_path)?;
        let updated_profile = read_codex_runtime_profile_from_path(&profile_path);
        let msg = render(&updated_profile, lang.as_str());
        self.reply_text(openid, message_id, &msg).await
    }

    async fn run_normal_message(
        &self,
        normalized: IncomingMessage,
        runtime_profile: CodexRuntimeProfile,
    ) -> Result<()> {
        let Some(_busy) = self.try_acquire_busy() else {
            warn!(
                sender_openid = %normalized.sender_openid,
                message_id = %normalized.message_id,
                "rejected because another turn is still running"
            );
            let lang = self.command_locale(&normalized.sender_openid).await;
            self.reply_text(
                &normalized.sender_openid,
                &normalized.message_id,
                &t!("errors.busy", locale = lang.as_str()),
            )
            .await?;
            return Ok(());
        };

        *self.active_openid.lock().await = Some(ActiveTurnContext {
            openid: normalized.sender_openid.clone(),
            reply_message_id: normalized.message_id.clone(),
        });
        let openid_for_cleanup = normalized.sender_openid.clone();
        let result = self.run_turn(normalized, runtime_profile).await;
        // Clear active-turn state + drop any pending approval queue the
        // turn left behind (so a later `/approve` doesn't resolve against
        // a completed turn).
        *self.active_openid.lock().await = None;
        {
            let mut guard = self.pending_approvals.lock().await;
            if let Some(queue) = guard.remove(&openid_for_cleanup)
                && !queue.is_empty()
            {
                debug!(
                    openid = %openid_for_cleanup,
                    dropped = queue.len(),
                    "dropping pending approvals after turn completion"
                );
            }
        }
        // `_busy` drops here, releasing the busy slot last — the same order
        // the hand-written `store(false)` used to run in.
        result
    }

    async fn flush_pending_scheduler_deliveries(&self, openid: &str, message_id: &str) {
        let deliveries = match crate::scheduler::take_pending_deliveries(
            &self.config.general.data_dir,
            openid,
        )
        .await
        {
            Ok(deliveries) => deliveries,
            Err(err) => {
                warn!(openid = %openid, error = %err, "failed to load pending scheduler deliveries");
                return;
            }
        };
        let lang = self.command_locale(openid).await;
        for delivery in deliveries {
            let text = t!(
                "scheduler.redelivery",
                title = delivery.title.as_str(),
                error = delivery.error.as_str(),
                text = delivery.text.as_str(),
                locale = lang.as_str()
            )
            .into_owned();
            if let Err(err) = self
                .qq_client
                .send_markdown(openid, message_id, &text, Some(message_id))
                .await
            {
                warn!(openid = %openid, job_id = %delivery.job_id, error = %err, "failed to flush pending scheduler delivery");
                let _ = crate::scheduler::queue_pending_delivery(
                    &self.config.general.data_dir,
                    openid,
                    &delivery,
                )
                .await;
                break;
            }
        }
    }

    async fn normalize_message(&self, event: C2CMessageEvent) -> Result<IncomingMessage> {
        let mut images = Vec::new();
        let mut files = Vec::new();
        for attachment in &event.attachments {
            let local = self.download_attachment(&event.id, attachment).await?;
            let normalized = IncomingAttachment {
                filename: attachment.filename.clone(),
                content_type: Some(attachment.content_type.clone()),
                source_url: attachment.url.clone(),
                local_path: local,
            };
            if attachment.content_type.starts_with("image/") {
                images.push(normalized);
            } else {
                files.push(normalized);
            }
        }
        Ok(IncomingMessage {
            sender_openid: event.author.user_openid,
            message_id: event.id.clone(),
            text: event.content.trim().to_string(),
            quote: extract_quote(event.message_type, &event.msg_elements),
            images,
            files,
            mentions: Vec::new(),
        })
    }

    async fn download_attachment(
        &self,
        message_id: &str,
        attachment: &MessageAttachment,
    ) -> Result<PathBuf> {
        let raw_filename = attachment
            .filename
            .clone()
            .unwrap_or_else(|| infer_filename(attachment));
        // The filename is untrusted QQ attachment metadata. Reduce it to its
        // final path component so embedded separators (e.g. "../../etc/foo")
        // cannot escape the inbox directory and write to an arbitrary path.
        let filename = sanitize_attachment_filename(&raw_filename)
            .unwrap_or_else(|| infer_filename(attachment));
        let destination = self
            .session
            .inbox_dir()
            .join(format!("{message_id}_{filename}"));
        self.qq_client
            .download_attachment(&attachment.url, &destination)
            .await?;
        Ok(destination)
    }
}

fn global_context_label(context_mode: Option<ContextMode>) -> &'static str {
    match context_mode {
        Some(mode) => mode.label(),
        None => "inherit",
    }
}

/// Reduce an untrusted attachment filename to a safe single path component.
/// Returns None when nothing usable remains (empty, ".", "..", or only
/// separators), so the caller can fall back to an inferred name.
fn sanitize_attachment_filename(raw: &str) -> Option<String> {
    // Treat both '/' and '\\' as separators regardless of host platform, since
    // the value originates from a remote peer.
    let last = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    if last.is_empty() || last == "." || last == ".." {
        return None;
    }
    Some(last.to_string())
}

fn infer_filename(attachment: &MessageAttachment) -> String {
    let extension = match attachment.content_type.as_str() {
        content if content.starts_with("image/png") => "png",
        content if content.starts_with("image/jpeg") => "jpg",
        content if content.starts_with("image/webp") => "webp",
        _ => "bin",
    };
    format!("attachment.{extension}")
}

fn extract_quote(message_type: Option<u32>, msg_elements: &[MsgElement]) -> Option<QuotedMessage> {
    if message_type != Some(MSG_TYPE_QUOTE) || msg_elements.is_empty() {
        return None;
    }
    let first = &msg_elements[0];
    let mut lines = Vec::new();
    if let Some(content) = &first.content {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
    }
    for attachment in &first.attachments {
        lines.push(format!(
            "[附件: {}]",
            attachment
                .filename
                .as_deref()
                .unwrap_or(&attachment.content_type)
        ));
    }
    for nested in &first.msg_elements {
        if let Some(content) = &nested.content {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
            }
        }
    }
    Some(QuotedMessage {
        message_id: first.msg_idx.clone(),
        text: if lines.is_empty() {
            "用户引用了一条消息".to_string()
        } else {
            lines.join("\n")
        },
    })
}

/// Localized rendering for command failures. Structured dialog errors get
/// proper translations (with recovery hints such as the available aliases);
/// anything else falls back to the error's own user-facing text.
fn render_command_error(err: &anyhow::Error, locale: &str) -> String {
    use crate::session::DialogError;
    match err.downcast_ref::<DialogError>() {
        Some(DialogError::BackgroundNotFound { alias, available }) => {
            if available.is_empty() {
                t!(
                    "errors.session.bg_not_found_empty",
                    alias = alias,
                    locale = locale
                )
                .into_owned()
            } else {
                let sep = if locale.starts_with("zh") {
                    "、"
                } else {
                    ", "
                };
                let list = available
                    .iter()
                    .map(|value| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(sep);
                t!(
                    "errors.session.bg_not_found",
                    alias = alias,
                    aliases = list,
                    locale = locale
                )
                .into_owned()
            }
        }
        Some(DialogError::AliasExists { alias }) => t!(
            "errors.session.alias_exists",
            alias = alias,
            locale = locale
        )
        .into_owned(),
        Some(DialogError::AliasHeldByForeground { alias }) => t!(
            "errors.session.alias_held_by_foreground",
            alias = alias,
            locale = locale
        )
        .into_owned(),
        Some(DialogError::AliasInvalid) => {
            t!("errors.session.alias_invalid", locale = locale).into_owned()
        }
        Some(DialogError::AliasAllocFailed) => {
            t!("errors.session.alias_alloc_failed", locale = locale).into_owned()
        }
        None => format!("{err:#}"),
    }
}

#[cfg(test)]
mod tests {
    use crate::qq::{MSG_TYPE_QUOTE, MessageAttachment, MsgElement};

    use super::{extract_quote, sanitize_attachment_filename};

    #[test]
    fn sanitize_attachment_filename_strips_traversal() {
        assert_eq!(
            sanitize_attachment_filename("foo/../../bar").as_deref(),
            Some("bar")
        );
        assert_eq!(
            sanitize_attachment_filename("a\\b\\c.png").as_deref(),
            Some("c.png")
        );
        assert_eq!(
            sanitize_attachment_filename("plain.jpg").as_deref(),
            Some("plain.jpg")
        );
        assert_eq!(sanitize_attachment_filename("../"), None);
        assert_eq!(sanitize_attachment_filename(".."), None);
        assert_eq!(sanitize_attachment_filename(""), None);
    }

    #[test]
    fn extracts_quote_from_msg_elements() {
        let quote = extract_quote(
            Some(MSG_TYPE_QUOTE),
            &[MsgElement {
                msg_idx: Some("ref-1".into()),
                content: Some("hello".into()),
                attachments: vec![MessageAttachment {
                    content_type: "text/plain".into(),
                    url: "https://example.com/a".into(),
                    filename: Some("a.txt".into()),
                }],
                msg_elements: Vec::new(),
            }],
        )
        .unwrap();
        assert_eq!(quote.message_id.as_deref(), Some("ref-1"));
        assert!(quote.text.contains("hello"));
        assert!(quote.text.contains("a.txt"));
    }
}
