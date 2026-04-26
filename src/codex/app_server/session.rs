//! Runs a single Codex turn over a shared `AppServerSupervisor`.
//!
//! Drives `thread/start` or `thread/resume` + `turn/start`, then consumes
//! notifications until `turn/completed` (or failure). Translates each
//! notification into the existing [`ExecutionUpdate`] stream consumed by
//! `PassiveTurnEmitter` so the QQ output is byte-identical to the previous
//! `codex exec --json` pipeline.

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use serde_json::Value as JsonValue;
use tokio::{
    sync::{Mutex, broadcast, mpsc, oneshot},
    time::{sleep, timeout},
};
use tracing::{debug, info, warn};

use crate::{
    codex::{
        events::{TokenUsage, TokenUsageInfo},
        executor::{CompactRequest, ExecutionRequest, ExecutionResult, ExecutionUpdate},
    },
    session::state::{ContextMode, ServiceTier},
};

use super::{
    client::{JsonRpcClient, Notification},
    events::{self as translator, TurnOutcome, TurnState},
    protocol::{
        ApprovalPolicy, CollaborationMode, CollaborationSettings, CompactedNotification,
        ItemNotification, ModeKind, ModelReroutedNotification, ReadOnlyAccess, SandboxPolicy,
        ThreadCompactStartParams, ThreadCompactStartResponse, ThreadResumeParams,
        ThreadResumeResponse, ThreadStartParams, ThreadStartResponse, ThreadUnsubscribeParams,
        ThreadUnsubscribeResponse, ThreadUnsubscribeStatus, TokenUsageUpdatedNotification,
        TurnCompletedNotification, TurnInputItem, TurnInterruptParams, TurnInterruptResponse,
        TurnPlanUpdatedNotification, TurnStartParams, TurnStartResponse, method,
    },
    supervisor::AppServerSupervisor,
};

const OUTPUT_IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const INTERRUPT_WAIT: Duration = Duration::from_secs(10);
const COMPACT_MAX_ATTEMPTS: usize = 2;
const COMPACT_RETRY_DELAY: Duration = Duration::from_secs(2);
const THREAD_UNLOAD_TIMEOUT: Duration = Duration::from_secs(12);

/// The subset of Codex config that is fixed when a thread is loaded.
///
/// `turn/start` can override some per-turn fields, but app-server keeps a
/// running thread's config snapshot for values like `model_context_window`.
/// Track what we last loaded so a session-level `/model`, `/reasoning`, or
/// `/context` change can force a reload before the next turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfigSignature {
    model: Option<String>,
    reasoning_effort: String,
    context_mode: Option<ContextMode>,
    service_tier: Option<ServiceTier>,
    cwd: PathBuf,
}

impl RuntimeConfigSignature {
    fn from_request(req: &ExecutionRequest) -> Self {
        Self {
            model: req.model.clone(),
            reasoning_effort: req.reasoning_effort.as_str().to_string(),
            context_mode: req.context_mode,
            service_tier: req.service_tier,
            cwd: req.workspace_dir.clone(),
        }
    }

    fn from_compact_request(req: &CompactRequest) -> Self {
        Self {
            model: req.model.clone(),
            reasoning_effort: req.reasoning_effort.as_str().to_string(),
            context_mode: req.context_mode,
            service_tier: req.service_tier,
            cwd: req.workspace_dir.clone(),
        }
    }
}

/// How the turn should classify sandbox/approval for Codex.
///
/// For both fields, `None` means "defer to the server's config.toml default"
/// (e.g. `sandbox_mode`, `approval_policy`, and the `sandbox_workspace_write.*`
/// knobs in `~/.codex-claw/.codex/config.toml`). `Some(...)` explicitly
/// overrides the config for that turn.
#[derive(Debug, Clone, Default)]
pub struct TurnPolicy {
    pub approval_policy: Option<ApprovalPolicy>,
    pub sandbox_policy: Option<SandboxPolicy>,
    pub plan_mode: bool,
}

impl TurnPolicy {
    /// Inherit approval + sandbox from `config.toml`. Used as the default
    /// when the user hasn't opted into a specific mode.
    pub fn inherit_from_config() -> Self {
        Self::default()
    }

    /// Plan mode: read-only sandbox + no approvals (the agent only observes),
    /// with `collaboration_mode = Plan` applied at turn/start.
    pub fn plan_mode() -> Self {
        Self {
            approval_policy: Some(ApprovalPolicy::Never),
            sandbox_policy: Some(SandboxPolicy::ReadOnly {
                access: Some(ReadOnlyAccess::FullAccess),
            }),
            plan_mode: true,
        }
    }

    /// Explicit approval policy override without touching sandbox.
    pub fn with_approval_policy(policy: ApprovalPolicy) -> Self {
        Self {
            approval_policy: Some(policy),
            sandbox_policy: None,
            plan_mode: false,
        }
    }
}

pub struct AppServerSession {
    supervisor: Arc<AppServerSupervisor>,
    runtime_configs: Arc<Mutex<HashMap<String, RuntimeConfigSignature>>>,
}

impl AppServerSession {
    pub fn new(
        supervisor: Arc<AppServerSupervisor>,
        runtime_configs: Arc<Mutex<HashMap<String, RuntimeConfigSignature>>>,
    ) -> Self {
        Self {
            supervisor,
            runtime_configs,
        }
    }

    /// Execute one turn against the app-server. Preserves the public contract
    /// of the legacy `CodexExecutor::execute` so callers don't change.
    pub async fn execute(
        &self,
        request: ExecutionRequest,
        policy: TurnPolicy,
        cancel_rx: Option<oneshot::Receiver<()>>,
        update_tx: Option<mpsc::UnboundedSender<ExecutionUpdate>>,
    ) -> Result<ExecutionResult> {
        let client = self.supervisor.client().await?;
        let notifications = self.supervisor.subscribe_notifications();

        let thread_id = self
            .ensure_thread(&client, &request, &policy)
            .await
            .context("establish thread")?;

        let effort = Some(request.reasoning_effort.as_str().to_string());
        let service_tier_wire = request.service_tier.map(service_tier_to_wire);

        // Plan mode needs a concrete model on the CollaborationMode, so fall
        // back to whatever the caller is already using for this turn.
        let collab = if policy.plan_mode {
            request.model.clone().map(|model| CollaborationMode {
                mode: ModeKind::Plan,
                settings: CollaborationSettings {
                    model,
                    reasoning_effort: effort.clone(),
                    developer_instructions: None,
                },
            })
        } else {
            None
        };

        let turn_params = TurnStartParams {
            thread_id: thread_id.clone(),
            input: build_turn_input(&request),
            approval_policy: policy.approval_policy,
            sandbox_policy: policy.sandbox_policy.clone(),
            model: request.model.clone(),
            effort,
            service_tier: service_tier_wire,
            collaboration_mode: collab,
        };
        info!(
            thread_id = %thread_id,
            approval_policy = ?policy.approval_policy,
            sandbox_override = policy.sandbox_policy.is_some(),
            plan_mode = policy.plan_mode,
            "sending turn/start"
        );

        let turn_resp: TurnStartResponse = client
            .request("turn/start", &turn_params)
            .await
            .context("turn/start")?;
        let turn_id = turn_resp.turn.id.clone();
        debug!(thread_id = %thread_id, turn_id = %turn_id, "turn started");

        let mut runner = TurnRunner {
            client: client.clone(),
            notifications,
            thread_id: thread_id.clone(),
            turn_id: turn_id.clone(),
            state: TurnState::default(),
            update_tx,
            cancel_rx,
            cancel_requested: false,
        };
        let outcome = runner.drive().await?;

        let ExecutionRequestContext { context_window, .. } =
            ExecutionRequestContext::from_request(&request);
        let token_usage_info = runner.build_token_usage_info(context_window);
        let text = runner
            .state
            .agent_text_parts
            .join("\n\n")
            .trim()
            .to_string();

        match outcome {
            TurnOutcome::Completed => Ok(ExecutionResult {
                session_id: Some(thread_id),
                text,
                changed_files: runner.state.changed_files.clone(),
                token_usage_info: token_usage_info.clone(),
                context_window: token_usage_info
                    .as_ref()
                    .and_then(|info| info.model_context_window)
                    .or(context_window),
            }),
            TurnOutcome::Interrupted => Err(anyhow!("codex turn aborted by user")),
            TurnOutcome::Failed(msg) => Err(anyhow!("codex turn failed: {msg}")),
        }
    }

    pub async fn compact_thread(
        &self,
        request: CompactRequest,
        cancel_rx: Option<oneshot::Receiver<()>>,
    ) -> Result<()> {
        let client = self.supervisor.client().await?;
        let desired_config = RuntimeConfigSignature::from_compact_request(&request);
        let mut cancel_rx = cancel_rx;
        for attempt in 1..=COMPACT_MAX_ATTEMPTS {
            let thread_id = self
                .reload_thread_for_compact(&client, &request, &desired_config)
                .await
                .context("thread/reload before compact")?;
            let params = ThreadCompactStartParams {
                thread_id: thread_id.clone(),
            };
            let notifications = self.supervisor.subscribe_notifications();
            info!(
                thread_id = %thread_id,
                attempt,
                max_attempts = COMPACT_MAX_ATTEMPTS,
                "sending thread/compact/start"
            );
            client
                .request::<_, ThreadCompactStartResponse>(method::THREAD_COMPACT_START, &params)
                .await
                .context("thread/compact/start")?;
            match wait_for_compaction(&thread_id, notifications, &mut cancel_rx).await {
                Ok(()) => return Ok(()),
                Err(err) if attempt < COMPACT_MAX_ATTEMPTS && is_retryable_compact_error(&err) => {
                    warn!(
                        thread_id = %thread_id,
                        attempt,
                        error = %err,
                        "codex compact failed with retryable stream error; retrying"
                    );
                    sleep(COMPACT_RETRY_DELAY).await;
                }
                Err(err) => return Err(err),
            }
        }
        unreachable!("compact retry loop should return before exhausting attempts")
    }

    async fn reload_thread_for_compact(
        &self,
        client: &Arc<JsonRpcClient>,
        request: &CompactRequest,
        desired_config: &RuntimeConfigSignature,
    ) -> Result<String> {
        self.runtime_configs
            .lock()
            .await
            .remove(&request.session_id);
        if let Err(err) = self
            .unload_thread(client, request.session_id.as_str())
            .await
        {
            warn!(
                thread_id = %request.session_id,
                error = %err,
                "failed to unload app-server thread before compact resume"
            );
        }
        let resp: ThreadResumeResponse = client
            .request::<ThreadResumeParams, ThreadResumeResponse>(
                "thread/resume",
                &ThreadResumeParams {
                    thread_id: request.session_id.clone(),
                    model: request.model.clone(),
                    cwd: Some(request.workspace_dir.to_string_lossy().into_owned()),
                    approval_policy: None,
                    sandbox_policy: None,
                    service_tier: request.service_tier.map(service_tier_to_wire),
                    config: build_compact_config_overrides(request),
                },
            )
            .await
            .context("thread/resume before compact")?;
        self.runtime_configs
            .lock()
            .await
            .insert(resp.thread.id.clone(), desired_config.clone());
        Ok(resp.thread.id)
    }

    async fn ensure_thread(
        &self,
        client: &Arc<JsonRpcClient>,
        request: &ExecutionRequest,
        policy: &TurnPolicy,
    ) -> Result<String> {
        let desired_config = RuntimeConfigSignature::from_request(request);
        if let Some(existing) = request.session_state.session_id.clone() {
            if self
                .needs_runtime_reload(existing.as_str(), &desired_config)
                .await
            {
                info!(
                    thread_id = %existing,
                    "app-server runtime config changed; unloading thread before resume"
                );
                self.runtime_configs.lock().await.remove(&existing);
                if let Err(err) = self.unload_thread(client, existing.as_str()).await {
                    warn!(
                        thread_id = %existing,
                        error = %err,
                        "failed to unload stale app-server thread before resume"
                    );
                }
            }
            match client
                .request::<ThreadResumeParams, ThreadResumeResponse>(
                    "thread/resume",
                    &ThreadResumeParams {
                        thread_id: existing.clone(),
                        model: request.model.clone(),
                        cwd: Some(request.workspace_dir.to_string_lossy().into_owned()),
                        approval_policy: policy.approval_policy,
                        sandbox_policy: policy.sandbox_policy.clone(),
                        service_tier: request.service_tier.map(service_tier_to_wire),
                        config: build_config_overrides(request),
                    },
                )
                .await
            {
                Ok(resp) => {
                    self.runtime_configs
                        .lock()
                        .await
                        .insert(resp.thread.id.clone(), desired_config);
                    return Ok(resp.thread.id);
                }
                Err(err) => {
                    warn!(
                        thread_id = %existing,
                        error = %err,
                        "thread/resume failed; starting a fresh thread"
                    );
                }
            }
        }
        let start = ThreadStartParams {
            model: request.model.clone(),
            cwd: Some(request.workspace_dir.to_string_lossy().into_owned()),
            approval_policy: policy.approval_policy,
            sandbox_policy: policy.sandbox_policy.clone(),
            service_tier: request.service_tier.map(service_tier_to_wire),
            config: build_config_overrides(request),
            add_dirs: request
                .add_dirs
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        };
        info!(
            approval_policy = ?start.approval_policy,
            sandbox_override = policy.sandbox_policy.is_some(),
            plan_mode = policy.plan_mode,
            "sending thread/start"
        );
        let resp: ThreadStartResponse = client
            .request("thread/start", &start)
            .await
            .context("thread/start")?;
        self.runtime_configs
            .lock()
            .await
            .insert(resp.thread.id.clone(), desired_config);
        Ok(resp.thread.id)
    }

    async fn needs_runtime_reload(
        &self,
        thread_id: &str,
        desired_config: &RuntimeConfigSignature,
    ) -> bool {
        self.runtime_configs
            .lock()
            .await
            .get(thread_id)
            .is_some_and(|applied| applied != desired_config)
    }

    async fn unload_thread(&self, client: &Arc<JsonRpcClient>, thread_id: &str) -> Result<()> {
        let mut notifications = self.supervisor.subscribe_notifications();
        let resp: ThreadUnsubscribeResponse = client
            .request(
                "thread/unsubscribe",
                &ThreadUnsubscribeParams {
                    thread_id: thread_id.to_string(),
                },
            )
            .await
            .context("thread/unsubscribe")?;
        match resp.status {
            ThreadUnsubscribeStatus::Unsubscribed => {
                wait_for_thread_closed(thread_id, &mut notifications).await;
            }
            ThreadUnsubscribeStatus::NotLoaded => {
                debug!(thread_id, "thread already not loaded before runtime reload");
            }
            ThreadUnsubscribeStatus::NotSubscribed => {
                warn!(
                    thread_id,
                    "thread was loaded but this app-server connection was not subscribed"
                );
            }
        }
        Ok(())
    }
}

async fn wait_for_thread_closed(
    thread_id: &str,
    notifications: &mut broadcast::Receiver<Notification>,
) {
    let wait = async {
        loop {
            match notifications.recv().await {
                Ok(notification) if is_thread_closed(thread_id, &notification) => return,
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    };
    if timeout(THREAD_UNLOAD_TIMEOUT, wait).await.is_err() {
        warn!(
            thread_id,
            "timed out waiting for app-server thread to close"
        );
    }
}

fn is_thread_closed(thread_id: &str, notification: &Notification) -> bool {
    notification.method == method::THREAD_CLOSED
        && notification
            .params
            .get("threadId")
            .and_then(|value| value.as_str())
            == Some(thread_id)
}

fn build_turn_input(req: &ExecutionRequest) -> Vec<TurnInputItem> {
    let mut items = Vec::with_capacity(1 + req.image_paths.len());
    items.push(TurnInputItem::Text {
        text: req.prompt.clone(),
    });
    for image in &req.image_paths {
        items.push(TurnInputItem::LocalImage {
            path: image.to_string_lossy().into_owned(),
        });
    }
    items
}

fn build_config_overrides(req: &ExecutionRequest) -> HashMap<String, JsonValue> {
    build_runtime_config_overrides(
        &req.config_overrides,
        req.model.as_deref(),
        req.service_tier,
        req.context_mode,
        req.reasoning_effort.as_str(),
    )
}

fn build_compact_config_overrides(req: &CompactRequest) -> HashMap<String, JsonValue> {
    build_runtime_config_overrides(
        &req.config_overrides,
        req.model.as_deref(),
        req.service_tier,
        req.context_mode,
        req.reasoning_effort.as_str(),
    )
}

fn build_runtime_config_overrides(
    config_overrides: &[String],
    model: Option<&str>,
    service_tier: Option<ServiceTier>,
    context_mode: Option<ContextMode>,
    reasoning_effort: &str,
) -> HashMap<String, JsonValue> {
    let mut out = HashMap::new();
    for override_arg in config_overrides {
        match parse_config_override(override_arg) {
            Some((key, value)) => {
                out.insert(key, value);
            }
            None => {
                warn!(override_arg, "ignoring malformed codex config override");
            }
        }
    }
    if let Some(model) = model {
        out.insert("model".to_string(), JsonValue::String(model.to_string()));
    }
    out.insert(
        "model_reasoning_effort".to_string(),
        JsonValue::String(reasoning_effort.to_string()),
    );
    if let Some(mode) = context_mode {
        out.insert(
            "model_context_window".to_string(),
            match mode {
                ContextMode::Standard => JsonValue::from(272_000u64),
                ContextMode::OneM => JsonValue::from(1_000_000u64),
            },
        );
    }
    if let Some(ServiceTier::Fast) = service_tier {
        out.insert(
            "service_tier".to_string(),
            service_tier_to_config_value(ServiceTier::Fast),
        );
    }
    out
}

fn parse_config_override(raw: &str) -> Option<(String, JsonValue)> {
    let (key, value) = raw.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let value = value.trim();
    let parsed = parse_toml_config_value(value)
        .and_then(|value| serde_json::to_value(value).ok())
        .unwrap_or_else(|| JsonValue::String(value.to_string()));
    Some((key.to_string(), parsed))
}

fn parse_toml_config_value(raw: &str) -> Option<toml::Value> {
    let wrapped = format!("value = {raw}");
    let mut table = toml::from_str::<toml::Table>(&wrapped).ok()?;
    table.remove("value")
}

fn service_tier_to_wire(tier: ServiceTier) -> Option<String> {
    match tier {
        ServiceTier::Fast => Some(tier.as_str().to_string()),
        ServiceTier::Flex => None,
    }
}

fn service_tier_to_config_value(tier: ServiceTier) -> JsonValue {
    match service_tier_to_wire(tier) {
        Some(value) => JsonValue::String(value),
        None => JsonValue::Null,
    }
}

struct ExecutionRequestContext {
    context_window: Option<u64>,
}

impl ExecutionRequestContext {
    fn from_request(request: &ExecutionRequest) -> Self {
        let context_window = request.context_mode.map(|mode| match mode {
            ContextMode::Standard => ContextMode::STANDARD_CONTEXT_WINDOW,
            ContextMode::OneM => 1_000_000u64,
        });
        Self { context_window }
    }
}

struct TurnRunner {
    client: Arc<JsonRpcClient>,
    notifications: broadcast::Receiver<Notification>,
    thread_id: String,
    turn_id: String,
    state: TurnState,
    update_tx: Option<mpsc::UnboundedSender<ExecutionUpdate>>,
    cancel_rx: Option<oneshot::Receiver<()>>,
    cancel_requested: bool,
}

impl TurnRunner {
    async fn drive(&mut self) -> Result<TurnOutcome> {
        let mut final_outcome: Option<TurnOutcome> = None;
        // Pull cancel_rx out of self so its borrow doesn't conflict with
        // &mut self borrows on the other select branch.
        let mut cancel_rx = self.cancel_rx.take();
        while final_outcome.is_none() {
            let has_cancel = cancel_rx.is_some() && !self.cancel_requested;
            tokio::select! {
                biased;
                cancel = async {
                    match cancel_rx.as_mut() {
                        Some(rx) => rx.await.ok(),
                        None => std::future::pending::<Option<()>>().await,
                    }
                }, if has_cancel => {
                    if cancel.is_some() {
                        self.cancel_requested = true;
                        self.send_interrupt().await;
                        final_outcome = Some(self.await_after_interrupt().await);
                    }
                }
                event = self.next_event() => {
                    match event? {
                        Some(notification) => {
                            if let Some(outcome) = self.handle_notification(notification).await {
                                final_outcome = Some(outcome);
                            }
                        }
                        None => {
                            return Err(anyhow!(
                                "codex 后端意外退出，正在重启"
                            ));
                        }
                    }
                }
            }
        }
        Ok(final_outcome.unwrap_or(TurnOutcome::Completed))
    }

    async fn next_event(&mut self) -> Result<Option<Notification>> {
        loop {
            match timeout(OUTPUT_IDLE_TIMEOUT, self.notifications.recv()).await {
                Ok(Ok(n)) if is_for_turn(&n, &self.thread_id, &self.turn_id) => {
                    return Ok(Some(n));
                }
                Ok(Ok(_)) => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) => return Ok(None),
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    warn!(lagged = n, "app-server notification stream lagged");
                    continue;
                }
                Err(_) => {
                    return Err(anyhow!(
                        "codex 输出超时（{} 秒无新事件）",
                        OUTPUT_IDLE_TIMEOUT.as_secs()
                    ));
                }
            }
        }
    }

    async fn handle_notification(&mut self, n: Notification) -> Option<TurnOutcome> {
        let method = n.method.as_str();
        let params = &n.params;
        match method {
            method::ITEM_STARTED => {
                if let Ok(item) = serde_json::from_value::<ItemNotification>(params.clone()) {
                    let updates = translator::translate_item_started(&mut self.state, &item);
                    self.emit_many(updates);
                }
            }
            method::ITEM_UPDATED => {
                if let Ok(item) = serde_json::from_value::<ItemNotification>(params.clone()) {
                    let updates = translator::translate_item_updated(&mut self.state, &item);
                    self.emit_many(updates);
                }
            }
            method::ITEM_COMPLETED => {
                if let Ok(item) = serde_json::from_value::<ItemNotification>(params.clone()) {
                    let updates = translator::translate_item_completed(&mut self.state, &item);
                    self.emit_many(updates);
                }
            }
            method::TURN_PLAN_UPDATED => {
                if let Ok(p) = serde_json::from_value::<TurnPlanUpdatedNotification>(params.clone())
                {
                    let updates = translator::translate_turn_plan_updated(&mut self.state, &p);
                    self.emit_many(updates);
                }
            }
            method::THREAD_TOKEN_USAGE_UPDATED => {
                if let Ok(p) =
                    serde_json::from_value::<TokenUsageUpdatedNotification>(params.clone())
                {
                    translator::translate_token_usage(&mut self.state, &p);
                }
            }
            method::THREAD_COMPACTED => {
                if let Ok(p) = serde_json::from_value::<CompactedNotification>(params.clone()) {
                    let update = translator::translate_compacted(&mut self.state, &p);
                    self.emit(update);
                }
            }
            method::MODEL_REROUTED => {
                if let Ok(p) = serde_json::from_value::<ModelReroutedNotification>(params.clone()) {
                    if let Some(update) = translator::translate_model_rerouted(&p) {
                        self.emit(update);
                    }
                }
            }
            method::ERROR => {
                let error = params.get("error").cloned().unwrap_or(JsonValue::Null);
                let will_retry = params
                    .get("willRetry")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let outcome = translator::translate_error_notification(&error, will_retry);
                if matches!(outcome, TurnOutcome::Failed(_)) {
                    return Some(outcome);
                }
            }
            method::TURN_COMPLETED => {
                if let Ok(p) = serde_json::from_value::<TurnCompletedNotification>(params.clone()) {
                    return Some(translator::translate_turn_completed(
                        &mut self.state,
                        &p.turn,
                    ));
                }
            }
            method::TURN_FAILED => {
                let msg = params
                    .get("error")
                    .and_then(|v| v.get("message"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| params.to_string());
                return Some(TurnOutcome::Failed(msg));
            }
            _ => {
                debug!(method = %method, "ignoring notification");
            }
        }
        None
    }

    async fn send_interrupt(&self) {
        let params = TurnInterruptParams {
            thread_id: self.thread_id.clone(),
            turn_id: Some(self.turn_id.clone()),
        };
        if let Err(err) = self
            .client
            .request::<_, TurnInterruptResponse>("turn/interrupt", &params)
            .await
        {
            warn!(error = %err, "turn/interrupt failed");
        }
    }

    async fn await_after_interrupt(&mut self) -> TurnOutcome {
        let deadline = tokio::time::Instant::now() + INTERRUPT_WAIT;
        loop {
            let remaining = match deadline.checked_duration_since(tokio::time::Instant::now()) {
                Some(d) => d,
                None => return TurnOutcome::Interrupted,
            };
            match timeout(remaining, self.notifications.recv()).await {
                Ok(Ok(n))
                    if is_for_turn(&n, &self.thread_id, &self.turn_id)
                        && (n.method == method::TURN_COMPLETED
                            || n.method == method::TURN_FAILED) =>
                {
                    return TurnOutcome::Interrupted;
                }
                Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => {
                    return TurnOutcome::Interrupted;
                }
            }
        }
    }

    fn emit(&self, update: ExecutionUpdate) {
        if let Some(tx) = self.update_tx.as_ref() {
            let _ = tx.send(update);
        }
    }

    fn emit_many(&self, updates: Vec<ExecutionUpdate>) {
        if let Some(tx) = self.update_tx.as_ref() {
            for update in updates {
                let _ = tx.send(update);
            }
        }
    }

    fn build_token_usage_info(&self, fallback_window: Option<u64>) -> Option<TokenUsageInfo> {
        let payload = self.state.token_usage.as_ref()?;
        Some(TokenUsageInfo {
            total_token_usage: TokenUsage {
                input_tokens: payload.total.input_tokens,
                cached_input_tokens: payload.total.cached_input_tokens,
                output_tokens: payload.total.output_tokens,
                reasoning_output_tokens: payload.total.reasoning_output_tokens,
                total_tokens: payload.total.total_tokens,
            },
            last_token_usage: TokenUsage {
                input_tokens: payload.last.input_tokens,
                cached_input_tokens: payload.last.cached_input_tokens,
                output_tokens: payload.last.output_tokens,
                reasoning_output_tokens: payload.last.reasoning_output_tokens,
                total_tokens: payload.last.total_tokens,
            },
            model_context_window: payload.model_context_window.or(fallback_window),
        })
    }
}

async fn wait_for_compaction(
    thread_id: &str,
    mut notifications: broadcast::Receiver<Notification>,
    cancel_rx: &mut Option<oneshot::Receiver<()>>,
) -> Result<()> {
    loop {
        let notification = match cancel_rx.as_mut() {
            Some(cancel) => {
                tokio::select! {
                    _ = cancel => return Err(anyhow!("codex turn aborted by user")),
                    result = timeout(OUTPUT_IDLE_TIMEOUT, next_compaction_notification(&mut notifications)) => result
                        .context(format!("codex compact timed out ({} seconds without completion)", OUTPUT_IDLE_TIMEOUT.as_secs()))??,
                }
            }
            None => timeout(
                OUTPUT_IDLE_TIMEOUT,
                next_compaction_notification(&mut notifications),
            )
            .await
            .context(format!(
                "codex compact timed out ({} seconds without completion)",
                OUTPUT_IDLE_TIMEOUT.as_secs()
            ))??,
        };

        if notification.method.is_empty() {
            anyhow::bail!("app-server notification stream closed during compact");
        }
        if is_compaction_completed(thread_id, &notification) {
            info!(
                thread_id,
                method = %notification.method,
                "codex compaction completed"
            );
            return Ok(());
        }
        if notification.method == method::ERROR {
            let error = notification
                .params
                .get("error")
                .cloned()
                .unwrap_or(JsonValue::Null);
            let will_retry = notification
                .params
                .get("willRetry")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if let TurnOutcome::Failed(msg) =
                translator::translate_error_notification(&error, will_retry)
            {
                anyhow::bail!("codex compact failed: {msg}");
            }
        }
    }
}

fn is_retryable_compact_error(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}").to_ascii_lowercase();
    message.contains("stream disconnected before completion")
        || message.contains("stream closed before response.completed")
}

fn is_compaction_completed(thread_id: &str, notification: &Notification) -> bool {
    match notification.method.as_str() {
        method::THREAD_COMPACTED => {
            notification.params.get("threadId").and_then(|v| v.as_str()) == Some(thread_id)
        }
        method::ITEM_COMPLETED => {
            let Ok(item) = serde_json::from_value::<ItemNotification>(notification.params.clone())
            else {
                return false;
            };
            item.thread_id == thread_id && item.item.item_type == "contextCompaction"
        }
        _ => false,
    }
}

async fn next_compaction_notification(
    notifications: &mut broadcast::Receiver<Notification>,
) -> Result<Notification> {
    loop {
        match notifications.recv().await {
            Ok(n) => return Ok(n),
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(lagged = n, "app-server notification stream lagged");
            }
            Err(broadcast::error::RecvError::Closed) => {
                anyhow::bail!("app-server notification stream closed during compact");
            }
        }
    }
}

fn is_for_turn(notification: &Notification, thread_id: &str, turn_id: &str) -> bool {
    let params = &notification.params;
    let matches_thread = params
        .get("threadId")
        .and_then(|v| v.as_str())
        .map(|v| v == thread_id)
        .unwrap_or(false);
    if !matches_thread {
        // Global-ish notifications (configWarning, account/*) have no
        // threadId; allow them so errors can still abort the turn.
        if params.get("threadId").is_none() {
            return true;
        }
        return false;
    }
    match params.get("turnId").and_then(|v| v.as_str()) {
        Some(v) => v == turn_id,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_for_turn_matches_thread_and_turn() {
        let n = Notification {
            method: "turn/started".into(),
            params: serde_json::json!({"threadId":"t","turnId":"u","turn":{"id":"u","status":"inProgress"}}),
        };
        assert!(is_for_turn(&n, "t", "u"));
        assert!(!is_for_turn(&n, "t", "other"));
        assert!(!is_for_turn(&n, "different", "u"));
    }

    #[test]
    fn is_for_turn_allows_unscoped() {
        let n = Notification {
            method: "configWarning".into(),
            params: serde_json::json!({"message":"hi"}),
        };
        assert!(is_for_turn(&n, "t", "u"));
    }

    #[test]
    fn compact_completion_accepts_legacy_thread_compacted_notification() {
        let n = Notification {
            method: method::THREAD_COMPACTED.into(),
            params: serde_json::json!({"threadId":"thread-1"}),
        };

        assert!(is_compaction_completed("thread-1", &n));
        assert!(!is_compaction_completed("other-thread", &n));
    }

    #[test]
    fn compact_completion_accepts_context_compaction_item() {
        let n = Notification {
            method: method::ITEM_COMPLETED.into(),
            params: serde_json::json!({
                "threadId": "thread-1",
                "item": {
                    "type": "contextCompaction",
                    "id": "item-1"
                }
            }),
        };

        assert!(is_compaction_completed("thread-1", &n));
        assert!(!is_compaction_completed("other-thread", &n));
    }

    #[test]
    fn compact_completion_ignores_regular_items() {
        let n = Notification {
            method: method::ITEM_COMPLETED.into(),
            params: serde_json::json!({
                "threadId": "thread-1",
                "item": {
                    "type": "agentMessage",
                    "id": "item-1"
                }
            }),
        };

        assert!(!is_compaction_completed("thread-1", &n));
    }

    #[test]
    fn compact_retry_detects_stream_disconnect_errors() {
        let err = anyhow!(
            "codex compact failed: stream disconnected before completion: stream closed before response.completed"
        );

        assert!(is_retryable_compact_error(&err));
    }

    #[test]
    fn compact_retry_ignores_non_stream_errors() {
        let err = anyhow!("codex compact failed: context length exceeded");

        assert!(!is_retryable_compact_error(&err));
    }

    #[test]
    fn flex_service_tier_does_not_write_empty_config_override() {
        let req = ExecutionRequest {
            prompt: "hi".to_string(),
            workspace_dir: std::path::PathBuf::from("/tmp"),
            codex_home: std::path::PathBuf::from("/tmp/codex-home"),
            config_overrides: Vec::new(),
            add_dirs: Vec::new(),
            session_state: crate::session::state::SessionState::default(),
            model: Some("gpt-5.5".to_string()),
            service_tier: Some(ServiceTier::Flex),
            context_mode: None,
            reasoning_effort: crate::session::state::ReasoningEffort::High,
            image_paths: Vec::new(),
        };

        let overrides = build_config_overrides(&req);
        assert!(!overrides.contains_key("service_tier"));
    }

    #[test]
    fn config_overrides_are_lower_priority_than_session_runtime_settings() {
        let req = ExecutionRequest {
            prompt: "hi".to_string(),
            workspace_dir: std::path::PathBuf::from("/tmp"),
            codex_home: std::path::PathBuf::from("/tmp/codex-home"),
            config_overrides: vec![
                "model=\"gpt-config\"".to_string(),
                "model_reasoning_effort=\"low\"".to_string(),
                "model_context_window=128000".to_string(),
                "tool_output_token_limit=2048".to_string(),
            ],
            add_dirs: Vec::new(),
            session_state: crate::session::state::SessionState::default(),
            model: Some("gpt-session".to_string()),
            service_tier: None,
            context_mode: Some(ContextMode::OneM),
            reasoning_effort: crate::session::state::ReasoningEffort::High,
            image_paths: Vec::new(),
        };

        let overrides = build_config_overrides(&req);
        assert_eq!(overrides["model"], JsonValue::String("gpt-session".into()));
        assert_eq!(
            overrides["model_reasoning_effort"],
            JsonValue::String("high".into())
        );
        assert_eq!(
            overrides["model_context_window"],
            JsonValue::from(1_000_000u64)
        );
        assert_eq!(overrides["tool_output_token_limit"], JsonValue::from(2048));
    }

    #[test]
    fn compact_config_overrides_match_session_runtime_settings() {
        let req = CompactRequest {
            session_id: "thread-1".to_string(),
            workspace_dir: std::path::PathBuf::from("/tmp/work-a"),
            config_overrides: vec![
                "model=\"gpt-config\"".to_string(),
                "model_reasoning_effort=\"low\"".to_string(),
                "model_context_window=128000".to_string(),
                "tool_output_token_limit=2048".to_string(),
            ],
            model: Some("gpt-session".to_string()),
            service_tier: Some(ServiceTier::Fast),
            context_mode: Some(ContextMode::OneM),
            reasoning_effort: crate::session::state::ReasoningEffort::High,
        };

        let overrides = build_compact_config_overrides(&req);
        assert_eq!(overrides["model"], JsonValue::String("gpt-session".into()));
        assert_eq!(
            overrides["model_reasoning_effort"],
            JsonValue::String("high".into())
        );
        assert_eq!(
            overrides["model_context_window"],
            JsonValue::from(1_000_000u64)
        );
        assert_eq!(overrides["service_tier"], JsonValue::String("fast".into()));
        assert_eq!(overrides["tool_output_token_limit"], JsonValue::from(2048));
    }

    #[test]
    fn runtime_config_signature_tracks_session_runtime_settings() {
        let mut req = ExecutionRequest {
            prompt: "hi".to_string(),
            workspace_dir: std::path::PathBuf::from("/tmp/work-a"),
            codex_home: std::path::PathBuf::from("/tmp/codex-home"),
            config_overrides: Vec::new(),
            add_dirs: Vec::new(),
            session_state: crate::session::state::SessionState::default(),
            model: Some("gpt-5.5".to_string()),
            service_tier: None,
            context_mode: Some(ContextMode::Standard),
            reasoning_effort: crate::session::state::ReasoningEffort::High,
            image_paths: Vec::new(),
        };

        let original = RuntimeConfigSignature::from_request(&req);
        req.context_mode = Some(ContextMode::OneM);
        assert_ne!(RuntimeConfigSignature::from_request(&req), original);

        req.context_mode = Some(ContextMode::Standard);
        req.reasoning_effort = crate::session::state::ReasoningEffort::Low;
        assert_ne!(RuntimeConfigSignature::from_request(&req), original);

        req.reasoning_effort = crate::session::state::ReasoningEffort::High;
        req.model = Some("gpt-5.4".to_string());
        assert_ne!(RuntimeConfigSignature::from_request(&req), original);
    }

    #[test]
    fn compact_runtime_config_signature_tracks_session_runtime_settings() {
        let mut req = CompactRequest {
            session_id: "thread-1".to_string(),
            workspace_dir: std::path::PathBuf::from("/tmp/work-a"),
            config_overrides: Vec::new(),
            model: Some("gpt-5.5".to_string()),
            service_tier: None,
            context_mode: Some(ContextMode::Standard),
            reasoning_effort: crate::session::state::ReasoningEffort::High,
        };

        let original = RuntimeConfigSignature::from_compact_request(&req);
        req.context_mode = Some(ContextMode::OneM);
        assert_ne!(RuntimeConfigSignature::from_compact_request(&req), original);

        req.context_mode = Some(ContextMode::Standard);
        req.reasoning_effort = crate::session::state::ReasoningEffort::Low;
        assert_ne!(RuntimeConfigSignature::from_compact_request(&req), original);

        req.reasoning_effort = crate::session::state::ReasoningEffort::High;
        req.model = Some("gpt-5.4".to_string());
        assert_ne!(RuntimeConfigSignature::from_compact_request(&req), original);
    }

    #[test]
    fn thread_closed_notification_matches_thread_id() {
        let n = Notification {
            method: method::THREAD_CLOSED.into(),
            params: serde_json::json!({"threadId":"thread-1"}),
        };

        assert!(is_thread_closed("thread-1", &n));
        assert!(!is_thread_closed("other-thread", &n));
    }

    #[test]
    fn malformed_config_override_is_ignored() {
        assert!(parse_config_override("not-a-pair").is_none());
        assert!(parse_config_override(" = 1").is_none());
    }
}
