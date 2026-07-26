use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use tracing::{info, warn};

use crate::memory::store::MemoryStore;

pub(crate) mod memory;
pub(crate) mod prompt;
pub(crate) mod runner;

pub use memory::ShadowConfig;

pub(crate) use memory::{ShadowContext, memory_threshold_met};

pub struct ShadowWorker {
    memory: Arc<MemoryStore>,
    codex_binary: String,
    codex_home: PathBuf,
    workspace_dir: PathBuf,
    memory_config: ShadowConfig,
    in_flight: Mutex<HashSet<String>>,
}

impl ShadowWorker {
    pub fn new(
        memory: Arc<MemoryStore>,
        codex_binary: String,
        codex_home: PathBuf,
        workspace_dir: PathBuf,
        memory_config: ShadowConfig,
    ) -> Self {
        Self {
            memory,
            codex_binary,
            codex_home,
            workspace_dir,
            memory_config,
            in_flight: Mutex::new(HashSet::new()),
        }
    }

    pub(crate) fn spawn_memory(self: &Arc<Self>, ctx: ShadowContext) {
        let worker = self.clone();
        tokio::spawn(async move {
            if let Err(err) = worker.run_memory(ctx).await {
                warn!(error = %err, "shadow memory task failed");
            }
        });
    }

    async fn run_memory(&self, ctx: ShadowContext) -> Result<()> {
        if !memory_threshold_met(&ctx, &self.memory_config) {
            return Ok(());
        }
        let key = format!("mem:{}", ctx.openid);
        if !self.try_acquire(&key) {
            return Ok(());
        }
        let outcome = self.inner_memory_shadow(&ctx).await;
        self.release(&key);
        outcome
    }

    fn try_acquire(&self, key: &str) -> bool {
        let mut guard = self.in_flight.lock().expect("shadow in_flight poisoned");
        guard.insert(key.to_string())
    }

    fn release(&self, key: &str) {
        let mut guard = self.in_flight.lock().expect("shadow in_flight poisoned");
        guard.remove(key);
    }

    async fn inner_memory_shadow(&self, ctx: &ShadowContext) -> Result<()> {
        let snapshot = {
            let memory = self.memory.clone();
            let openid = ctx.openid.clone();
            tokio::task::spawn_blocking(move || memory.snapshot_for(&openid)).await??
        };
        let prompt_text = prompt::render_memory_prompt(
            &snapshot.memory,
            &snapshot.user,
            &ctx.last_user_text,
            &ctx.last_assistant_text,
        );
        let oneshot = runner::OneshotConfig {
            codex_binary: &self.codex_binary,
            workspace_dir: &self.workspace_dir,
            codex_home: &self.codex_home,
            model: self.memory_config.model_override.as_deref(),
            reasoning: Some(&self.memory_config.reasoning),
            prompt: &prompt_text,
            deadline: self.memory_config.deadline,
        };
        let output = runner::run_codex_oneshot(oneshot).await?;
        let response = memory::parse_memory_response(&output)?;
        // apply_memory_response writes + fsyncs to disk; keep it off the reactor.
        let report = {
            let memory = self.memory.clone();
            let openid = ctx.openid.clone();
            tokio::task::spawn_blocking(move || {
                memory::apply_memory_response(&memory, &openid, &response)
            })
            .await??
        };
        info!(
            openid = %ctx.openid,
            added = report.added,
            duplicate = report.duplicate,
            rejected = report.rejected,
            over_budget = report.over_budget,
            too_long = report.too_long,
            "shadow memory applied"
        );
        Ok(())
    }
}
