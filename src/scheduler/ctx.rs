//! The scheduler's view of the application: just the shared services the cron
//! machinery needs, held without a reference back to `App` so the module
//! dependency stays one-directional (`app -> scheduler`).

use std::{future::Future, pin::Pin, sync::Arc};

use crate::{codex::CodexExecutor, config::AppConfig, session::SessionStore};

/// Everything scheduler code needs from the application. The `App` owns the
/// only strong reference; the tick loop holds it weakly (see
/// [`super::loop_::Scheduler`]) so a dropped `App` silently parks the
/// scheduler instead of keeping the runtime alive.
pub(crate) struct SchedulerCtx {
    pub(crate) config: AppConfig,
    pub(crate) session: Arc<SessionStore>,
    pub(crate) codex: Arc<CodexExecutor>,
    pub(crate) notifier: Arc<dyn ProactiveNotifier>,
}

/// The one outbound-messaging capability the scheduler uses: pushing a
/// proactive markdown message to a job's owner. Kept as a trait so the
/// scheduler does not depend on the QQ client type; the concrete impl is a
/// one-line delegate on `QqApiClient` (wired up in `app`).
pub(crate) trait ProactiveNotifier: Send + Sync {
    fn send_markdown_proactive<'a>(
        &'a self,
        openid: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;
}
