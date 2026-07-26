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
        ApprovalOutcome, ApprovalRequest, CodexExecutor, CodexRuntimeProfile, CommandApprovalEvent,
        CompactRequest, ExecutionRequest, ExecutionResult, ExecutionUpdate,
        FileChangeApprovalEvent, PermissionsApprovalEvent, TokenUsageInfo, build_prompt,
        read_codex_runtime_profile_from_path, write_context_mode_to_config_path,
        write_model_to_config_path, write_reasoning_effort_to_config_path,
        write_service_tier_to_config_path,
    },
    commands::{ApprovalIntent, CommandOutcome, maybe_handle_command},
    config::AppConfig,
    memory::{inject as memory_inject, store::MemoryStore},
    message::{IncomingAttachment, IncomingMessage, QuotedMessage},
    qq::{
        C2CMessageEvent, Directive, MSG_TYPE_QUOTE, MessageAttachment, MsgElement,
        PassiveDispatchReport, PassiveTurnEmitter, QqApiClient, parse_output,
    },
    self_update,
    session::{
        SessionStore,
        state::{
            ContextMode, DialogProfile, DialogState, PendingSetting, ReasoningEffort, ServiceTier,
            SessionSettings, SessionState, TokenUsageSnapshot, UserSessionState,
        },
    },
    shadow::{ShadowContext, ShadowWorker},
    util::{lang::normalize_lang, layout::DataLayout, text::format_tokens_compact},
};

const CONTEXT_WARNING_THRESHOLD: f64 = 0.80;

pub struct App {
    pub config: AppConfig,
    pub(crate) session: Arc<SessionStore>,
    pub qq_client: Arc<QqApiClient>,
    pub codex: Arc<CodexExecutor>,
    pub(crate) memory: Arc<MemoryStore>,
    pub(crate) shadow: Option<Arc<ShadowWorker>>,
    busy: AtomicBool,
    active_turn: Mutex<Option<oneshot::Sender<()>>>,
    /// The QQ openid whose turn currently holds `busy`. Used to route
    /// server-initiated approval requests to the right user.
    active_openid: Mutex<Option<ActiveTurnContext>>,
    /// Queued approval decisions awaiting user reply. FIFO per openid.
    pending_approvals: Mutex<HashMap<String, VecDeque<PendingApprovalEntry>>>,
    pending_resume_messages: Mutex<HashMap<String, IncomingMessage>>,
}

#[derive(Clone)]
struct ActiveTurnContext {
    openid: String,
    reply_message_id: String,
}

enum PendingApprovalEntry {
    Outcome(oneshot::Sender<ApprovalOutcome>),
}

/// RAII ownership of the singleton `App::busy` slot: dropping the guard
/// releases the slot, so every early-return and panic path unwinds it without
/// a hand-written `store(false)`. Acquired via [`App::try_acquire_busy`].
struct BusyGuard<'a> {
    busy: &'a AtomicBool,
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::SeqCst);
    }
}

/// Everything `run_turn` resolves once up front and threads through its
/// phases: the user's session snapshot, the settings the turn runs with, the
/// filesystem layout, and the effective (model, reasoning, context) triple.
struct TurnSetup {
    user_snapshot: UserSessionState,
    effective_settings: SessionSettings,
    runtime_state: SessionState,
    workspace_dir: PathBuf,
    shared_workspace_dir: PathBuf,
    codex_home: PathBuf,
    effective_model: String,
    reasoning: ReasoningEffort,
    context_mode: Option<ContextMode>,
    service_tier: Option<ServiceTier>,
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
            pending_resume_messages: Mutex::new(HashMap::new()),
        });
        app.clone().install_approval_handler();
        app
    }

    /// Try to reserve the singleton busy slot (one turn at a time across all
    /// users). Returns `None` when another turn already holds it; the caller
    /// should report "busy" to the user and bail.
    fn try_acquire_busy(&self) -> Option<BusyGuard<'_>> {
        if self.busy.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(BusyGuard { busy: &self.busy })
        }
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
                let prompt = format_command_approval(&event);
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
        if let Err(err) = self.reply_text(&openid, &reply_message_id, &prompt).await {
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
            crate::scheduler::finish_job_for_owner(self, &normalized.sender_openid, "stopped")
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

        let command_outcome = maybe_handle_command(
            &normalized.text,
            &normalized.sender_openid,
            &self.session,
            &self.config.general.default_model,
            &runtime_profile,
            self.busy.load(Ordering::SeqCst),
        )
        .await?;
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
            self.reply_text(
                &normalized.sender_openid,
                &normalized.message_id,
                "上一轮仍在处理中，请稍后再试。",
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

    async fn run_turn(
        &self,
        message: IncomingMessage,
        runtime_profile: CodexRuntimeProfile,
    ) -> Result<()> {
        let setup = self.prepare_turn_setup(&message, &runtime_profile).await?;
        let memory_block = self.load_memory_block(&message.sender_openid).await;
        let (prompt, add_dirs) = self
            .assemble_prompt_and_dirs(&message, &setup, memory_block.as_deref())
            .await;

        let (update_tx, update_rx) = mpsc::unbounded_channel();
        let strip_signal = self
            .detect_strip_signal(&message.sender_openid, &setup.user_snapshot.foreground)
            .await;
        let emitter = self.spawn_reply_emitter(&message, &setup, strip_signal, update_rx);

        let execution = self
            .codex
            .execute(
                ExecutionRequest {
                    prompt,
                    workspace_dir: setup.workspace_dir.clone(),
                    codex_home: setup.codex_home.clone(),
                    config_overrides: Vec::new(),
                    add_dirs,
                    session_state: setup.runtime_state.clone(),
                    model: Some(setup.effective_model.clone()),
                    service_tier: setup.service_tier,
                    context_mode: setup.context_mode,
                    reasoning_effort: setup.reasoning,
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
        let dispatch_report = await_dispatch_report(emitter, &message.message_id).await;

        match execution {
            Ok(output) => {
                self.finish_turn_success(&message, &setup, output, &dispatch_report)
                    .await
            }
            Err(err) => {
                self.finish_turn_failure(&message, &setup, err, &dispatch_report)
                    .await
            }
        }
    }

    /// `run_turn` phase 1: resolve the session snapshot, effective settings,
    /// workspace/codex-home paths and the runtime triple this turn runs with,
    /// and drop the turn-context marker into the workspace.
    async fn prepare_turn_setup(
        &self,
        message: &IncomingMessage,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<TurnSetup> {
        let user_snapshot = self
            .session
            .snapshot_for_user(&message.sender_openid)
            .await?;
        let effective_settings = user_snapshot.effective_settings();
        let runtime_state = SessionState {
            session_id: user_snapshot.foreground.session_id.clone(),
            settings: effective_settings.clone(),
        };
        let workspace_dir = user_snapshot.foreground.workspace_dir.clone();
        let shared_workspace_dir = self.session.attachment_workspace_dir().to_path_buf();
        let codex_home = self.session.codex_home().to_path_buf();
        write_turn_context(&workspace_dir, &message.sender_openid).await;
        let (effective_model, reasoning, context_mode) =
            self.effective_runtime_triple(&effective_settings, runtime_profile);
        let service_tier = runtime_profile.service_tier;
        Ok(TurnSetup {
            user_snapshot,
            effective_settings,
            runtime_state,
            workspace_dir,
            shared_workspace_dir,
            codex_home,
            effective_model,
            reasoning,
            context_mode,
            service_tier,
        })
    }

    /// `run_turn` phase 2: load the user's memory snapshot. snapshot_for does
    /// synchronous file reads (and fsync on the write paths); run it off the
    /// reactor so the per-message hot path never blocks a tokio worker thread.
    /// Failures degrade to running the turn without a memory block.
    async fn load_memory_block(&self, openid: &str) -> Option<String> {
        let memory = self.memory.clone();
        let snapshot_openid = openid.to_string();
        match tokio::task::spawn_blocking(move || memory.snapshot_for(&snapshot_openid)).await {
            Ok(Ok(snap)) => memory_inject::render(&snap),
            Ok(Err(err)) => {
                warn!(
                    error = %err,
                    openid = %openid,
                    "failed to load memory snapshot; continuing without it",
                );
                None
            }
            Err(err) => {
                warn!(error = %err, "memory snapshot task panicked; continuing without it");
                None
            }
        }
    }

    /// `run_turn` phase 3: assemble the codex prompt plus the extra
    /// directories the sandbox may access, and log the turn start.
    async fn assemble_prompt_and_dirs(
        &self,
        message: &IncomingMessage,
        setup: &TurnSetup,
        memory_block: Option<&str>,
    ) -> (String, Vec<PathBuf>) {
        let prompt = build_prompt(
            message,
            &setup.runtime_state.settings,
            &setup.effective_model,
            &setup.workspace_dir,
            &setup.shared_workspace_dir,
            &self.config.general.self_repo_dir,
            memory_block,
        );
        let mut add_dirs = vec![self.session.inbox_dir().to_path_buf()];
        if setup.workspace_dir != setup.shared_workspace_dir {
            add_dirs.push(setup.shared_workspace_dir.clone());
        }
        let layout = DataLayout::new(&self.config.general.data_dir);
        tokio::fs::create_dir_all(layout.cron_jobs_dir()).await.ok();
        add_dirs.extend(layout.turn_add_dirs());
        info!(
            sender_openid = %message.sender_openid,
            message_id = %message.message_id,
            model = %setup.effective_model,
            reasoning = %setup.reasoning.as_str(),
            codex_home = %setup.codex_home.display(),
            workspace_dir = %setup.workspace_dir.display(),
            "starting codex turn"
        );
        (prompt, add_dirs)
    }

    /// `run_turn` phase 4: when an interactive scheduler job is pending for
    /// this user and this turn runs on its thread (or the job has no thread
    /// bound yet), return the job's end signal so streamed replies strip it.
    async fn detect_strip_signal(&self, openid: &str, foreground: &DialogState) -> Option<String> {
        match crate::scheduler::pending_for_owner(&self.config.general.data_dir, openid).await {
            Ok(Some(pending))
                if pending.codex_session_id.is_none()
                    || foreground.session_id == pending.codex_session_id =>
            {
                Some(pending.end_signal)
            }
            Ok(Some(_)) => None,
            Ok(None) => None,
            Err(err) => {
                warn!(
                    sender_openid = %openid,
                    error = %err,
                    "failed to inspect pending interactive scheduler state"
                );
                None
            }
        }
    }

    /// `run_turn` phase 5: spawn the streaming reply emitter that forwards
    /// execution updates to QQ while the turn runs.
    fn spawn_reply_emitter(
        &self,
        message: &IncomingMessage,
        setup: &TurnSetup,
        strip_signal: Option<String>,
        update_rx: mpsc::UnboundedReceiver<ExecutionUpdate>,
    ) -> tokio::task::JoinHandle<(PassiveDispatchReport, Option<anyhow::Error>)> {
        tokio::spawn(
            PassiveTurnEmitter::new(
                self.qq_client.clone(),
                message.sender_openid.clone(),
                message.message_id.clone(),
                setup.workspace_dir.clone(),
                setup.effective_settings.verbose,
            )
            .with_strip_signal(strip_signal)
            .run(update_rx),
        )
    }

    /// `run_turn` success tail: persist the thread binding and usage snapshot,
    /// deliver whatever the streaming emitter did not send, then run the
    /// post-turn hooks (scheduler completion, plan-mode follow-up, shadow
    /// workers, self-update auto-build).
    async fn finish_turn_success(
        &self,
        message: &IncomingMessage,
        setup: &TurnSetup,
        output: ExecutionResult,
        dispatch_report: &PassiveDispatchReport,
    ) -> Result<()> {
        let effective_model = &setup.effective_model;
        let reasoning = setup.reasoning;
        let context_mode = setup.context_mode;
        let workspace_dir = &setup.workspace_dir;
        let effective_settings = &setup.effective_settings;
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
            if let Some(snapshot) = build_usage_snapshot(&info, output.context_window) {
                let _ = self
                    .session
                    .set_foreground_usage(&message.sender_openid, snapshot.clone())
                    .await;
                Some(snapshot)
            } else {
                None
            }
        } else {
            None
        };
        let lang_for_warning = self.command_locale(&message.sender_openid).await;
        let context_warning = usage_snapshot
            .as_ref()
            .and_then(|snap| build_context_warning(snap, &lang_for_warning));
        if !dispatch_report.saw_agent_message {
            let parsed = parse_output(&output.text, workspace_dir);
            let mut payload = parsed.text.clone();
            if let Some(warning) = context_warning.as_deref() {
                if !payload.is_empty() {
                    payload.push_str("\n\n");
                }
                payload.push_str(warning);
            }
            if !payload.is_empty() {
                self.reply_text(&message.sender_openid, &message.message_id, &payload)
                    .await?;
            }
            for directive in parsed.directives {
                self.send_directive(&message.sender_openid, &message.message_id, directive)
                    .await?;
            }
        } else if let Some(warning) = context_warning.as_deref() {
            self.reply_text(&message.sender_openid, &message.message_id, warning)
                .await?;
        }

        if let Err(err) =
            crate::scheduler::on_fg_turn_completed(self, &message.sender_openid, &output.text).await
        {
            warn!(
                sender_openid = %message.sender_openid,
                error = %err,
                "failed to process interactive scheduler completion hook"
            );
        }

        // Plan-mode post-turn: if the planning turn produced a
        // `<proposed_plan>` block, stash it + prompt the user to
        // approve it via `/实施`.
        if effective_settings.plan_mode
            && let Some(plan) = extract_proposed_plan(&output.text)
        {
            let _ = self
                .session
                .update_settings_for_user(&message.sender_openid, |settings| {
                    settings.pending_plan = Some(plan.clone());
                })
                .await;
            let lang = effective_settings.language.as_str();
            let prompt = build_plan_followup_prompt(lang);
            let _ = self
                .reply_text(&message.sender_openid, &message.message_id, &prompt)
                .await;
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
            workspace_dir,
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
            self.reply_text(&message.sender_openid, &message.message_id, &text)
                .await?;
        }
        Ok(())
    }

    /// `run_turn` failure tail: persist the interrupted thread id when it is
    /// safe to, route resume failures into the recovery flow, and report any
    /// other error to the user.
    async fn finish_turn_failure(
        &self,
        message: &IncomingMessage,
        setup: &TurnSetup,
        err: anyhow::Error,
        dispatch_report: &PassiveDispatchReport,
    ) -> Result<()> {
        let user_snapshot = &setup.user_snapshot;
        let effective_model = &setup.effective_model;
        let reasoning = setup.reasoning;
        let context_mode = setup.context_mode;
        let workspace_dir = &setup.workspace_dir;
        // The turn established a thread (id captured via the update
        // stream) but did not complete — /stop or an upstream failure
        // mid-flight. Persist that thread id (with the profile the
        // turn actually ran with) so the next message resumes the same
        // conversation instead of starting fresh and losing all prior
        // context. Guarded: the write only lands while the foreground
        // is still the dialog this turn started on — /stop of an
        // interactive cron task, /new or the expiry sweeper may have
        // swapped the foreground mid-turn, and overwriting then would
        // orphan the conversation the user switched back to.
        // Best-effort: a store failure only logs so the user still
        // gets the turn's error report below. Skipped for
        // resume-recovery errors: there the thread failed to load, so
        // its id is already the (unusable) foreground session and
        // recovery owns the flow.
        if let Some(session_id) = dispatch_report.session_id.clone()
            && !is_resume_recovery_error(&err)
        {
            match self
                .session
                .bind_foreground_session_profile_if_matches(
                    &message.sender_openid,
                    &user_snapshot.foreground,
                    session_id,
                    DialogProfile {
                        model_override: Some(effective_model.clone()),
                        reasoning_effort: Some(reasoning),
                        service_tier: None,
                        context_mode,
                    },
                )
                .await
            {
                Ok(true) => {}
                Ok(false) => info!(
                    sender_openid = %message.sender_openid,
                    "skipped persisting interrupted turn thread: foreground changed mid-turn"
                ),
                Err(store_err) => warn!(
                    sender_openid = %message.sender_openid,
                    error = %store_err,
                    "failed to persist interrupted turn thread id"
                ),
            }
        }
        if err.to_string().contains("aborted by user") {
            info!("codex turn aborted by operator");
            return Ok(());
        }
        if is_resume_recovery_error(&err) {
            warn!("codex resume failed; entering user recovery flow: {err:#}");
            self.pending_resume_messages
                .lock()
                .await
                .insert(message.sender_openid.clone(), message.clone());
            let lang = self.command_locale(&message.sender_openid).await;
            self.session
                .set_pending_setting(&message.sender_openid, Some(PendingSetting::ResumeRecovery))
                .await?;
            self.reply_text(
                &message.sender_openid,
                &message.message_id,
                &t!("commands.resume.recovery_prompt", locale = lang.as_str()),
            )
            .await?;
            return Ok(());
        }
        error!("codex execution failed: {err:#}");
        let text = self
            .format_execution_error_message(&err, workspace_dir)
            .unwrap_or_else(|| format!("Codex 执行失败：{err}"));
        if dispatch_report.sent_replies == 0 {
            self.reply_text(&message.sender_openid, &message.message_id, &text)
                .await?;
        }
        Err(err)
    }

    fn runtime_profile_path(&self) -> PathBuf {
        let codex_home = &self.config.general.codex_home_global;
        codex_home.join("config.toml")
    }

    /// Resolve a user's UI language, falling back to the canonical default when
    /// they have no session record yet. Shared by command handlers and the
    /// scheduler so locale resolution stays consistent in one place.
    pub(crate) async fn command_locale(&self, openid: &str) -> String {
        self.session
            .language_for_user(openid)
            .await
            .unwrap_or_else(crate::session::state::default_language)
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
        // Reserve the busy slot for the whole update: otherwise a message
        // arriving during the multi-minute build would see busy=false, start a
        // real turn, and be silently killed by the exit(0) below. Every
        // early-return below releases the slot by dropping the guard.
        let Some(busy) = self.try_acquire_busy() else {
            self.reply_text(
                openid,
                message_id,
                "当前有任务在运行，请先等待当前任务完成后再执行 `/self-update`。",
            )
            .await?;
            return Ok(());
        };
        let build_result = self_update::ensure_successful_build(&self.config).await?;
        if !build_result.success {
            self.reply_text(openid, message_id, &build_result.summary)
                .await?;
            return Ok(());
        }
        // Smoke-test the freshly built binary before overwriting the running one,
        // so a binary that compiles but panics on startup can't brick the
        // service via an external supervisor's crash loop.
        if let Err(err) = self_update::smoke_test_binary(&build_result.binary_path).await {
            warn!(error = %err, "self-update smoke test failed; aborting update");
            self.reply_text(
                openid,
                message_id,
                &format!("新构建的二进制启动自检失败，已放弃本次更新：{err}"),
            )
            .await?;
            return Ok(());
        }
        let running_binary =
            std::env::current_exe().context("failed to detect current executable")?;
        self_update::replace_binary_for_restart(&build_result.binary_path, &running_binary).await?;
        // Point of no return: the running binary is already replaced, so the
        // busy slot must stay held until the process exits — a message arriving
        // now must not start a turn that exit(0) would kill mid-flight.
        // `process::exit` would skip the guard's Drop anyway, but leak it
        // explicitly so the intent doesn't hinge on that coincidence.
        std::mem::forget(busy);
        // The notification is best-effort: a send failure must not leave us
        // stuck with busy=true and no exit.
        if let Err(err) = self
            .reply_text(
                openid,
                message_id,
                &format!(
                    "已覆盖运行中的二进制：`{}`\n即将退出当前进程（已通知 codex app-server 关闭）。若已配置外部守护服务，将自动重启；否则请手动重新启动。",
                    running_binary.display()
                ),
            )
            .await
        {
            warn!(error = %err, "failed to send self-update completion notice; exiting anyway");
        }
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
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<()> {
        let Some(_busy) = self.try_acquire_busy() else {
            let lang = self.command_locale(openid).await;
            self.reply_text(
                openid,
                message_id,
                &t!("commands.compact.busy", locale = lang.as_str()),
            )
            .await?;
            return Ok(());
        };

        self.handle_compact_command_inner(openid, message_id, runtime_profile)
            .await
        // `_busy` drops here, releasing the busy slot.
    }

    async fn handle_compact_command_inner(
        &self,
        openid: &str,
        message_id: &str,
        runtime_profile: &CodexRuntimeProfile,
    ) -> Result<()> {
        let user_snapshot = self.session.snapshot_for_user(openid).await?;
        let lang = self.command_locale(openid).await;
        let locale = lang.as_str();
        let Some(session_id) = user_snapshot.foreground.session_id.clone() else {
            self.reply_text(
                openid,
                message_id,
                &t!("commands.compact.missing_session", locale = locale),
            )
            .await?;
            return Ok(());
        };
        self.reply_text(
            openid,
            message_id,
            &t!("commands.compact.start", locale = locale),
        )
        .await?;

        let effective_settings = user_snapshot.effective_settings();
        let (effective_model, reasoning, context_mode) =
            self.effective_runtime_triple(&effective_settings, runtime_profile);
        let request = CompactRequest {
            session_id: session_id.clone(),
            workspace_dir: user_snapshot.foreground.workspace_dir.clone(),
            config_overrides: Vec::new(),
            add_dirs: self.compact_add_dirs(&user_snapshot.foreground.workspace_dir),
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
                self.reply_text(openid, message_id, &text).await?;
            }
            Err(err) => {
                if err.to_string().contains("aborted by user") {
                    return Ok(());
                }
                self.reply_text(
                    openid,
                    message_id,
                    &format!(
                        "{}: {err:#}",
                        t!("commands.compact.failed", locale = locale)
                    ),
                )
                .await?;
            }
        }

        Ok(())
    }

    /// Reply to `message_id` from `openid` with `text`, quoting the original
    /// message — the reply shape every user-facing message in this file uses.
    async fn reply_text(&self, openid: &str, message_id: &str, text: &str) -> Result<()> {
        self.qq_client
            .send_text(openid, message_id, text, Some(message_id))
            .await
    }

    /// Resolve the (model, reasoning effort, context mode) triple a codex call
    /// runs with: per-dialog effective settings first, then the global runtime
    /// profile, then the config defaults. Shared by `run_turn` and `/compact`.
    fn effective_runtime_triple(
        &self,
        effective_settings: &SessionSettings,
        runtime_profile: &CodexRuntimeProfile,
    ) -> (String, ReasoningEffort, Option<ContextMode>) {
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
        (effective_model, reasoning, context_mode)
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

    fn compact_add_dirs(&self, workspace_dir: &PathBuf) -> Vec<PathBuf> {
        let mut add_dirs = vec![self.session.inbox_dir().to_path_buf()];
        let shared_workspace_dir = self.session.attachment_workspace_dir().to_path_buf();
        if workspace_dir != &shared_workspace_dir {
            add_dirs.push(shared_workspace_dir);
        }
        add_dirs
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

/// Join the reply emitter task and fold its outcome into the turn's dispatch
/// report.
///
/// Do NOT hard-fail the turn on a streaming-send error: if execution succeeded
/// we still need to persist the session id and run the post-turn hooks.
/// Resetting the delivery counters (saw_agent_message = false) also makes the
/// success branch re-send the reply as a whole message, recovering from a
/// transient streamed-send failure — while the captured thread id is kept so
/// an interrupted turn can still be persisted.
async fn await_dispatch_report(
    emitter: tokio::task::JoinHandle<(PassiveDispatchReport, Option<anyhow::Error>)>,
    message_id: &str,
) -> PassiveDispatchReport {
    match emitter.await {
        Ok((report, None)) => report,
        Ok((report, Some(err))) => {
            warn!(
                error = %err,
                message_id = %message_id,
                "failed to stream reply to QQ; continuing to persist turn state"
            );
            PassiveDispatchReport {
                session_id: report.session_id,
                ..PassiveDispatchReport::default()
            }
        }
        Err(err) => {
            warn!(error = %err, "reply emitter task panicked; continuing to persist turn state");
            PassiveDispatchReport::default()
        }
    }
}

fn global_context_label(context_mode: Option<ContextMode>) -> &'static str {
    match context_mode {
        Some(mode) => mode.label(),
        None => "inherit",
    }
}

fn is_resume_recovery_error(err: &anyhow::Error) -> bool {
    format!("{err:#}").contains("codex resume failed for thread")
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

fn format_command_approval(event: &CommandApprovalEvent) -> String {
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

async fn write_turn_context(workspace_dir: &std::path::Path, openid: &str) {
    let path = workspace_dir.join(".claw-turn.json");
    let body = serde_json::json!({
        "owner_openid": openid,
        "openid": openid,
    });
    if let Err(err) = tokio::fs::write(
        &path,
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".to_string()),
    )
    .await
    {
        warn!(error = %err, path = %path.display(), "failed to write turn context");
    }
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
pub(crate) fn extract_proposed_plan(text: &str) -> Option<String> {
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
pub(crate) fn build_plan_followup_prompt(lang: &str) -> String {
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

fn build_usage_snapshot(
    info: &TokenUsageInfo,
    context_window: Option<u64>,
) -> Option<TokenUsageSnapshot> {
    let window = info.model_context_window.or(context_window)?;
    let context_usage = info.context_window_usage().clone();
    Some(TokenUsageSnapshot {
        total_tokens: context_usage.tokens_in_context_window(),
        window,
        input_tokens: context_usage.input_tokens,
        cached_input_tokens: context_usage.cached_input_tokens,
        output_tokens: context_usage.output_tokens,
        updated_at: chrono::Utc::now(),
    })
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

#[cfg(test)]
mod tests {
    use crate::codex::TokenUsageInfo;
    use crate::codex::events::TokenUsage;
    use crate::qq::{MSG_TYPE_QUOTE, MessageAttachment, MsgElement};
    use crate::session::state::fixtures::{legacy_cumulative_usage, usage};

    use super::{
        build_context_warning, build_usage_snapshot, extract_quote, sanitize_attachment_filename,
    };

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

    #[test]
    fn context_warning_is_localized_per_language() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            (
                "en",
                &["80% used", "220K used / 272K", "`/compact`"],
                &["`/压缩`"],
            ),
            ("zh", &["`/压缩`"], &["`/compact`"]),
        ];
        for (lang, expected, forbidden) in cases {
            let warning = build_context_warning(&usage(220_000, 272_000), lang)
                .unwrap_or_else(|| panic!("case: {lang} produced no warning"));
            for needle in *expected {
                assert!(
                    warning.contains(needle),
                    "case: {lang} missing {needle:?} in {warning}"
                );
            }
            for needle in *forbidden {
                assert!(
                    !warning.contains(needle),
                    "case: {lang} unexpectedly contains {needle:?} in {warning}"
                );
            }
        }
    }

    #[test]
    fn usage_snapshot_requires_context_window() {
        let info = TokenUsageInfo {
            total_token_usage: TokenUsage {
                input_tokens: 100,
                cached_input_tokens: 0,
                output_tokens: 50,
                reasoning_output_tokens: 0,
                total_tokens: 150,
            },
            last_token_usage: TokenUsage {
                input_tokens: 80,
                cached_input_tokens: 0,
                output_tokens: 20,
                reasoning_output_tokens: 0,
                total_tokens: 100,
            },
            model_context_window: None,
        };

        assert!(build_usage_snapshot(&info, None).is_none());

        let snapshot = build_usage_snapshot(&info, Some(272_000)).expect("snapshot");
        assert_eq!(snapshot.window, 272_000);
        assert_eq!(snapshot.total_tokens, 100);
    }

    #[test]
    fn context_warning_skips_implausible_legacy_cumulative_usage() {
        let warning = build_context_warning(&legacy_cumulative_usage(), "zh");
        assert!(warning.is_none());
    }
}
