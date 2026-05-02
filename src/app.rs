use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{error, info, warn};

use rust_i18n::t;

use crate::{
    codex::{
        compact,
        executor::{CodexExecutor, ExecutionRequest, ExecutionResult},
        output::{Directive, parse_output},
        prompt::build_prompt,
        runtime::read_codex_runtime_profile_from_path,
    },
    commands::{CommandOutcome, CommandReply, maybe_handle_command},
    config::AppConfig,
    message::{IncomingAttachment, IncomingMessage, QuotedMessage},
    normalize_lang,
    qq::{
        api::QqApiClient,
        passive::PassiveTurnEmitter,
        types::{C2CMessageEvent, MSG_TYPE_QUOTE, MessageAttachment, MsgElement},
    },
    self_update,
    session::{
        state::{ContextMode, DialogProfile, SessionState, TokenUsageSnapshot},
        store::SessionStore,
    },
};

const CONTEXT_WARNING_THRESHOLD: f64 = 0.80;
const SELF_REPO_BUILD_RETRY_LIMIT: usize = 1;

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

        let result = self.run_turn(normalized, runtime_profile).await;
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
        let effective_settings = user_snapshot
            .settings
            .merged_with_profile(user_snapshot.foreground.profile.as_ref());
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
        let service_tier = effective_settings
            .service_tier
            .or(runtime_profile.service_tier);
        let context_mode = effective_settings
            .context_mode
            .or(runtime_profile.context_mode);
        let prompt = build_prompt(
            &message,
            &runtime_state.settings,
            &effective_model,
            &workspace_dir,
            &shared_workspace_dir,
            &self.config.general.self_repo_dir,
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
                    codex_home: codex_home.clone(),
                    config_overrides: Vec::new(),
                    add_dirs: add_dirs.clone(),
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

                if self_update::changed_self_repo(
                    &workspace_dir,
                    &output.changed_files,
                    &self.config.general.self_repo_dir,
                ) {
                    self.handle_self_repo_change_after_turn(
                        &message,
                        &workspace_dir,
                        &codex_home,
                        &add_dirs,
                        &effective_model,
                        reasoning,
                        service_tier,
                        context_mode,
                        output.session_id.clone(),
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
                    "已覆盖运行中的二进制：`{}`\n即将退出当前进程。若已配置外部守护服务，将自动重启；否则请手动重新启动。",
                    running_binary.display()
                ),
                Some(message_id),
            )
            .await?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::process::exit(0);
        #[allow(unreachable_code)]
        Ok(())
    }

    async fn handle_self_repo_change_after_turn(
        &self,
        message: &IncomingMessage,
        workspace_dir: &Path,
        codex_home: &Path,
        add_dirs: &[PathBuf],
        effective_model: &str,
        reasoning: crate::session::state::ReasoningEffort,
        service_tier: Option<crate::session::state::ServiceTier>,
        context_mode: Option<ContextMode>,
        session_id: Option<String>,
    ) -> Result<()> {
        let build_result = match self_update::run_build(&self.config).await {
            Ok(result) => result,
            Err(err) => {
                self.qq_client
                    .send_text(
                        &message.sender_openid,
                        &message.message_id,
                        &format!(
                            "检测到修改了 codex-claw 源码，但阶段 1 自动构建触发失败：{err}\n当前未部署新版本。"
                        ),
                        Some(&message.message_id),
                    )
                    .await?;
                return Ok(());
            }
        };

        if build_result.success {
            self.qq_client
                .send_text(
                    &message.sender_openid,
                    &message.message_id,
                    &format_self_repo_build_success_message(&build_result.summary),
                    Some(&message.message_id),
                )
                .await?;
            return Ok(());
        }

        self.qq_client
            .send_text(
                &message.sender_openid,
                &message.message_id,
                &format_self_repo_build_retry_start_message(&build_result.summary),
                Some(&message.message_id),
            )
            .await?;

        let retry_text = self
            .run_self_repo_build_retry(
                message,
                workspace_dir,
                codex_home,
                add_dirs,
                effective_model,
                reasoning,
                service_tier,
                context_mode,
                session_id,
                &build_result.summary,
            )
            .await;

        let text = match retry_text {
            Ok(text) => text,
            Err(err) => format!(
                "自动重试流程执行失败：{err}\n\n最近一次构建结果：\n{}\n\n当前仍停留在阶段 1，未部署新版本。",
                build_result.summary
            ),
        };
        self.qq_client
            .send_text(
                &message.sender_openid,
                &message.message_id,
                &text,
                Some(&message.message_id),
            )
            .await?;
        Ok(())
    }

    async fn run_self_repo_build_retry(
        &self,
        message: &IncomingMessage,
        workspace_dir: &Path,
        codex_home: &Path,
        add_dirs: &[PathBuf],
        effective_model: &str,
        reasoning: crate::session::state::ReasoningEffort,
        service_tier: Option<crate::session::state::ServiceTier>,
        context_mode: Option<ContextMode>,
        session_id: Option<String>,
        build_summary: &str,
    ) -> Result<String> {
        let retry_prompt = format!(
            "You just edited the codex-claw repository and the automatic stage-1 build failed.\n\
Perform at most one focused repair pass for the build failure only.\n\
Requirements:\n\
1. Make the minimal code or config changes needed so `cargo build --release` succeeds.\n\
2. Do not add unrelated features or refactors.\n\
3. Do not ask the user for confirmation.\n\
4. Do not run `/self-update`; deployment is handled outside this turn.\n\
5. After making the fix, stop.\n\n\
Latest build result:\n{build_summary}"
        );

        let mut latest_session_id = session_id;
        let mut retry_execution: Option<ExecutionResult> = None;

        for _ in 0..SELF_REPO_BUILD_RETRY_LIMIT {
            let execution = self
                .codex
                .execute(
                    ExecutionRequest {
                        prompt: retry_prompt.clone(),
                        workspace_dir: workspace_dir.to_path_buf(),
                        codex_home: codex_home.to_path_buf(),
                        config_overrides: Vec::new(),
                        add_dirs: add_dirs.to_vec(),
                        session_state: SessionState {
                            session_id: latest_session_id.clone(),
                            settings: Default::default(),
                        },
                        model: Some(effective_model.to_string()),
                        service_tier,
                        context_mode,
                        reasoning_effort: reasoning,
                        image_paths: Vec::new(),
                    },
                    Some(self.install_active_turn().await),
                    None,
                )
                .await;
            self.clear_active_turn().await;
            let output = execution?;
            latest_session_id = output.session_id.clone();
            retry_execution = Some(output);
        }

        if let Some(session_id) = latest_session_id {
            self.session
                .bind_foreground_session_profile(
                    &message.sender_openid,
                    Some(session_id),
                    DialogProfile {
                        model_override: Some(effective_model.to_string()),
                        reasoning_effort: Some(reasoning),
                        service_tier,
                        context_mode,
                    },
                )
                .await?;
        }

        let final_build = self_update::run_build(&self.config).await?;
        let retry_note = retry_execution
            .as_ref()
            .map(|output| parse_output(&output.text, workspace_dir).text.trim().to_string())
            .filter(|text| !text.is_empty());

        if final_build.success {
            let mut text = format_self_repo_retry_success_message(&final_build.summary);
            if let Some(note) = retry_note {
                text.push_str("\n\n自动重试说明：\n");
                text.push_str(&note);
            }
            return Ok(text);
        }

        let mut text = format_self_repo_retry_failure_message(&final_build.summary);
        if let Some(note) = retry_note {
            text.push_str("\n\n自动重试说明：\n");
            text.push_str(&note);
        }
        Ok(text)
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
        let effective_settings = user_snapshot
            .settings
            .merged_with_profile(user_snapshot.foreground.profile.as_ref());
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
        if self
            .session
            .rollout_path_for_session(&session_id)?
            .is_none()
        {
            self.qq_client
                .send_text(
                    openid,
                    message_id,
                    &t!("commands.compact.missing_rollout", locale = locale),
                    Some(message_id),
                )
                .await?;
            return Ok(());
        }

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
        let service_tier = effective_settings
            .service_tier
            .or(runtime_profile.service_tier);
        let context_mode = effective_settings
            .context_mode
            .or(runtime_profile.context_mode);
        let runtime_state = SessionState {
            session_id: Some(session_id.clone()),
            settings: effective_settings,
        };
        let mut add_dirs = vec![self.session.inbox_dir().to_path_buf()];
        if workspace_dir != shared_workspace_dir {
            add_dirs.push(shared_workspace_dir);
        }
        self.qq_client
            .send_text(
                openid,
                message_id,
                &t!("commands.compact.start", locale = locale),
                Some(message_id),
            )
            .await?;

        match self
            .run_session_compaction(
                openid,
                &session_id,
                &workspace_dir,
                &effective_model,
                reasoning,
                service_tier,
                context_mode,
                &codex_home,
                &add_dirs,
                runtime_state,
            )
            .await
        {
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
                        &format!("{}: {err}", t!("commands.compact.failed", locale = locale)),
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
    async fn run_session_compaction(
        &self,
        openid: &str,
        session_id: &str,
        workspace_dir: &std::path::Path,
        effective_model: &str,
        reasoning: crate::session::state::ReasoningEffort,
        service_tier: Option<crate::session::state::ServiceTier>,
        context_mode: Option<crate::session::state::ContextMode>,
        codex_home: &std::path::Path,
        add_dirs: &[PathBuf],
        runtime_state: SessionState,
    ) -> Result<()> {
        let Some(rollout_path) = self.session.rollout_path_for_session(session_id)? else {
            anyhow::bail!("session rollout file not found");
        };
        let rollout_snapshot = compact::read_rollout_snapshot(&rollout_path)?;
        info!(
            openid = %openid,
            session_id,
            rollout = %rollout_path.display(),
            "triggering codex session compaction"
        );

        let compaction = self
            .codex
            .execute(
                ExecutionRequest {
                    prompt: compact::SUMMARIZATION_PROMPT.to_string(),
                    workspace_dir: workspace_dir.to_path_buf(),
                    codex_home: codex_home.to_path_buf(),
                    config_overrides: Vec::new(),
                    add_dirs: add_dirs.to_vec(),
                    session_state: runtime_state,
                    model: Some(effective_model.to_string()),
                    service_tier,
                    context_mode,
                    reasoning_effort: reasoning,
                    image_paths: Vec::new(),
                },
                Some(self.install_active_turn().await),
                None,
            )
            .await;
        self.clear_active_turn().await;
        let output = compaction?;
        let parsed = parse_output(&output.text, workspace_dir);
        let summary_text = compact::build_summary_text(&parsed.text);
        let replacement_history =
            compact::build_compacted_history(&rollout_snapshot.user_messages, &summary_text);
        compact::append_compacted_rollout(&rollout_path, &summary_text, &replacement_history)?;
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

fn format_self_repo_build_success_message(summary: &str) -> String {
    format!(
        "检测到修改了 codex-claw 源码。\n阶段 1 自动构建成功：\n{summary}\n\n当前服务仍在运行旧二进制；要让修改生效，请继续发送 `/self-update` 完成阶段 2 部署。"
    )
}

fn format_self_repo_build_retry_start_message(summary: &str) -> String {
    format!(
        "检测到修改了 codex-claw 源码，但阶段 1 自动构建失败。\n下面开始一次仅针对构建错误的自动重试；本次不会自动部署。\n\n首次构建结果：\n{summary}"
    )
}

fn format_self_repo_retry_success_message(summary: &str) -> String {
    format!(
        "自动重试后，阶段 1 构建成功：\n{summary}\n\n当前服务仍在运行旧二进制；要让修改生效，请继续发送 `/self-update` 完成阶段 2 部署。"
    )
}

fn format_self_repo_retry_failure_message(summary: &str) -> String {
    format!(
        "自动重试已执行 1 次，但阶段 1 构建仍然失败。\n最近一次构建结果：\n{summary}\n\n当前未部署新版本。"
    )
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

    use super::{
        build_context_warning, extract_quote, format_self_repo_build_retry_start_message,
        format_self_repo_build_success_message, format_self_repo_retry_failure_message,
        format_self_repo_retry_success_message,
    };

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

    #[test]
    fn self_repo_success_message_mentions_two_stage_flow() {
        let text = format_self_repo_build_success_message("构建成功：`/tmp/codex-claw`");
        assert!(text.contains("阶段 1 自动构建成功"));
        assert!(text.contains("仍在运行旧二进制"));
        assert!(text.contains("`/self-update`"));
    }

    #[test]
    fn self_repo_retry_start_message_does_not_claim_completion() {
        let text = format_self_repo_build_retry_start_message("stderr:\nerror: boom");
        assert!(text.contains("阶段 1 自动构建失败"));
        assert!(text.contains("自动重试"));
        assert!(!text.contains("构建成功"));
    }

    #[test]
    fn self_repo_retry_messages_distinguish_success_and_failure() {
        let success = format_self_repo_retry_success_message("构建成功：`/tmp/codex-claw`");
        assert!(success.contains("自动重试后，阶段 1 构建成功"));
        assert!(success.contains("`/self-update`"));

        let failure = format_self_repo_retry_failure_message("stderr:\nerror: still broken");
        assert!(failure.contains("自动重试已执行 1 次"));
        assert!(failure.contains("当前未部署新版本"));
    }
}
