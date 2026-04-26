use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, error, info, warn};

use rust_i18n::t;

use crate::{
    codex::{
        app_server::{
            ApprovalOutcome, ApprovalRequest, CommandApprovalEvent, FileChangeApprovalEvent,
            PermissionsApprovalEvent,
        },
        compact,
        executor::{CodexExecutor, CompactRequest, ExecutionRequest},
        output::{Directive, parse_output},
        prompt::build_prompt,
        runtime::{
            read_codex_runtime_profile_from_path, write_context_mode_to_config_path,
            write_model_to_config_path, write_reasoning_effort_to_config_path,
            write_service_tier_to_config_path,
        },
    },
    commands::{ApprovalIntent, CommandOutcome, CommandReply, maybe_handle_command},
    config::AppConfig,
    memory::{inject as memory_inject, store::MemoryStore},
    message::{IncomingAttachment, IncomingMessage, QuotedMessage},
    normalize_lang,
    qq::{
        api::QqApiClient,
        passive::PassiveTurnEmitter,
        types::{C2CMessageEvent, MSG_TYPE_QUOTE, MessageAttachment, MsgElement},
    },
    self_update,
    session::{
        state::{
            ContextMode, DialogProfile, ServiceTier, SessionState, TokenUsageSnapshot,
            UserSessionState,
        },
        store::SessionStore,
    },
    shadow::{ShadowContext, ShadowWorker},
};

const CONTEXT_WARNING_THRESHOLD: f64 = 0.80;

pub struct App {
    pub config: AppConfig,
    pub session: Arc<SessionStore>,
    pub qq_client: Arc<QqApiClient>,
    pub codex: Arc<CodexExecutor>,
    pub memory: Arc<MemoryStore>,
    pub shadow: Option<Arc<ShadowWorker>>,
    busy: AtomicBool,
    active_turn: Mutex<Option<oneshot::Sender<()>>>,
    /// The QQ openid whose turn currently holds `busy`. Used to route
    /// server-initiated approval requests to the right user.
    active_openid: Mutex<Option<ActiveTurnContext>>,
    /// Queued approval decisions awaiting user reply. FIFO per openid.
    pending_approvals: Mutex<HashMap<String, VecDeque<PendingApprovalEntry>>>,
}

#[derive(Clone)]
struct ActiveTurnContext {
    openid: String,
    reply_message_id: String,
}

enum PendingApprovalEntry {
    Outcome(oneshot::Sender<ApprovalOutcome>),
}

impl App {
    pub fn new(
        config: AppConfig,
        session: Arc<SessionStore>,
        qq_client: Arc<QqApiClient>,
        codex: Arc<CodexExecutor>,
        memory: Arc<MemoryStore>,
        shadow: Option<Arc<ShadowWorker>>,
    ) -> Arc<Self> {
        let app = Arc::new(Self {
            config,
            session,
            qq_client,
            codex,
            memory,
            shadow,
            busy: AtomicBool::new(false),
            active_turn: Mutex::new(None),
            active_openid: Mutex::new(None),
            pending_approvals: Mutex::new(HashMap::new()),
        });
        app.clone().install_approval_handler();
        app
    }

    /// Wire the approval broker to forward requests into our QQ prompt +
    /// pending-approval queue.
    fn install_approval_handler(self: Arc<Self>) {
        let (tx, mut rx) = mpsc::channel::<ApprovalRequest>(32);
        let broker = self.codex.handle().approvals.clone();
        tokio::spawn(async move {
            broker.install_handler(tx).await;
        });
        let app_for_loop = self.clone();
        tokio::spawn(async move {
            while let Some(request) = rx.recv().await {
                app_for_loop.clone().route_approval_request(request).await;
            }
        });
    }

    async fn route_approval_request(self: Arc<Self>, request: ApprovalRequest) {
        let Some(ctx) = self.active_openid.lock().await.clone() else {
            // No active turn owner — decline so the server can proceed.
            warn!("approval request arrived with no active turn owner; declining");
            decline_approval_request(request);
            return;
        };
        let openid = ctx.openid.clone();
        let reply_id = ctx.reply_message_id.clone();
        match request {
            ApprovalRequest::Command { event, reply } => {
                let prompt = format_command_approval(&event, &openid);
                self.enqueue_outcome(openid.clone(), reply_id, prompt, reply)
                    .await;
            }
            ApprovalRequest::FileChange { event, reply } => {
                let prompt = format_file_change_approval(&event);
                self.enqueue_outcome(openid.clone(), reply_id, prompt, reply)
                    .await;
            }
            ApprovalRequest::Permissions { event, reply } => {
                let prompt = format_permissions_approval(&event);
                self.enqueue_outcome(openid.clone(), reply_id, prompt, reply)
                    .await;
            }
            ApprovalRequest::Elicitation { event, reply } => {
                // MCP elicitations are free-form — out of scope for MVP.
                warn!(
                    thread_id = %event.thread_id,
                    server = event.server.as_deref().unwrap_or(""),
                    "MCP elicitation received; auto-declining (not yet wired to QQ)"
                );
                let _ = reply.send(None);
            }
        }
    }

    async fn enqueue_outcome(
        self: Arc<Self>,
        openid: String,
        reply_message_id: String,
        prompt: String,
        tx: oneshot::Sender<ApprovalOutcome>,
    ) {
        let mut guard = self.pending_approvals.lock().await;
        let slot = guard.entry(openid.clone()).or_default();
        slot.push_back(PendingApprovalEntry::Outcome(tx));
        drop(guard);
        if let Err(err) = self
            .qq_client
            .send_text(&openid, &reply_message_id, &prompt, Some(&reply_message_id))
            .await
        {
            warn!(error = %err, openid = %openid, "failed to deliver approval prompt to QQ");
        }
    }

    /// Resolve the oldest pending approval for `openid` with `intent`.
    /// Returns `true` if a pending approval was resolved; `false` if there
    /// was none (caller should tell the user).
    async fn resolve_pending_approval(&self, openid: &str, intent: ApprovalIntent) -> bool {
        let mut guard = self.pending_approvals.lock().await;
        let Some(queue) = guard.get_mut(openid) else {
            return false;
        };
        let Some(entry) = queue.pop_front() else {
            return false;
        };
        let PendingApprovalEntry::Outcome(tx) = entry;
        let outcome = match intent {
            ApprovalIntent::Accept => ApprovalOutcome::Accept,
            ApprovalIntent::AcceptForSession => ApprovalOutcome::AcceptForSession,
            ApprovalIntent::Decline => ApprovalOutcome::Decline,
            ApprovalIntent::Cancel => ApprovalOutcome::Cancel,
        };
        let _ = tx.send(outcome);
        true
    }

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

        match maybe_handle_command(
            &normalized.text,
            &normalized.sender_openid,
            &self.session,
            &self.config.general.default_model,
            &runtime_profile,
            self.busy.load(Ordering::SeqCst),
        )
        .await?
        {
            CommandOutcome::Reply(reply) => {
                info!(
                    sender_openid = %normalized.sender_openid,
                    message_id = %normalized.message_id,
                    "handled as direct command"
                );
                self.send_command_reply(&normalized.sender_openid, &normalized.message_id, &reply)
                    .await?;
                return Ok(());
            }
            CommandOutcome::CancelCurrent(message) => {
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
            CommandOutcome::SelfUpdate => {
                self.handle_self_update_command(&normalized.sender_openid, &normalized.message_id)
                    .await?;
                return Ok(());
            }
            CommandOutcome::Compact => {
                self.handle_compact_command(
                    &normalized.sender_openid,
                    &normalized.message_id,
                    &runtime_profile,
                )
                .await?;
                return Ok(());
            }
            CommandOutcome::SetGlobalModel(value) => {
                let lang = self.command_locale(&normalized.sender_openid).await;
                let profile_path = self.runtime_profile_path();
                write_model_to_config_path(&profile_path, value.as_deref())?;
                let updated_profile = read_codex_runtime_profile_from_path(&profile_path);
                let effective_model = updated_profile
                    .configured_model
                    .unwrap_or_else(|| self.config.general.default_model.clone());
                let msg = t!(
                    "commands.model.updated",
                    model = effective_model,
                    locale = lang.as_str()
                )
                .into_owned();
                self.qq_client
                    .send_text(
                        &normalized.sender_openid,
                        &normalized.message_id,
                        &msg,
                        Some(&normalized.message_id),
                    )
                    .await?;
                return Ok(());
            }
            CommandOutcome::SetGlobalReasoning(value) => {
                let lang = self.command_locale(&normalized.sender_openid).await;
                let profile_path = self.runtime_profile_path();
                write_reasoning_effort_to_config_path(&profile_path, value)?;
                let updated_profile = read_codex_runtime_profile_from_path(&profile_path);
                let effective_reasoning = updated_profile
                    .reasoning_effort
                    .unwrap_or(self.config.general.default_reasoning_effort)
                    .as_str();
                let msg = t!(
                    "commands.reasoning.updated",
                    value = effective_reasoning,
                    locale = lang.as_str()
                )
                .into_owned();
                self.qq_client
                    .send_text(
                        &normalized.sender_openid,
                        &normalized.message_id,
                        &msg,
                        Some(&normalized.message_id),
                    )
                    .await?;
                return Ok(());
            }
            CommandOutcome::SetGlobalFast(value) => {
                let lang = self.command_locale(&normalized.sender_openid).await;
                let profile_path = self.runtime_profile_path();
                write_service_tier_to_config_path(&profile_path, value)?;
                let updated_profile = read_codex_runtime_profile_from_path(&profile_path);
                let msg = t!(
                    "commands.fast.updated",
                    value = global_fast_label(updated_profile.service_tier),
                    locale = lang.as_str()
                )
                .into_owned();
                self.qq_client
                    .send_text(
                        &normalized.sender_openid,
                        &normalized.message_id,
                        &msg,
                        Some(&normalized.message_id),
                    )
                    .await?;
                return Ok(());
            }
            CommandOutcome::SetGlobalContext(value) => {
                let lang = self.command_locale(&normalized.sender_openid).await;
                let profile_path = self.runtime_profile_path();
                write_context_mode_to_config_path(&profile_path, value)?;
                let updated_profile = read_codex_runtime_profile_from_path(&profile_path);
                let msg = t!(
                    "commands.context.updated",
                    value = global_context_label(updated_profile.context_mode),
                    locale = lang.as_str()
                )
                .into_owned();
                self.qq_client
                    .send_text(
                        &normalized.sender_openid,
                        &normalized.message_id,
                        &msg,
                        Some(&normalized.message_id),
                    )
                    .await?;
                return Ok(());
            }
            CommandOutcome::Approval(intent) => {
                let resolved = self
                    .resolve_pending_approval(&normalized.sender_openid, intent)
                    .await;
                let zh = self
                    .session
                    .snapshot_for_user(&normalized.sender_openid)
                    .await
                    .ok()
                    .map(|snap| snap.settings.language.starts_with("zh"))
                    .unwrap_or(true);
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
                self.qq_client
                    .send_text(
                        &normalized.sender_openid,
                        &normalized.message_id,
                        msg,
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
            if let Some(queue) = guard.remove(&openid_for_cleanup) {
                if !queue.is_empty() {
                    debug!(
                        openid = %openid_for_cleanup,
                        dropped = queue.len(),
                        "dropping pending approvals after turn completion"
                    );
                }
            }
        }
        self.busy.store(false, Ordering::SeqCst);
        result
    }

    async fn run_turn(
        &self,
        message: IncomingMessage,
        runtime_profile: crate::codex::runtime::CodexRuntimeProfile,
    ) -> Result<()> {
        let user_snapshot = self
            .session
            .snapshot_for_user(&message.sender_openid)
            .await?;
        let effective_settings = effective_session_settings(&user_snapshot);
        let runtime_state = SessionState {
            session_id: user_snapshot.foreground.session_id.clone(),
            settings: effective_settings.clone(),
        };
        let workspace_dir = user_snapshot.foreground.workspace_dir.clone();
        let shared_workspace_dir = self.session.attachment_workspace_dir().to_path_buf();
        let codex_home = self.session.codex_home().to_path_buf();
        let effective_model = effective_settings
            .model_override
            .clone()
            .or_else(|| runtime_profile.configured_model.clone())
            .unwrap_or_else(|| self.config.general.default_model.clone());
        let reasoning = effective_settings
            .reasoning_effort
            .or(runtime_profile.reasoning_effort)
            .unwrap_or(self.config.general.default_reasoning_effort);
        let service_tier = runtime_profile.service_tier;
        let context_mode = effective_settings
            .context_mode
            .or(runtime_profile.context_mode);
        let memory_block = match self.memory.snapshot_for(&message.sender_openid) {
            Ok(snap) => memory_inject::render(&snap),
            Err(err) => {
                warn!(
                    error = %err,
                    openid = %message.sender_openid,
                    "failed to load memory snapshot; continuing without it",
                );
                None
            }
        };
        let prompt = build_prompt(
            &message,
            &runtime_state.settings,
            &effective_model,
            &workspace_dir,
            &shared_workspace_dir,
            &self.config.general.self_repo_dir,
            memory_block.as_deref(),
        );
        let mut add_dirs = vec![self.session.inbox_dir().to_path_buf()];
        if workspace_dir != shared_workspace_dir {
            add_dirs.push(shared_workspace_dir.clone());
        }
        info!(
            sender_openid = %message.sender_openid,
            message_id = %message.message_id,
            model = %effective_model,
            reasoning = %reasoning.as_str(),
            codex_home = %codex_home.display(),
            workspace_dir = %workspace_dir.display(),
            "starting codex turn"
        );

        let (update_tx, update_rx) = mpsc::unbounded_channel();
        let emitter = tokio::spawn(
            PassiveTurnEmitter::new(
                self.qq_client.clone(),
                message.sender_openid.clone(),
                message.message_id.clone(),
                workspace_dir.clone(),
                effective_settings.verbose,
            )
            .run(update_rx),
        );

        let execution = self
            .codex
            .execute(
                ExecutionRequest {
                    prompt,
                    workspace_dir: workspace_dir.clone(),
                    codex_home,
                    config_overrides: Vec::new(),
                    add_dirs,
                    session_state: runtime_state,
                    model: Some(effective_model.clone()),
                    service_tier,
                    context_mode,
                    reasoning_effort: reasoning,
                    image_paths: message
                        .images
                        .iter()
                        .map(|image| image.local_path.clone())
                        .collect(),
                },
                Some(self.install_active_turn().await),
                Some(update_tx),
            )
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
                if let Some(session_id) = output.session_id.clone() {
                    self.session
                        .bind_foreground_session_profile(
                            &message.sender_openid,
                            Some(session_id),
                            DialogProfile {
                                model_override: Some(effective_model.clone()),
                                reasoning_effort: Some(reasoning),
                                service_tier: None,
                                context_mode,
                            },
                        )
                        .await?;
                } else {
                    self.session
                        .set_foreground_session_id(&message.sender_openid, None)
                        .await?;
                }
                let usage_snapshot = if let Some(info) = output.token_usage_info {
                    let window = info
                        .model_context_window
                        .or(output.context_window)
                        .or_else(|| context_mode.map(compact::context_mode_window))
                        .unwrap_or(ContextMode::STANDARD_CONTEXT_WINDOW);
                    let context_usage = info.context_window_usage().clone();
                    let snapshot = TokenUsageSnapshot {
                        total_tokens: context_usage.tokens_in_context_window(),
                        window,
                        input_tokens: context_usage.input_tokens,
                        cached_input_tokens: context_usage.cached_input_tokens,
                        output_tokens: context_usage.output_tokens,
                        updated_at: chrono::Utc::now(),
                    };
                    let _ = self
                        .session
                        .set_foreground_usage(&message.sender_openid, snapshot.clone())
                        .await;
                    Some(snapshot)
                } else {
                    None
                };
                let lang_for_warning = self
                    .session
                    .snapshot_for_user(&message.sender_openid)
                    .await
                    .ok()
                    .map(|snap| snap.settings.language)
                    .unwrap_or_else(|| "en".to_string());
                let context_warning = usage_snapshot
                    .as_ref()
                    .and_then(|snap| build_context_warning(snap, &lang_for_warning));
                if !dispatch_report.saw_agent_message {
                    let parsed = parse_output(&output.text, &workspace_dir);
                    let mut payload = parsed.text.clone();
                    if let Some(warning) = context_warning.as_deref() {
                        if !payload.is_empty() {
                            payload.push_str("\n\n");
                        }
                        payload.push_str(warning);
                    }
                    if !payload.is_empty() {
                        self.qq_client
                            .send_text(
                                &message.sender_openid,
                                &message.message_id,
                                &payload,
                                Some(&message.message_id),
                            )
                            .await?;
                    }
                    for directive in parsed.directives {
                        self.send_directive(&message.sender_openid, &message.message_id, directive)
                            .await?;
                    }
                } else if let Some(warning) = context_warning.as_deref() {
                    self.qq_client
                        .send_text(
                            &message.sender_openid,
                            &message.message_id,
                            warning,
                            Some(&message.message_id),
                        )
                        .await?;
                }

                // Plan-mode post-turn: if the planning turn produced a
                // `<proposed_plan>` block, stash it + prompt the user to
                // approve it via `/实施`.
                if effective_settings.plan_mode {
                    if let Some(plan) = extract_proposed_plan(&output.text) {
                        let _ = self
                            .session
                            .update_settings_for_user(&message.sender_openid, |settings| {
                                settings.pending_plan = Some(plan.clone());
                            })
                            .await;
                        let lang = effective_settings.language.as_str();
                        let prompt = build_plan_followup_prompt(lang);
                        let _ = self
                            .qq_client
                            .send_text(
                                &message.sender_openid,
                                &message.message_id,
                                &prompt,
                                Some(&message.message_id),
                            )
                            .await;
                    }
                }

                if let Some(worker) = self.shadow.as_ref() {
                    let ctx = ShadowContext {
                        openid: message.sender_openid.clone(),
                        last_user_text: message.text.clone(),
                        last_assistant_text: output.text.clone(),
                        tool_call_count: dispatch_report.tool_call_count,
                        modified_file_count: output.changed_files.len(),
                    };
                    worker.spawn_memory(ctx.clone());
                    worker.spawn_skill(ctx);
                }
                if self_update::changed_self_repo(
                    &workspace_dir,
                    &output.changed_files,
                    &self.config.general.self_repo_dir,
                ) {
                    let text = match self_update::run_build(&self.config).await {
                        Ok(build_result) => format!(
                            "检测到修改了 codex-claw 源码，已自动触发构建：\n{}",
                            build_result.summary
                        ),
                        Err(err) => {
                            format!("检测到修改了 codex-claw 源码，但自动构建触发失败：{err}")
                        }
                    };
                    self.qq_client
                        .send_text(
                            &message.sender_openid,
                            &message.message_id,
                            &text,
                            Some(&message.message_id),
                        )
                        .await?;
                }
                Ok(())
            }
            Err(err) => {
                if err.to_string().contains("aborted by user") {
                    info!("codex turn aborted by operator");
                    return Ok(());
                }
                error!("codex execution failed: {err:#}");
                let text = self
                    .format_execution_error_message(&err, &workspace_dir)
                    .unwrap_or_else(|| format!("Codex 执行失败：{err}"));
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

    fn runtime_profile_path(&self) -> PathBuf {
        let codex_home = &self.config.general.codex_home_global;
        codex_home.join("config.toml")
    }

    async fn command_locale(&self, openid: &str) -> String {
        self.session
            .snapshot_for_user(openid)
            .await
            .ok()
            .map(|snap| snap.settings.language)
            .unwrap_or_else(|| "zh".to_string())
    }

    fn format_execution_error_message(
        &self,
        err: &anyhow::Error,
        workspace_dir: &std::path::Path,
    ) -> Option<String> {
        let raw = err.to_string();
        if !raw.contains("Operation not permitted (os error 1)") {
            return None;
        }
        let current_binary = std::env::current_exe()
            .map(|path| format!("`{}`", path.display()))
            .unwrap_or_else(|_| "`~/.codex-claw/bin/codex-claw`".to_string());
        Some(format!(
            "Codex 执行失败：系统返回 `Operation not permitted (os error 1)`。\n\
这通常是 macOS 的文件权限（TCC）限制导致的。\n\
请执行以下检查：\n\
1) 在“系统设置 -> 隐私与安全性 -> 完全磁盘访问”里，允许 {current_binary} 与 `{}`
\n\
2) 重启服务：`launchctl kickstart -k gui/$(id -u)/com.codex-claw`\n\
3) 若仍失败，可先把工作目录换到非 `Desktop/Documents/Downloads` 的路径后再 `/new <目录>`\n\
当前工作目录：`{}`",
            self.config.general.codex_binary,
            workspace_dir.display()
        ))
    }

    async fn handle_self_update_command(&self, openid: &str, message_id: &str) -> Result<()> {
        if self.busy.load(Ordering::SeqCst) {
            self.qq_client
                .send_text(
                    openid,
                    message_id,
                    "当前有任务在运行，请先等待当前任务完成后再执行 `/self-update`。",
                    Some(message_id),
                )
                .await?;
            return Ok(());
        }
        let build_result = self_update::ensure_successful_build(&self.config).await?;
        if !build_result.success {
            self.qq_client
                .send_text(openid, message_id, &build_result.summary, Some(message_id))
                .await?;
            return Ok(());
        }
        let running_binary =
            std::env::current_exe().context("failed to detect current executable")?;
        self_update::replace_binary_for_restart(&build_result.binary_path, &running_binary).await?;
        self.qq_client
            .send_text(
                openid,
                message_id,
                &format!(
                    "已覆盖运行中的二进制：`{}`\n即将退出当前进程（已通知 codex app-server 关闭）。若已配置外部守护服务，将自动重启；否则请手动重新启动。",
                    running_binary.display()
                ),
                Some(message_id),
            )
            .await?;
        // Gracefully shut down the shared codex app-server child before
        // exiting — `std::process::exit` skips Drop impls, so `kill_on_drop`
        // won't fire and the child would otherwise be reparented to init.
        // A lingering app-server sharing CODEX_HOME with our replacement
        // process would corrupt SQLite / rollout files.
        info!("shutting down app-server child before self-update exit");
        self.codex.handle().shutdown().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::process::exit(0);
        #[allow(unreachable_code)]
        Ok(())
    }

    async fn handle_compact_command(
        &self,
        openid: &str,
        message_id: &str,
        runtime_profile: &crate::codex::runtime::CodexRuntimeProfile,
    ) -> Result<()> {
        if self.busy.swap(true, Ordering::SeqCst) {
            let lang = self
                .session
                .snapshot_for_user(openid)
                .await
                .map(|snapshot| snapshot.settings.language)
                .unwrap_or_else(|_| "zh".to_string());
            self.qq_client
                .send_text(
                    openid,
                    message_id,
                    &t!("commands.compact.busy", locale = lang.as_str()),
                    Some(message_id),
                )
                .await?;
            return Ok(());
        }

        let result = self
            .handle_compact_command_inner(openid, message_id, runtime_profile)
            .await;
        self.busy.store(false, Ordering::SeqCst);
        result
    }

    async fn handle_compact_command_inner(
        &self,
        openid: &str,
        message_id: &str,
        runtime_profile: &crate::codex::runtime::CodexRuntimeProfile,
    ) -> Result<()> {
        let user_snapshot = self.session.snapshot_for_user(openid).await?;
        let lang = user_snapshot.settings.language.clone();
        let locale = lang.as_str();
        let Some(session_id) = user_snapshot.foreground.session_id.clone() else {
            self.qq_client
                .send_text(
                    openid,
                    message_id,
                    &t!("commands.compact.missing_session", locale = locale),
                    Some(message_id),
                )
                .await?;
            return Ok(());
        };
        self.qq_client
            .send_text(
                openid,
                message_id,
                &t!("commands.compact.start", locale = locale),
                Some(message_id),
            )
            .await?;

        let effective_settings = effective_session_settings(&user_snapshot);
        let effective_model = effective_settings
            .model_override
            .clone()
            .or_else(|| runtime_profile.configured_model.clone())
            .unwrap_or_else(|| self.config.general.default_model.clone());
        let reasoning = effective_settings
            .reasoning_effort
            .or(runtime_profile.reasoning_effort)
            .unwrap_or(self.config.general.default_reasoning_effort);
        let context_mode = effective_settings
            .context_mode
            .or(runtime_profile.context_mode);
        let request = CompactRequest {
            session_id: session_id.clone(),
            workspace_dir: user_snapshot.foreground.workspace_dir.clone(),
            config_overrides: Vec::new(),
            model: Some(effective_model.clone()),
            service_tier: runtime_profile.service_tier,
            context_mode,
            reasoning_effort: reasoning,
        };

        match self.run_session_compaction(openid, request).await {
            Ok(()) => {
                let text = format!(
                    "{}\n\n{}",
                    t!("commands.compact.success", locale = locale),
                    t!("commands.compact.warning", locale = locale),
                );
                self.qq_client
                    .send_text(openid, message_id, &text, Some(message_id))
                    .await?;
            }
            Err(err) => {
                if err.to_string().contains("aborted by user") {
                    return Ok(());
                }
                self.qq_client
                    .send_text(
                        openid,
                        message_id,
                        &format!(
                            "{}: {err:#}",
                            t!("commands.compact.failed", locale = locale)
                        ),
                        Some(message_id),
                    )
                    .await?;
            }
        }

        Ok(())
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

    #[allow(clippy::too_many_arguments)]
    async fn run_session_compaction(&self, openid: &str, request: CompactRequest) -> Result<()> {
        info!(
            openid = %openid,
            session_id = %request.session_id,
            model = ?request.model,
            reasoning = %request.reasoning_effort.as_str(),
            context_mode = ?request.context_mode,
            workspace_dir = %request.workspace_dir.display(),
            "triggering codex session compaction"
        );

        let compaction = self
            .codex
            .compact_session(request, Some(self.install_active_turn().await))
            .await;
        self.clear_active_turn().await;
        compaction?;
        Ok(())
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
                let info = self.qq_client.upload_file(openid, &path, 1, None).await?;
                self.qq_client.send_media(openid, message_id, &info).await?;
            }
            Directive::File { path, name } => {
                info!(path = %path.display(), "sending file directive to qq");
                let info = self
                    .qq_client
                    .upload_file(openid, &path, 4, name.as_deref())
                    .await?;
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

fn format_tokens_compact(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{}K", (value + 500) / 1_000)
    } else {
        value.to_string()
    }
}

fn effective_session_settings(
    snapshot: &UserSessionState,
) -> crate::session::state::SessionSettings {
    let mut base = snapshot.settings.clone();
    base.model_override = None;
    base.reasoning_effort = None;
    base.service_tier = None;
    base.context_mode = None;
    let profile = if snapshot.foreground.saved {
        snapshot.foreground.profile.as_ref()
    } else {
        None
    };
    base.merged_with_profile(profile)
}

fn global_fast_label(service_tier: Option<ServiceTier>) -> &'static str {
    match service_tier {
        Some(ServiceTier::Fast) => "on",
        Some(ServiceTier::Flex) => "off",
        None => "inherit",
    }
}

fn global_context_label(context_mode: Option<ContextMode>) -> &'static str {
    match context_mode {
        Some(ContextMode::Standard) => "272K",
        Some(ContextMode::OneM) => "1M",
        None => "inherit",
    }
}

fn decline_approval_request(request: ApprovalRequest) {
    match request {
        ApprovalRequest::Command { reply, .. } => {
            let _ = reply.send(ApprovalOutcome::Decline);
        }
        ApprovalRequest::FileChange { reply, .. } => {
            let _ = reply.send(ApprovalOutcome::Decline);
        }
        ApprovalRequest::Permissions { reply, .. } => {
            let _ = reply.send(ApprovalOutcome::Decline);
        }
        ApprovalRequest::Elicitation { reply, .. } => {
            let _ = reply.send(None);
        }
    }
}

fn format_command_approval(event: &CommandApprovalEvent, _openid: &str) -> String {
    let mut lines = vec!["[审批请求] Codex 想执行 shell 命令".to_string()];
    if let Some(cmd) = event.command.as_deref() {
        lines.push("命令：".to_string());
        lines.push(format!("```shell\n{cmd}\n```"));
    }
    if let Some(cwd) = event.cwd.as_deref() {
        lines.push(format!("目录：`{cwd}`"));
    }
    if let Some(reason) = event.reason.as_deref().filter(|r| !r.trim().is_empty()) {
        lines.push(format!("原因：{}", reason.trim()));
    }
    lines.push(
        "——\n/同意            仅本次放行\n/同意本会话      本轮后续同类命令自动放行\n/拒绝            拒绝，Codex 会尝试别的方式\n/取消            拒绝并终止当前回合"
            .to_string(),
    );
    lines.join("\n")
}

fn format_file_change_approval(event: &FileChangeApprovalEvent) -> String {
    let mut lines = vec!["[审批请求] Codex 想写入/修改文件".to_string()];
    if let Some(reason) = event.reason.as_deref().filter(|r| !r.trim().is_empty()) {
        lines.push(format!("原因：{}", reason.trim()));
    }
    if let Some(root) = event.grant_root.as_deref() {
        lines.push(format!("授权目录：`{root}`"));
    }
    let summary = summarize_file_changes(&event.file_changes);
    if !summary.is_empty() {
        lines.push(format!("变更：{summary}"));
    }
    lines.push("——\n/同意 /同意本会话 /拒绝 /取消".to_string());
    lines.join("\n")
}

fn format_permissions_approval(event: &PermissionsApprovalEvent) -> String {
    let mut lines = vec!["[审批请求] Codex 请求权限升级".to_string()];
    if let Some(reason) = event.reason.as_deref().filter(|r| !r.trim().is_empty()) {
        lines.push(format!("原因：{}", reason.trim()));
    }
    let summary = serde_json::to_string(&event.permissions).unwrap_or_default();
    if !summary.is_empty() && summary != "null" {
        let trimmed: String = summary.chars().take(400).collect();
        lines.push(format!("请求：{trimmed}"));
    }
    lines.push("——\n/同意 /拒绝 /取消".to_string());
    lines.join("\n")
}

fn summarize_file_changes(payload: &serde_json::Value) -> String {
    // Payload shape is a map of path -> change descriptor or an array.
    let mut paths: Vec<String> = Vec::new();
    match payload {
        serde_json::Value::Object(map) => {
            for (k, _) in map.iter().take(6) {
                paths.push(k.clone());
            }
        }
        serde_json::Value::Array(arr) => {
            for entry in arr.iter().take(6) {
                if let Some(p) = entry.get("path").and_then(|v| v.as_str()) {
                    paths.push(p.to_string());
                }
            }
        }
        _ => {}
    }
    if paths.is_empty() {
        return String::new();
    }
    paths.join(", ")
}

/// Pull a `<proposed_plan>...</proposed_plan>` block out of a plan-mode turn's
/// final output. Tolerates extra whitespace and unwrapped code fences.
pub fn extract_proposed_plan(text: &str) -> Option<String> {
    const OPEN: &str = "<proposed_plan>";
    const CLOSE: &str = "</proposed_plan>";
    let start = text.find(OPEN)? + OPEN.len();
    let relative_end = text[start..].find(CLOSE)?;
    let plan = text[start..start + relative_end].trim();
    if plan.is_empty() {
        None
    } else {
        Some(plan.to_string())
    }
}

/// Follow-up QQ prompt shown after a plan-mode turn emits a `<proposed_plan>`
/// block.
pub fn build_plan_followup_prompt(lang: &str) -> String {
    let zh = lang.starts_with("zh");
    if zh {
        "Codex 已生成执行计划。接下来请选择：\n\
         /实施          退出 Plan 模式并按此计划执行\n\
         /继续规划      保持 Plan 模式，继续打磨\n\
         /取消计划      丢弃此计划"
            .to_string()
    } else {
        "Codex produced an execution plan. Next step:\n\
         /execute-plan   leave plan mode and run the plan\n\
         /keep-planning  stay in plan mode and refine\n\
         /cancel-plan    discard the plan"
            .to_string()
    }
}

#[cfg(test)]
mod plan_followup_tests {
    use super::extract_proposed_plan;

    #[test]
    fn extracts_plan_block() {
        let text = "Intro text\n<proposed_plan>\n1. Do X\n2. Do Y\n</proposed_plan>\nOutro";
        assert_eq!(
            extract_proposed_plan(text).as_deref(),
            Some("1. Do X\n2. Do Y")
        );
    }

    #[test]
    fn returns_none_without_block() {
        assert_eq!(extract_proposed_plan("no plan here"), None);
    }

    #[test]
    fn returns_none_for_empty_block() {
        assert_eq!(
            extract_proposed_plan("<proposed_plan>   \n</proposed_plan>"),
            None
        );
    }
}

fn build_context_warning(snapshot: &TokenUsageSnapshot, lang: &str) -> Option<String> {
    let percent = snapshot.percent_used()?;
    if (percent as f64 / 100.0) < CONTEXT_WARNING_THRESHOLD {
        return None;
    }
    let used_tokens = snapshot.context_tokens()?;
    let lang = normalize_lang(lang);
    Some(
        t!(
            "warnings.context_near_limit",
            percent = percent,
            used = format_tokens_compact(used_tokens),
            total = format_tokens_compact(snapshot.window),
            locale = lang
        )
        .into_owned(),
    )
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
    use crate::session::state::TokenUsageSnapshot;

    use super::{build_context_warning, extract_quote};

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

    #[test]
    fn context_warning_uses_context_window_percentage() {
        let warning = build_context_warning(
            &TokenUsageSnapshot {
                total_tokens: 220_000,
                window: 272_000,
                input_tokens: 0,
                cached_input_tokens: 0,
                output_tokens: 0,
                updated_at: chrono::Utc::now(),
            },
            "en",
        )
        .expect("warning");
        assert!(
            warning.contains("80% used"),
            "unexpected warning: {warning}"
        );
        assert!(warning.contains("220K used / 272K"));
        assert!(warning.contains("`/compact`"));
        assert!(!warning.contains("`/压缩`"));
    }

    #[test]
    fn context_warning_localizes_compact_command_name() {
        let warning = build_context_warning(
            &TokenUsageSnapshot {
                total_tokens: 220_000,
                window: 272_000,
                input_tokens: 0,
                cached_input_tokens: 0,
                output_tokens: 0,
                updated_at: chrono::Utc::now(),
            },
            "zh",
        )
        .expect("warning");
        assert!(warning.contains("`/压缩`"), "unexpected warning: {warning}");
        assert!(!warning.contains("`/compact`"));
    }

    #[test]
    fn context_warning_skips_implausible_legacy_cumulative_usage() {
        let warning = build_context_warning(
            &TokenUsageSnapshot {
                total_tokens: 19_668_612,
                window: 1_000_000,
                input_tokens: 19_568_077,
                cached_input_tokens: 18_968_448,
                output_tokens: 100_535,
                updated_at: chrono::Utc::now(),
            },
            "zh",
        );
        assert!(warning.is_none());
    }
}
