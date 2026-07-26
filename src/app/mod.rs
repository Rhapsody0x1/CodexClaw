//! The gateway-facing application layer: `App` owns the shared runtime state
//! (session store, QQ client, codex executor, busy slot) and its submodules
//! implement the message and turn flows.
//!
//! - [`inbound`]: normalize incoming QQ events and dispatch command outcomes
//! - [`turn`]: `run_turn` phases plus the `/compact` and `/self-update` flows
//! - [`approvals`]: server-initiated approval routing and prompts
//! - [`format`]: pure formatting helpers

mod approvals;
mod format;
mod inbound;
mod turn;

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Result;
use tokio::sync::{Mutex, oneshot};
use tracing::info;

use crate::{
    codex::{ApprovalOutcome, CodexExecutor},
    config::AppConfig,
    memory::store::MemoryStore,
    message::IncomingMessage,
    qq::{Directive, QqApiClient},
    scheduler::{ProactiveNotifier, SchedulerCtx},
    session::SessionStore,
    shadow::ShadowWorker,
};

/// `QqApiClient` is the production notifier behind the scheduler's
/// [`ProactiveNotifier`] seam; the impl lives here (not in `qq/` or
/// `scheduler/`) so neither of those modules depends on the other.
impl ProactiveNotifier for QqApiClient {
    fn send_markdown_proactive<'a>(
        &'a self,
        openid: &'a str,
        text: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(QqApiClient::send_markdown_proactive(self, openid, text))
    }
}

pub struct App {
    pub config: AppConfig,
    pub(crate) session: Arc<SessionStore>,
    pub qq_client: Arc<QqApiClient>,
    pub codex: Arc<CodexExecutor>,
    pub(crate) memory: Arc<MemoryStore>,
    pub(crate) shadow: Option<Arc<ShadowWorker>>,
    /// The scheduler's slice of the app. The `App` holds the only strong
    /// reference (the tick loop keeps a `Weak`), so dropping the `App` still
    /// parks the scheduler exactly as when it held `Weak<App>` directly.
    pub(crate) scheduler_ctx: Arc<SchedulerCtx>,
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

impl App {
    pub fn new(
        config: AppConfig,
        session: Arc<SessionStore>,
        qq_client: Arc<QqApiClient>,
        codex: Arc<CodexExecutor>,
        memory: Arc<MemoryStore>,
        shadow: Option<Arc<ShadowWorker>>,
        scheduler_ctx: Arc<SchedulerCtx>,
    ) -> Arc<Self> {
        let app = Arc::new(Self {
            config,
            session,
            qq_client,
            codex,
            memory,
            shadow,
            scheduler_ctx,
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

    fn runtime_profile_path(&self) -> PathBuf {
        let codex_home = &self.config.general.codex_home_global;
        codex_home.join("config.toml")
    }

    /// Resolve a user's UI language, falling back to the canonical default when
    /// they have no session record yet. Shared by command handlers and the
    /// scheduler so locale resolution stays consistent in one place.
    pub(crate) async fn command_locale(&self, openid: &str) -> String {
        self.session.command_locale(openid).await
    }

    /// Reply to `message_id` from `openid` with `text`, quoting the original
    /// message — the reply shape every user-facing message in this file uses.
    async fn reply_text(&self, openid: &str, message_id: &str, text: &str) -> Result<()> {
        self.qq_client
            .send_text(openid, message_id, text, Some(message_id))
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
}
