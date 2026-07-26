//! Renders a stream of [`ExecutionUpdate`]s into QQ passive replies: tool
//! summaries, agent text, and the attachments requested by a ```` ```qqbot ````
//! block (see [`super::directive`]).

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    codex::ExecutionUpdate,
    qq::{
        api::{QqApiClient, estimate_text_chunk_count},
        directive::{Directive, parse_output},
    },
    util::text::strip_end_signal,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ToolSummary {
    display: String,
    count: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PassiveDispatchReport {
    pub(crate) sent_replies: usize,
    pub(crate) saw_agent_message: bool,
    pub(crate) tool_call_count: usize,
    /// The turn's codex thread id, captured from the update stream as soon as
    /// the thread is established — available even when the turn is interrupted
    /// or fails before completing, so the caller can still persist it.
    pub(crate) session_id: Option<String>,
}

pub(crate) struct PassiveTurnEmitter {
    qq_client: Arc<QqApiClient>,
    openid: String,
    message_id: String,
    workspace_dir: PathBuf,
    verbose: bool,
    strip_signal: Option<String>,
    sent_replies: usize,
    saw_agent_message: bool,
    tool_call_count: usize,
    pending_tools: Vec<ToolSummary>,
    session_id: Option<String>,
}

impl PassiveTurnEmitter {
    pub(crate) fn new(
        qq_client: Arc<QqApiClient>,
        openid: String,
        message_id: String,
        workspace_dir: PathBuf,
        verbose: bool,
    ) -> Self {
        Self {
            qq_client,
            openid,
            message_id,
            workspace_dir,
            verbose,
            strip_signal: None,
            sent_replies: 0,
            saw_agent_message: false,
            tool_call_count: 0,
            pending_tools: Vec::new(),
            session_id: None,
        }
    }

    pub(crate) fn with_strip_signal(mut self, signal: Option<String>) -> Self {
        self.strip_signal = signal;
        self
    }

    /// Always returns the report, even when a streamed send fails: the report
    /// carries state the caller must not lose (notably the thread id from
    /// SessionStarted, needed to persist an interrupted turn). The first send
    /// error is returned alongside; after it, remaining updates are still
    /// drained for their state but nothing more is sent.
    pub(crate) async fn run(
        mut self,
        mut updates: mpsc::UnboundedReceiver<ExecutionUpdate>,
    ) -> (PassiveDispatchReport, Option<anyhow::Error>) {
        let mut send_error = None;
        while let Some(update) = updates.recv().await {
            match update {
                ExecutionUpdate::SessionStarted { session_id } => {
                    self.session_id = Some(session_id)
                }
                ExecutionUpdate::ToolCall { display } => self.record_tool(display),
                ExecutionUpdate::AgentMessage { text } => {
                    if send_error.is_some() {
                        continue;
                    }
                    if let Err(err) = self.handle_agent_message(text).await {
                        send_error = Some(err);
                    }
                }
            }
        }
        if send_error.is_none()
            && let Err(err) = self.flush_tail().await
        {
            send_error = Some(err);
        }
        (
            PassiveDispatchReport {
                sent_replies: self.sent_replies,
                saw_agent_message: self.saw_agent_message,
                tool_call_count: self.tool_call_count,
                session_id: self.session_id.clone(),
            },
            send_error,
        )
    }

    fn record_tool(&mut self, display: String) {
        self.tool_call_count += 1;
        let display = if self.verbose {
            display
        } else {
            compact_tool_display(&display)
        };
        match self.pending_tools.last_mut() {
            Some(last) if last.display == display => last.count += 1,
            _ => self.pending_tools.push(ToolSummary { display, count: 1 }),
        }
    }

    async fn handle_agent_message(&mut self, raw_text: String) -> Result<()> {
        self.saw_agent_message = true;
        let raw_text = if let Some(signal) = self.strip_signal.as_deref() {
            strip_end_signal(&raw_text, signal).0
        } else {
            raw_text
        };
        let parsed = parse_output(&raw_text, &self.workspace_dir);
        let text = parsed.text.trim().to_string();

        if !self.pending_tools.is_empty() {
            let tool_block = format_tool_block(&self.pending_tools);
            self.pending_tools.clear();
            if !tool_block.is_empty() {
                self.send_text_block(&tool_block).await?;
            }
        }

        if !text.is_empty() {
            self.send_text_block(&text).await?;
        }

        for directive in parsed.directives {
            self.send_directive(directive).await?;
        }
        Ok(())
    }

    async fn flush_tail(&mut self) -> Result<()> {
        if !self.pending_tools.is_empty() {
            let tool_block = format_tool_block(&self.pending_tools);
            self.pending_tools.clear();
            if !tool_block.is_empty() {
                self.send_text_block(&tool_block).await?;
            }
        }
        Ok(())
    }

    async fn send_text_block(&mut self, text: &str) -> Result<()> {
        let chunks = estimate_text_chunk_count(text);
        self.qq_client
            .send_text(&self.openid, &self.message_id, text, Some(&self.message_id))
            .await?;
        self.sent_replies += chunks;
        info!(
            sent_replies = self.sent_replies,
            "sent qq passive text block"
        );
        Ok(())
    }

    async fn send_directive(&mut self, directive: Directive) -> Result<()> {
        match directive {
            Directive::Image { path } => {
                let info = self
                    .qq_client
                    .upload_file(&self.openid, &path, 1, None)
                    .await?;
                self.qq_client
                    .send_media(&self.openid, &self.message_id, &info)
                    .await?;
            }
            Directive::File { path, name } => {
                let info = self
                    .qq_client
                    .upload_file(&self.openid, &path, 4, name.as_deref())
                    .await?;
                self.qq_client
                    .send_media(&self.openid, &self.message_id, &info)
                    .await?;
            }
        }
        self.sent_replies += 1;
        Ok(())
    }
}

fn format_tool_block(tools: &[ToolSummary]) -> String {
    tools
        .iter()
        .filter(|tool| tool.count > 0)
        .map(|tool| {
            if tool.count == 1 {
                tool.display.clone()
            } else if let Some((first_line, rest)) = tool.display.split_once('\n') {
                format!("{first_line} ×{}\n{rest}", tool.count)
            } else {
                format!("{} ×{}", tool.display, tool.count)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn compact_tool_display(display: &str) -> String {
    let trimmed = display.trim();
    match trimmed.find(']') {
        Some(index) if trimmed.starts_with('[') => trimmed[..=index].to_string(),
        _ => trimmed
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use super::{ToolSummary, compact_tool_display, format_tool_block};
    use crate::{config::QqConfig, qq::api::QqApiClient};

    /// Client with empty credentials pointed at an unreachable host: these
    /// tests never let the emitter reach the network.
    fn offline_client() -> Arc<QqApiClient> {
        Arc::new(
            QqApiClient::new(QqConfig {
                app_id: String::new(),
                app_secret: String::new(),
                api_base_url: "https://example.com".to_string(),
                token_url: "https://example.com/token".to_string(),
            })
            .unwrap(),
        )
    }

    #[test]
    fn formats_repeated_tool_runs() {
        let block = format_tool_block(&[
            ToolSummary {
                display: "[🖥️ Bash]\n```shell\npwd\n```".to_string(),
                count: 2,
            },
            ToolSummary {
                display: "[🔍 Web Search] rust async await".to_string(),
                count: 1,
            },
        ]);
        assert_eq!(
            block,
            "[🖥️ Bash] ×2\n```shell\npwd\n```\n[🔍 Web Search] rust async await"
        );
    }

    #[test]
    fn compacts_tool_display_to_label() {
        assert_eq!(
            compact_tool_display("[🖥️ Bash]\n```shell\npwd\n```"),
            "[🖥️ Bash]"
        );
        assert_eq!(
            compact_tool_display("[🔍 Web Search] rust async await"),
            "[🔍 Web Search]"
        );
        assert_eq!(
            compact_tool_display("[Thinking]\n检查日志中断点"),
            "[Thinking]"
        );
    }

    #[tokio::test]
    async fn run_captures_session_id_from_session_started() {
        use crate::codex::ExecutionUpdate;
        use tokio::sync::mpsc;

        let client = offline_client();
        let emitter = super::PassiveTurnEmitter::new(
            client,
            "u".to_string(),
            "m".to_string(),
            PathBuf::from("/tmp"),
            false,
        );
        let (tx, rx) = mpsc::unbounded_channel();
        // SessionStarted must be captured even when the turn produces no agent
        // message (interrupted / failed mid-flight): no network send happens.
        tx.send(ExecutionUpdate::SessionStarted {
            session_id: "thread-xyz".to_string(),
        })
        .unwrap();
        drop(tx);
        let (report, send_error) = emitter.run(rx).await;
        assert!(send_error.is_none());
        assert_eq!(report.session_id.as_deref(), Some("thread-xyz"));
    }

    #[test]
    fn record_tool_tracks_total_tool_call_count() {
        let client = offline_client();
        let mut emitter = super::PassiveTurnEmitter::new(
            client,
            "u".to_string(),
            "m".to_string(),
            PathBuf::from("/tmp"),
            false,
        );
        emitter.record_tool("[🖥️ Bash]\n```shell\npwd\n```".to_string());
        emitter.record_tool("[🖥️ Bash]\n```shell\nls\n```".to_string());

        assert_eq!(emitter.tool_call_count, 2);
    }
}
