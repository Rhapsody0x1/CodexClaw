use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    codex::{
        executor::ExecutionUpdate,
        output::{Directive, parse_output},
    },
    qq::api::{QqApiClient, estimate_text_chunk_count},
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ToolSummary {
    display: String,
    count: u32,
}

#[derive(Debug, Clone, Default)]
pub struct PassiveDispatchReport {
    pub sent_replies: usize,
    pub saw_agent_message: bool,
    pub tool_call_count: usize,
}

pub struct PassiveTurnEmitter {
    qq_client: Arc<QqApiClient>,
    openid: String,
    message_id: String,
    workspace_dir: PathBuf,
    verbose: bool,
    sent_replies: usize,
    saw_agent_message: bool,
    tool_call_count: usize,
    pending_tools: Vec<ToolSummary>,
}

impl PassiveTurnEmitter {
    pub fn new(
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
            sent_replies: 0,
            saw_agent_message: false,
            tool_call_count: 0,
            pending_tools: Vec::new(),
        }
    }

    pub async fn run(
        mut self,
        mut updates: mpsc::UnboundedReceiver<ExecutionUpdate>,
    ) -> Result<PassiveDispatchReport> {
        while let Some(update) = updates.recv().await {
            match update {
                ExecutionUpdate::ToolCall { display } => self.record_tool(display),
                ExecutionUpdate::AgentMessage { text } => self.handle_agent_message(text).await?,
            }
        }
        self.flush_tail().await?;
        Ok(PassiveDispatchReport {
            sent_replies: self.sent_replies,
            saw_agent_message: self.saw_agent_message,
            tool_call_count: self.tool_call_count,
        })
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
                format!("{first_line} * {}\n{rest}", tool.count)
            } else {
                format!("{} * {}", tool.display, tool.count)
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

    #[test]
    fn formats_repeated_tool_runs() {
        let block = format_tool_block(&[
            ToolSummary {
                display: "[Tool: Bash]\n```shell\npwd\n```".to_string(),
                count: 2,
            },
            ToolSummary {
                display: "[Tool: Web Search] rust async await".to_string(),
                count: 1,
            },
        ]);
        assert_eq!(
            block,
            "[Tool: Bash] * 2\n```shell\npwd\n```\n[Tool: Web Search] rust async await"
        );
    }

    #[test]
    fn compacts_tool_display_to_label() {
        assert_eq!(
            compact_tool_display("[Tool: Bash]\n```shell\npwd\n```"),
            "[Tool: Bash]"
        );
        assert_eq!(
            compact_tool_display("[Tool: Web Search] rust async await"),
            "[Tool: Web Search]"
        );
        assert_eq!(
            compact_tool_display("[Thinking]\n检查日志中断点"),
            "[Thinking]"
        );
    }

    #[test]
    fn record_tool_tracks_total_tool_call_count() {
        let client = Arc::new(
            QqApiClient::new(QqConfig {
                app_id: String::new(),
                app_secret: String::new(),
                api_base_url: "https://example.com".to_string(),
                token_url: "https://example.com/token".to_string(),
            })
            .unwrap(),
        );
        let mut emitter = super::PassiveTurnEmitter::new(
            client,
            "u".to_string(),
            "m".to_string(),
            PathBuf::from("/tmp"),
            false,
        );
        emitter.record_tool("[Tool: Bash]\n```shell\npwd\n```".to_string());
        emitter.record_tool("[Tool: Bash]\n```shell\nls\n```".to_string());

        assert_eq!(emitter.tool_call_count, 2);
    }
}
