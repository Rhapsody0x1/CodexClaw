use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use tracing::{info, warn};

use crate::memory::store::MemoryStore;
use crate::skills::index::SkillIndex;

pub mod memory;
pub mod prompt;
pub mod runner;
pub mod skill;

pub use memory::{ShadowConfig, ShadowContext, memory_threshold_met};
pub use skill::{SkillShadowConfig, skill_threshold_met};

pub struct ShadowWorker {
    memory: Arc<MemoryStore>,
    skill_index: Arc<SkillIndex>,
    skills_root: PathBuf,
    codex_binary: String,
    codex_home: PathBuf,
    workspace_dir: PathBuf,
    memory_config: ShadowConfig,
    skill_config: SkillShadowConfig,
    in_flight: Mutex<HashSet<String>>,
}

impl ShadowWorker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        memory: Arc<MemoryStore>,
        skill_index: Arc<SkillIndex>,
        skills_root: PathBuf,
        codex_binary: String,
        codex_home: PathBuf,
        workspace_dir: PathBuf,
        memory_config: ShadowConfig,
        skill_config: SkillShadowConfig,
    ) -> Self {
        Self {
            memory,
            skill_index,
            skills_root,
            codex_binary,
            codex_home,
            workspace_dir,
            memory_config,
            skill_config,
            in_flight: Mutex::new(HashSet::new()),
        }
    }

    pub fn spawn_memory(self: &Arc<Self>, ctx: ShadowContext) {
        let worker = self.clone();
        tokio::spawn(async move {
            if let Err(err) = worker.run_memory(ctx).await {
                warn!(error = %err, "shadow memory task failed");
            }
        });
    }

    pub fn spawn_skill(self: &Arc<Self>, ctx: ShadowContext) {
        let worker = self.clone();
        tokio::spawn(async move {
            if let Err(err) = worker.run_skill(ctx).await {
                warn!(error = %err, "shadow skill task failed");
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

    async fn run_skill(&self, ctx: ShadowContext) -> Result<()> {
        if !skill_threshold_met(&ctx, &self.skill_config) {
            return Ok(());
        }
        let key = format!("skill:{}", ctx.openid);
        if !self.try_acquire(&key) {
            return Ok(());
        }
        let outcome = self.inner_skill_shadow(&ctx).await;
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
        let snapshot = self.memory.snapshot_for(&ctx.openid)?;
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
        let report = memory::apply_memory_response(&self.memory, &ctx.openid, &response)?;
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

    async fn inner_skill_shadow(&self, ctx: &ShadowContext) -> Result<()> {
        let existing = self.skill_index.list_claw().unwrap_or_default();
        let hints = skill::existing_skill_hints(&existing);
        let prompt_text =
            prompt::render_skill_prompt(&hints, &ctx.last_user_text, &ctx.last_assistant_text);
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
        let response = skill::parse_skill_response(&output)?;
        let report = skill::apply_skill_response(&self.skills_root, &self.skill_index, &response);
        info!(
            openid = %ctx.openid,
            created = ?report.created,
            skipped_none = report.skipped_none,
            skipped_invalid_slug = report.skipped_invalid_slug,
            skipped_validation = ?report.skipped_validation,
            write_error = ?report.write_error,
            "shadow skill applied"
        );
        Ok(())
    }
}
