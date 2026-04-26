//! Newline-delimited JSON transport for the `codex app-server` stdio child.
//!
//! Each line on stdout is parsed as either a `Response`, a server-initiated
//! `Request`, or a `Notification`. Parsing failures are reported to the caller
//! which logs them and keeps reading (a malformed line must never kill the
//! reader).

use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};
use tracing::{debug, warn};

use super::protocol::{JsonRpcError, Message};

pub struct StdioTransport {
    /// Held by the writer side.
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<Option<BufReader<ChildStdout>>>,
    stderr: Mutex<Option<ChildStderr>>,
    child: Mutex<Option<Child>>,
}

impl StdioTransport {
    /// Spawn `codex app-server --listen stdio://` with the given environment.
    pub fn spawn(
        codex_binary: &std::path::Path,
        codex_home: &std::path::Path,
        sqlite_home: &std::path::Path,
        extra_path: Option<&std::ffi::OsStr>,
    ) -> Result<Self> {
        let mut cmd = Command::new(codex_binary);
        cmd.arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .env("CODEX_HOME", codex_home)
            .env("CODEX_SQLITE_HOME", sqlite_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(path) = extra_path {
            cmd.env("PATH", path);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn {}", codex_binary.display()))?;
        let stdin = child.stdin.take().context("child stdin missing")?;
        let stdout = child.stdout.take().context("child stdout missing")?;
        let stderr = child.stderr.take().context("child stderr missing")?;
        Ok(Self {
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(Some(BufReader::new(stdout))),
            stderr: Mutex::new(Some(stderr)),
            child: Mutex::new(Some(child)),
        })
    }

    pub async fn take_stdout(&self) -> Option<BufReader<ChildStdout>> {
        self.stdout.lock().await.take()
    }

    pub async fn take_stderr(&self) -> Option<ChildStderr> {
        self.stderr.lock().await.take()
    }

    pub async fn take_child(&self) -> Option<Child> {
        self.child.lock().await.take()
    }

    pub async fn write_message(&self, value: serde_json::Value) -> Result<()> {
        let mut line = serde_json::to_vec(&value).context("serialize JSON-RPC message")?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&line).await.context("write stdin")?;
        stdin.flush().await.context("flush stdin")?;
        Ok(())
    }
}

/// Try to parse a single stdout line into a `Message`.
pub fn parse_line(line: &str) -> Result<Message, ParseError> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(ParseError::Invalid)?;
    let obj = value.as_object().ok_or(ParseError::NotObject)?;
    let method = obj
        .get("method")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let id = obj.get("id").cloned();

    if let (Some(method), Some(id)) = (method.clone(), id.clone()) {
        // Server → client request (both method + id).
        let params = obj
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        return Ok(Message::Request { id, method, params });
    }
    if let Some(method) = method {
        // Notification (method only, no id).
        let params = obj
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        return Ok(Message::Notification { method, params });
    }
    if let Some(id) = id {
        // Response (id + result/error).
        let outcome = if let Some(err) = obj.get("error") {
            let err: JsonRpcError = serde_json::from_value(err.clone())
                .map_err(|e| ParseError::BadError(e.to_string()))?;
            Err(err)
        } else {
            let result = obj
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Ok(result)
        };
        return Ok(Message::Response { id, outcome });
    }
    Err(ParseError::Indeterminate)
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("invalid JSON: {0}")]
    Invalid(serde_json::Error),
    #[error("message not a JSON object")]
    NotObject,
    #[error("message has no id or method")]
    Indeterminate,
    #[error("invalid error payload: {0}")]
    BadError(String),
}

/// Spawn a task that reads stdout line-by-line and invokes `handler` for each
/// parsed message. Returns when EOF is reached or the handler errors.
pub fn spawn_reader<F>(
    reader: BufReader<ChildStdout>,
    mut handler: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnMut(Message) + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = reader.lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => match parse_line(&line) {
                    Ok(msg) => handler(msg),
                    Err(ParseError::Invalid(_)) if line.trim().is_empty() => {}
                    Err(err) => {
                        warn!(error = %err, line = %line, "dropping unparseable app-server line");
                    }
                },
                Ok(None) => {
                    debug!("app-server stdout EOF");
                    break;
                }
                Err(err) => {
                    warn!(error = %err, "error reading app-server stdout");
                    break;
                }
            }
        }
    })
}

/// Spawn a task that drains stderr into tracing (one line per record).
pub fn spawn_stderr_logger(stderr: ChildStderr) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.contains("ERROR") {
                warn!(target: "codex_app_server", "{}", trimmed);
            } else {
                debug!(target: "codex_app_server", "{}", trimmed);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_response_without_jsonrpc_field() {
        let m = parse_line(r#"{"id":1,"result":{"ok":true}}"#).unwrap();
        match m {
            Message::Response { id, outcome } => {
                assert_eq!(id, serde_json::json!(1));
                assert!(outcome.is_ok());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_notification() {
        let m = parse_line(r#"{"method":"turn/started","params":{"threadId":"t"}}"#).unwrap();
        match m {
            Message::Notification { method, params } => {
                assert_eq!(method, "turn/started");
                assert_eq!(params["threadId"], "t");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_server_request_with_id() {
        let m = parse_line(
            r#"{"id":0,"method":"item/commandExecution/requestApproval","params":{"threadId":"t"}}"#,
        )
        .unwrap();
        match m {
            Message::Request { id, method, .. } => {
                assert_eq!(id, serde_json::json!(0));
                assert_eq!(method, "item/commandExecution/requestApproval");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn rejects_non_object() {
        let err = parse_line("[1,2,3]").unwrap_err();
        assert!(matches!(err, ParseError::NotObject));
    }

    #[test]
    fn rejects_indeterminate() {
        let err = parse_line("{}").unwrap_err();
        assert!(matches!(err, ParseError::Indeterminate));
    }
}
