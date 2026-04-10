use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Result;
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{error, info, warn};

use crate::{
    codex::{
        executor::{CodexExecutor, ExecutionRequest},
        output::{Directive, parse_output},
        prompt::build_prompt,
        runtime::read_codex_runtime_profile,
    },
    commands::{CommandOutcome, CommandReply, maybe_handle_command},
    config::AppConfig,
    message::{IncomingAttachment, IncomingMessage, QuotedMessage},
    qq::{
        api::QqApiClient,
        passive::PassiveTurnEmitter,
        types::{C2CMessageEvent, MSG_TYPE_QUOTE, MessageAttachment, MsgElement},
    },
    session::store::SessionStore,
};

pub struct App {
    pub config: AppConfig,
    pub session: Arc<SessionStore>,
    pub qq_client: Arc<QqApiClient>,
    pub codex: Arc<CodexExecutor>,
    busy: AtomicBool,
    active_turn: Mutex<Option<oneshot::Sender<()>>>,
}

impl App {
    pub fn new(
        config: AppConfig,
        session: Arc<SessionStore>,
        qq_client: Arc<QqApiClient>,
        codex: Arc<CodexExecutor>,
    ) -> Self {
        Self {
            config,
            session,
            qq_client,
            codex,
            busy: AtomicBool::new(false),
            active_turn: Mutex::new(None),
        }
    }

    pub async fn handle_c2c_event(&self, event: C2CMessageEvent) -> Result<()> {
        let normalized = self.normalize_message(event).await?;
        let runtime_profile = read_codex_runtime_profile();
        info!(
            sender_openid = %normalized.sender_openid,
            message_id = %normalized.message_id,
            text = %normalized.text,
            images = normalized.images.len(),
            files = normalized.files.len(),
            quote = normalized.quote.is_some(),
            "received normalized c2c message"
        );

        match maybe_handle_command(
            &normalized.text,
            &self.session,
            &self.config.general.default_model,
            &runtime_profile,
            self.busy.load(Ordering::SeqCst),
        )
        .await? {
            CommandOutcome::Reply(reply) => {
                info!(
                    sender_openid = %normalized.sender_openid,
                    message_id = %normalized.message_id,
                    "handled as direct command"
                );
                self.send_command_reply(
                    &normalized.sender_openid,
                    &normalized.message_id,
                    &reply,
                )
                .await?;
                return Ok(());
            }
            CommandOutcome::StopCurrent(message) => {
                self.cancel_active_turn().await;
                self.qq_client
                    .send_text(
                        &normalized.sender_openid,
                        &normalized.message_id,
                        &message,
                        Some(&normalized.message_id),
                    )
                    .await?;
                return Ok(());
            }
            CommandOutcome::Continue => {}
        }

        if self.busy.swap(true, Ordering::SeqCst) {
            warn!(
                sender_openid = %normalized.sender_openid,
                message_id = %normalized.message_id,
                "rejected because another turn is still running"
            );
            self.qq_client
                .send_text(
                    &normalized.sender_openid,
                    &normalized.message_id,
                    "上一轮仍在处理中，请稍后再试。",
                    Some(&normalized.message_id),
                )
                .await?;
            return Ok(());
        }

        let result = self.run_turn(normalized).await;
        self.busy.store(false, Ordering::SeqCst);
        result
    }

    async fn run_turn(&self, message: IncomingMessage) -> Result<()> {
        let snapshot = self.session.snapshot().await;
        let configured_model = snapshot.settings.model_override.clone();
        let model_label = configured_model
            .clone()
            .unwrap_or_else(|| self.config.general.default_model.clone());
        let reasoning = snapshot
            .settings
            .reasoning_effort
            .unwrap_or(self.config.general.default_reasoning_effort);
        let prompt = build_prompt(
            &message,
            &snapshot.settings,
            &self.config.general.default_model,
        );
        info!(
            sender_openid = %message.sender_openid,
            message_id = %message.message_id,
            model = %model_label,
            reasoning = %reasoning.as_str(),
            plan_mode = snapshot.settings.plan_mode,
            "starting codex turn"
        );

        let (update_tx, update_rx) = mpsc::unbounded_channel();
        let emitter = tokio::spawn(
            PassiveTurnEmitter::new(
                self.qq_client.clone(),
                message.sender_openid.clone(),
                message.message_id.clone(),
                self.session.workspace_dir().to_path_buf(),
                snapshot.settings.verbose,
            )
            .run(update_rx),
        );

        let execution = self
            .codex
            .execute(ExecutionRequest {
                prompt,
                workspace_dir: self.session.workspace_dir().to_path_buf(),
                session_state: snapshot.clone(),
                model: configured_model,
                service_tier: snapshot.settings.service_tier,
                context_mode: snapshot.settings.context_mode,
                reasoning_effort: reasoning,
                image_paths: message
                    .images
                    .iter()
                    .map(|image| image.local_path.clone())
                    .collect(),
            }, Some(self.install_active_turn().await), Some(update_tx))
            .await;
        self.clear_active_turn().await;
        let dispatch_report = emitter.await??;

        match execution {
            Ok(output) => {
                info!(
                    sender_openid = %message.sender_openid,
                    message_id = %message.message_id,
                    session_id = output.session_id.as_deref().unwrap_or(""),
                    text_len = output.text.len(),
                    "codex turn completed"
                );
                self.session.set_session_id(output.session_id).await?;
                if !dispatch_report.saw_agent_message {
                    let parsed = parse_output(&output.text, self.session.workspace_dir());
                    if !parsed.text.is_empty() {
                        self.qq_client
                            .send_text(
                                &message.sender_openid,
                                &message.message_id,
                                &parsed.text,
                                Some(&message.message_id),
                            )
                            .await?;
                    }
                    for directive in parsed.directives {
                        self.send_directive(&message.sender_openid, &message.message_id, directive)
                            .await?;
                    }
                }
                Ok(())
            }
            Err(err) => {
                if err.to_string().contains("aborted by user") {
                    info!("codex turn aborted by operator");
                    return Ok(());
                }
                error!("codex execution failed: {err:#}");
                let text = format!("Codex 执行失败：{err}");
                if dispatch_report.sent_replies == 0 {
                    self.qq_client
                        .send_text(
                            &message.sender_openid,
                            &message.message_id,
                            &text,
                            Some(&message.message_id),
                        )
                        .await?;
                }
                Err(err)
            }
        }
    }

    async fn send_command_reply(
        &self,
        openid: &str,
        message_id: &str,
        reply: &CommandReply,
    ) -> Result<()> {
        self.qq_client
            .send_text(openid, message_id, &reply.text, Some(message_id))
            .await
    }

    async fn install_active_turn(&self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        *self.active_turn.lock().await = Some(tx);
        rx
    }

    async fn clear_active_turn(&self) {
        self.active_turn.lock().await.take();
    }

    async fn cancel_active_turn(&self) {
        if let Some(cancel) = self.active_turn.lock().await.take() {
            let _ = cancel.send(());
        }
    }

    async fn send_directive(
        &self,
        openid: &str,
        message_id: &str,
        directive: Directive,
    ) -> Result<()> {
        match directive {
            Directive::Image { path } => {
                info!(path = %path.display(), "sending image directive to qq");
                let info = self.qq_client.upload_file(openid, &path, 1).await?;
                self.qq_client.send_media(openid, message_id, &info).await?;
            }
            Directive::File { path, .. } => {
                info!(path = %path.display(), "sending file directive to qq");
                let info = self.qq_client.upload_file(openid, &path, 4).await?;
                self.qq_client.send_media(openid, message_id, &info).await?;
            }
        }
        Ok(())
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
        let filename = attachment
            .filename
            .clone()
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

#[cfg(test)]
mod tests {
    use crate::qq::types::{EventAuthor, MSG_TYPE_QUOTE, MessageAttachment, MsgElement};

    use super::extract_quote;

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
        let _ = EventAuthor {
            user_openid: "u".into(),
        };
    }
}
