use std::{path::Path, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    sync::{Mutex, oneshot},
    time::timeout,
};
use tracing::{error, info, warn};

use crate::config::AppConfig;

pub const ENV_LAUNCHER_ADDR: &str = "CODEX_CLAW_LAUNCHER_ADDR";
pub const CHILD_ARG: &str = "--run-bot-child";
pub const LAUNCHER_ARG: &str = "--launcher";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlRequest {
    Ready { pid: u32 },
    Deploy { binary_path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ControlResponse {
    ok: bool,
    message: String,
}

struct LauncherState {
    addr: String,
    config_path: Option<String>,
    child: Option<Child>,
    current_binary: PathBuf,
    pending_ready: Option<oneshot::Sender<()>>,
}

impl LauncherState {
    async fn kill_running_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    async fn start_child(&mut self, binary: &Path) -> Result<oneshot::Receiver<()>> {
        let mut command = Command::new(binary);
        command.arg(CHILD_ARG).env(ENV_LAUNCHER_ADDR, &self.addr);
        if let Some(config_path) = self.config_path.as_ref() {
            command.env("CODEX_CLAW_CONFIG", config_path);
        }
        let child = command
            .spawn()
            .with_context(|| format!("failed to spawn child process {}", binary.display()))?;
        let (tx, rx) = oneshot::channel();
        self.pending_ready = Some(tx);
        self.child = Some(child);
        Ok(rx)
    }

    async fn deploy(&mut self, binary: &Path) -> Result<String> {
        let previous = self.current_binary.clone();
        self.kill_running_child().await;
        let ready = self.start_child(binary).await?;
        match timeout(Duration::from_secs(20), ready).await {
            Ok(Ok(())) => {
                self.current_binary = binary.to_path_buf();
                Ok(format!("已切换到新版本：{}", binary.display()))
            }
            _ => {
                warn!(
                    "new child did not report ready in time, rolling back to {}",
                    previous.display()
                );
                self.kill_running_child().await;
                let rollback_ready = self.start_child(&previous).await?;
                let _ = timeout(Duration::from_secs(20), rollback_ready).await;
                Err(anyhow!("新版本启动超时，已回滚到旧版本"))
            }
        }
    }
}

pub async fn run_launcher(config: &AppConfig) -> Result<()> {
    let addr = config.general.launcher_control_addr.clone();
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind launcher control address {addr}"))?;
    let config_path = std::env::var("CODEX_CLAW_CONFIG").ok();
    let initial_binary = std::env::current_exe().context("failed to detect current executable")?;
    let state = Arc::new(Mutex::new(LauncherState {
        addr: addr.clone(),
        config_path,
        child: None,
        current_binary: initial_binary.clone(),
        pending_ready: None,
    }));
    {
        let mut guard = state.lock().await;
        let ready = guard.start_child(&initial_binary).await?;
        match timeout(Duration::from_secs(20), ready).await {
            Ok(Ok(())) => {
                info!("launcher initial child is ready");
            }
            _ => {
                return Err(anyhow!("launcher child startup timed out"));
            }
        }
    }
    info!(addr = %addr, "launcher started");
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, state).await {
                error!("launcher control connection from {peer:?} failed: {err:#}");
            }
        });
    }
}

async fn handle_connection(stream: TcpStream, state: Arc<Mutex<LauncherState>>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        return Ok(());
    }
    let request = serde_json::from_str::<ControlRequest>(line.trim())
        .context("failed to parse launcher request")?;
    let response = match request {
        ControlRequest::Ready { pid } => {
            let mut guard = state.lock().await;
            if let Some(notify) = guard.pending_ready.take() {
                let _ = notify.send(());
            }
            ControlResponse {
                ok: true,
                message: format!("ack pid={pid}"),
            }
        }
        ControlRequest::Deploy { binary_path } => {
            let binary = PathBuf::from(binary_path);
            let mut guard = state.lock().await;
            match guard.deploy(&binary).await {
                Ok(message) => ControlResponse { ok: true, message },
                Err(err) => ControlResponse {
                    ok: false,
                    message: err.to_string(),
                },
            }
        }
    };
    writer
        .write_all(format!("{}\n", serde_json::to_string(&response)?).as_bytes())
        .await?;
    writer.flush().await?;
    Ok(())
}

pub async fn notify_ready(addr: &str) -> Result<()> {
    let pid = std::process::id();
    let response = send_request(addr, &ControlRequest::Ready { pid }).await?;
    if !response.ok {
        return Err(anyhow!("launcher rejected ready: {}", response.message));
    }
    Ok(())
}

pub async fn request_deploy(addr: &str, binary_path: &Path) -> Result<String> {
    let response = send_request(
        addr,
        &ControlRequest::Deploy {
            binary_path: binary_path.display().to_string(),
        },
    )
    .await?;
    if response.ok {
        Ok(response.message)
    } else {
        Err(anyhow!(response.message))
    }
}

async fn send_request(addr: &str, request: &ControlRequest) -> Result<ControlResponse> {
    let mut stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("failed to connect launcher at {addr}"))?;
    let raw = serde_json::to_string(request)?;
    stream.write_all(raw.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    serde_json::from_str::<ControlResponse>(line.trim())
        .context("failed to parse launcher response")
}
