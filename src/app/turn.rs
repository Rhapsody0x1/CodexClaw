//! `run_turn` and its phases, plus the other busy-slot flows (`/compact`,
//! `/self-update`) that execute a codex call end to end.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use rust_i18n::t;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::{
    codex::{
        CodexRuntimeProfile, CompactRequest, ExecutionRequest, ExecutionResult, ExecutionUpdate,
        build_prompt,
    },
    memory::inject as memory_inject,
    message::IncomingMessage,
    qq::{PassiveDispatchReport, PassiveTurnEmitter, parse_output},
    self_update,
    session::state::{
        ContextMode, DialogProfile, DialogState, PendingSetting, ReasoningEffort, ServiceTier,
        SessionSettings, SessionState, UserSessionState,
    },
    shadow::ShadowContext,
    util::layout::DataLayout,
};

use super::{
    App,
    format::{
        build_context_warning, build_plan_followup_prompt, build_usage_snapshot,
        extract_proposed_plan,
    },
};

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
    pub(super) async fn run_turn(
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

        if let Err(err) = crate::scheduler::on_fg_turn_completed(
            &self.scheduler_ctx,
            &message.sender_openid,
            &output.text,
        )
        .await
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

    pub(super) async fn handle_self_update_command(
        &self,
        openid: &str,
        message_id: &str,
    ) -> Result<()> {
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
            // Release the slot before the (network) reply, matching the
            // pre-guard ordering: a message arriving mid-send should start a
            // turn, not be bounced with "busy".
            drop(busy);
            self.reply_text(openid, message_id, &build_result.summary)
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

    pub(super) async fn handle_compact_command(
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

fn is_resume_recovery_error(err: &anyhow::Error) -> bool {
    format!("{err:#}").contains("codex resume failed for thread")
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
