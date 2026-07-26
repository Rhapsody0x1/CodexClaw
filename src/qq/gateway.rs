use std::{
    future::pending,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use crate::{
    qq::{
        api::QqApiClient,
        types::{
            C2CMessageEvent, DISPATCH_EVENT, GatewayEnvelope, HEARTBEAT_ACK_EVENT, HEARTBEAT_EVENT,
            HELLO_EVENT, HelloPayload, IDENTIFY_EVENT, INTENT_GROUP_AND_C2C, INVALID_SESSION_EVENT,
            RECONNECT_EVENT, RESUME_EVENT, ReadyPayload,
        },
    },
    util::layout::DataLayout,
};

/// Outbound side of the gateway's only callback into the application: every
/// decoded `C2C_MESSAGE_CREATE` event is forwarded here. The gateway never
/// awaits the handling of an event, so the consumer must spawn per event to
/// preserve the original detached-task concurrency.
type C2CEventSender = mpsc::UnboundedSender<C2CMessageEvent>;

const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
/// Floor for the server-provided heartbeat interval. Guards against a zero
/// (interval panic) or absurdly small value that would make the half-open
/// check fire before any ACK could realistically round-trip.
const MIN_HEARTBEAT_MS: u64 = 1_000;
/// Reconnect only after this many consecutive heartbeats go unacknowledged, so
/// a single slow ACK or a tick/ACK scheduling race never triggers a spurious
/// reconnect while a truly dead connection is still detected within ~2 ticks.
const MAX_MISSED_HEARTBEAT_ACKS: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct GatewaySessionState {
    session_id: Option<String>,
    last_seq: Option<u64>,
}

struct GatewaySessionStore {
    path: PathBuf,
    state: RwLock<GatewaySessionState>,
}

impl GatewaySessionStore {
    async fn load_or_init(data_dir: &Path) -> Result<Self> {
        let layout = DataLayout::new(data_dir);
        tokio::fs::create_dir_all(layout.qq_dir()).await?;
        let path = layout.gateway_session_file();
        let state = match tokio::fs::read_to_string(&path).await {
            Ok(raw) => serde_json::from_str::<GatewaySessionState>(&raw)
                .with_context(|| format!("failed to parse {}", path.display()))?,
            Err(_) => GatewaySessionState::default(),
        };
        let store = Self {
            path,
            state: RwLock::new(state),
        };
        store.persist().await?;
        Ok(store)
    }

    async fn snapshot(&self) -> GatewaySessionState {
        self.state.read().await.clone()
    }

    async fn set_last_seq(&self, last_seq: Option<u64>) -> Result<()> {
        let mut state = self.state.write().await;
        state.last_seq = last_seq;
        drop(state);
        self.persist().await
    }

    async fn set_session_id(&self, session_id: Option<String>) -> Result<()> {
        let mut state = self.state.write().await;
        state.session_id = session_id;
        drop(state);
        self.persist().await
    }

    async fn clear(&self) -> Result<()> {
        let mut state = self.state.write().await;
        *state = GatewaySessionState::default();
        drop(state);
        self.persist().await
    }

    async fn persist(&self) -> Result<()> {
        let raw = serde_json::to_string_pretty(&*self.state.read().await)?;
        tokio::fs::write(&self.path, raw)
            .await
            .with_context(|| format!("failed to write {}", self.path.display()))?;
        Ok(())
    }
}

pub fn spawn_gateway(data_dir: PathBuf, qq_client: Arc<QqApiClient>, events: C2CEventSender) {
    tokio::spawn(async move {
        let session_store = match GatewaySessionStore::load_or_init(&data_dir).await {
            Ok(store) => store,
            Err(err) => {
                error!("failed to initialize qq gateway session store: {err:#}");
                return;
            }
        };
        let mut reconnect_delay = Duration::from_secs(1);
        loop {
            match connect_once(&qq_client, &events, &session_store, &mut reconnect_delay).await {
                Ok(()) => reconnect_delay = Duration::from_secs(1),
                Err(err) => {
                    warn!(
                        "qq gateway loop ended: {err:#}; reconnecting in {}s",
                        reconnect_delay.as_secs()
                    );
                    tokio::time::sleep(reconnect_delay).await;
                    reconnect_delay = std::cmp::min(reconnect_delay * 2, MAX_RECONNECT_DELAY);
                }
            }
        }
    });
}

async fn connect_once(
    qq_client: &QqApiClient,
    events: &C2CEventSender,
    session_store: &GatewaySessionStore,
    reconnect_delay: &mut Duration,
) -> Result<()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let token = qq_client.get_access_token().await?;
    let gateway_url = qq_client.get_gateway_url().await?;
    info!("connecting to qq gateway at {}", gateway_url);
    let request = gateway_url
        .into_client_request()
        .map_err(|err| anyhow!("failed to build websocket request: {err}"))?;
    let (mut websocket, _) = connect_async(request).await?;
    let session = session_store.snapshot().await;
    let mut last_seq = session.last_seq;
    let mut session_id = session.session_id;
    let mut heartbeat: Option<tokio::time::Interval> = None;
    // Half-open detection: incremented when a heartbeat is sent, reset to 0 on
    // its ACK. If it reaches MAX_MISSED_HEARTBEAT_ACKS the connection is
    // silently dead (no RST) and we reconnect rather than block forever on
    // websocket.next().
    let mut unacked_heartbeats: u32 = 0;

    loop {
        tokio::select! {
            _ = async {
                if let Some(interval) = &mut heartbeat {
                    interval.tick().await;
                } else {
                    pending::<()>().await;
                }
            } => {
                if unacked_heartbeats >= MAX_MISSED_HEARTBEAT_ACKS {
                    return Err(anyhow!("qq gateway heartbeat not acknowledged; connection is half-open"));
                }
                let payload = serde_json::json!({
                    "op": HEARTBEAT_EVENT,
                    "d": last_seq,
                });
                websocket.send(Message::Text(payload.to_string())).await?;
                unacked_heartbeats += 1;
            }
            message = websocket.next() => {
                let Some(message) = message else {
                    return Err(anyhow!("qq gateway websocket closed"));
                };
                let message = message?;
                match message {
                    Message::Text(text) => {
                        let payload = serde_json::from_str::<GatewayEnvelope>(&text)
                            .with_context(|| format!("failed to parse gateway payload: {text}"))?;
                        if let Some(seq) = payload.s {
                            last_seq = Some(seq);
                            session_store.set_last_seq(last_seq).await?;
                        }
                        match payload.op {
                            HELLO_EVENT => {
                                let hello: HelloPayload = serde_json::from_value(payload.d)?;
                                // Clamp the server-provided interval: tokio::time::interval
                                // panics on a zero period, and heartbeat_interval is untrusted
                                // network data from the gateway HELLO frame. A floor also keeps
                                // a tiny value from tripping the half-open check every cycle.
                                let heartbeat_ms = hello.heartbeat_interval.max(MIN_HEARTBEAT_MS);
                                let mut interval = tokio::time::interval(Duration::from_millis(heartbeat_ms));
                                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                                heartbeat = Some(interval);
                                if let (Some(existing_session), Some(seq)) = (session_id.as_deref(), last_seq) {
                                    info!("resuming qq gateway session {}", existing_session);
                                    websocket.send(Message::Text(serde_json::json!({
                                        "op": RESUME_EVENT,
                                        "d": {
                                            "token": format!("QQBot {}", token),
                                            "session_id": existing_session,
                                            "seq": seq,
                                        }
                                    }).to_string())).await?;
                                } else {
                                    let intents = INTENT_GROUP_AND_C2C;
                                    info!("identifying qq gateway with intents {}", intents);
                                    websocket.send(Message::Text(serde_json::json!({
                                        "op": IDENTIFY_EVENT,
                                        "d": {
                                            "token": format!("QQBot {}", token),
                                            "intents": intents,
                                            "shard": [0, 1],
                                        }
                                    }).to_string())).await?;
                                }
                            }
                            DISPATCH_EVENT => {
                                match payload.t.as_deref() {
                                    Some("READY") => {
                                        let ready: ReadyPayload = serde_json::from_value(payload.d)?;
                                        info!("qq gateway ready, session {}", ready.session_id);
                                        session_id = Some(ready.session_id.clone());
                                        session_store.set_session_id(session_id.clone()).await?;
                                        // Connection is healthy again; reset the reconnect backoff
                                        // so the next unexpected drop retries promptly.
                                        *reconnect_delay = Duration::from_secs(1);
                                    }
                                    Some("RESUMED") => {
                                        info!("qq gateway session resumed");
                                        *reconnect_delay = Duration::from_secs(1);
                                    }
                                    Some("C2C_MESSAGE_CREATE") => {
                                        let event = serde_json::from_value::<C2CMessageEvent>(payload.d)?;
                                        // Non-blocking hand-off: the gateway must never await
                                        // event handling, or one slow turn would stall the
                                        // heartbeat and the whole receive loop.
                                        if events.send(event).is_err() {
                                            warn!("c2c event consumer is gone; dropping message from gateway");
                                        }
                                    }
                                    Some(other) => {
                                        info!("ignoring gateway dispatch event {}", other);
                                    }
                                    None => {}
                                }
                            }
                            HEARTBEAT_ACK_EVENT => {
                                unacked_heartbeats = 0;
                            }
                            RECONNECT_EVENT => {
                                return Err(anyhow!("qq gateway requested reconnect"));
                            }
                            INVALID_SESSION_EVENT => {
                                let can_resume = serde_json::from_value::<bool>(payload.d).unwrap_or(false);
                                warn!("qq gateway invalid session, can_resume={}", can_resume);
                                if !can_resume {
                                    session_store.clear().await?;
                                    qq_client.invalidate_access_token().await;
                                }
                                return Err(anyhow!("qq gateway invalid session"));
                            }
                            other => {
                                info!("received gateway op {}", other);
                            }
                        }
                    }
                    Message::Ping(payload) => {
                        websocket.send(Message::Pong(payload)).await?;
                    }
                    Message::Close(frame) => {
                        return Err(anyhow!("qq gateway closed: {frame:?}"));
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn reload(data_dir: &Path) -> GatewaySessionState {
        GatewaySessionStore::load_or_init(data_dir)
            .await
            .expect("reload store")
            .snapshot()
            .await
    }

    #[tokio::test]
    async fn load_or_init_creates_an_empty_session_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = GatewaySessionStore::load_or_init(dir.path())
            .await
            .expect("init store");

        assert!(
            store.path.exists(),
            "session file should be created eagerly"
        );
        let state = store.snapshot().await;
        assert_eq!(state.session_id, None);
        assert_eq!(state.last_seq, None);
    }

    #[tokio::test]
    async fn session_id_and_seq_survive_a_reload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = GatewaySessionStore::load_or_init(dir.path())
            .await
            .expect("init store");
        store
            .set_session_id(Some("sess-1".to_string()))
            .await
            .expect("set session id");
        store.set_last_seq(Some(42)).await.expect("set last seq");

        let state = reload(dir.path()).await;
        assert_eq!(state.session_id.as_deref(), Some("sess-1"));
        assert_eq!(state.last_seq, Some(42));
    }

    #[tokio::test]
    async fn clear_drops_both_fields_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = GatewaySessionStore::load_or_init(dir.path())
            .await
            .expect("init store");
        store
            .set_session_id(Some("sess-1".to_string()))
            .await
            .expect("set session id");
        store.set_last_seq(Some(42)).await.expect("set last seq");
        store.clear().await.expect("clear");

        let state = reload(dir.path()).await;
        assert_eq!(state.session_id, None);
        assert_eq!(state.last_seq, None);
    }

    /// A *missing* file is a normal cold start (see the test above), but an
    /// unparsable one is reported rather than silently reset, so `spawn_gateway`
    /// refuses to start instead of quietly discarding a resumable session.
    #[tokio::test]
    async fn unparsable_session_file_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = DataLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.qq_dir())
            .await
            .expect("mkdir");
        tokio::fs::write(layout.gateway_session_file(), "{ not json")
            .await
            .expect("write garbage");

        assert!(GatewaySessionStore::load_or_init(dir.path()).await.is_err());
    }
}
