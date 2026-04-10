use std::{
    future::pending,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use crate::{
    app::App,
    qq::types::{
        C2CMessageEvent, DISPATCH_EVENT, GatewayEnvelope, HEARTBEAT_ACK_EVENT, HEARTBEAT_EVENT,
        HELLO_EVENT, HelloPayload, IDENTIFY_EVENT, INTENT_GROUP_AND_C2C, INVALID_SESSION_EVENT,
        RECONNECT_EVENT, RESUME_EVENT, ReadyPayload,
    },
};

const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

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
        let dir = data_dir.join("qq");
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join("gateway-session.json");
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

pub fn spawn_gateway(app: Arc<App>) {
    tokio::spawn(async move {
        let session_store = match GatewaySessionStore::load_or_init(&app.config.general.data_dir).await
        {
            Ok(store) => store,
            Err(err) => {
                error!("failed to initialize qq gateway session store: {err:#}");
                return;
            }
        };
        let mut reconnect_delay = Duration::from_secs(1);
        loop {
            match connect_once(app.clone(), &session_store).await {
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

async fn connect_once(app: Arc<App>, session_store: &GatewaySessionStore) -> Result<()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let token = app.qq_client.get_access_token().await?;
    let gateway_url = app.qq_client.get_gateway_url().await?;
    info!("connecting to qq gateway at {}", gateway_url);
    let request = gateway_url
        .into_client_request()
        .map_err(|err| anyhow!("failed to build websocket request: {err}"))?;
    let (mut websocket, _) = connect_async(request).await?;
    let session = session_store.snapshot().await;
    let mut last_seq = session.last_seq;
    let mut session_id = session.session_id;
    let mut heartbeat: Option<tokio::time::Interval> = None;

    loop {
        tokio::select! {
            _ = async {
                if let Some(interval) = &mut heartbeat {
                    interval.tick().await;
                } else {
                    pending::<()>().await;
                }
            } => {
                let payload = serde_json::json!({
                    "op": HEARTBEAT_EVENT,
                    "d": last_seq,
                });
                websocket.send(Message::Text(payload.to_string())).await?;
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
                                let mut interval = tokio::time::interval(Duration::from_millis(hello.heartbeat_interval));
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
                                    }
                                    Some("RESUMED") => {
                                        info!("qq gateway session resumed");
                                    }
                                    Some("C2C_MESSAGE_CREATE") => {
                                        let event = serde_json::from_value::<C2CMessageEvent>(payload.d)?;
                                        let app = app.clone();
                                        tokio::spawn(async move {
                                            if let Err(err) = app.handle_c2c_event(event).await {
                                                warn!("failed to process c2c message from gateway: {err:#}");
                                            }
                                        });
                                    }
                                    Some(other) => {
                                        info!("ignoring gateway dispatch event {}", other);
                                    }
                                    None => {}
                                }
                            }
                            HEARTBEAT_ACK_EVENT => {}
                            RECONNECT_EVENT => {
                                return Err(anyhow!("qq gateway requested reconnect"));
                            }
                            INVALID_SESSION_EVENT => {
                                let can_resume = serde_json::from_value::<bool>(payload.d).unwrap_or(false);
                                warn!("qq gateway invalid session, can_resume={}", can_resume);
                                if !can_resume {
                                    session_store.clear().await?;
                                    app.qq_client.invalidate_access_token().await;
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
                        return Err(anyhow!("qq gateway closed: {:?}", frame));
                    }
                    _ => {}
                }
            }
        }
    }
}
