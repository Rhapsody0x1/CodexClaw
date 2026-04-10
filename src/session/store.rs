use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::sync::RwLock;

use crate::session::state::SessionState;

pub struct SessionStore {
    root: PathBuf,
    state_path: PathBuf,
    workspace_dir: PathBuf,
    inbox_dir: PathBuf,
    state: RwLock<SessionState>,
}

impl SessionStore {
    pub async fn load_or_init(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join("session").join("main");
        let workspace_dir = root.join("workspace");
        let inbox_dir = workspace_dir.join("inbox");
        tokio::fs::create_dir_all(&inbox_dir).await?;
        let state_path = root.join("settings.json");
        let state = match tokio::fs::read_to_string(&state_path).await {
            Ok(raw) => serde_json::from_str::<SessionState>(&raw)
                .with_context(|| format!("failed to parse {}", state_path.display()))?,
            Err(_) => SessionState::default(),
        };
        let store = Self {
            root,
            state_path,
            workspace_dir,
            inbox_dir,
            state: RwLock::new(state),
        };
        store.persist().await?;
        Ok(store)
    }

    pub async fn snapshot(&self) -> SessionState {
        self.state.read().await.clone()
    }

    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    pub fn inbox_dir(&self) -> &Path {
        &self.inbox_dir
    }

    pub async fn update_settings<F>(&self, mutator: F) -> Result<SessionState>
    where
        F: FnOnce(&mut SessionState),
    {
        let mut state = self.state.write().await;
        mutator(&mut state);
        let snapshot = state.clone();
        drop(state);
        self.persist().await?;
        Ok(snapshot)
    }

    pub async fn set_session_id(&self, session_id: Option<String>) -> Result<SessionState> {
        self.update_settings(|state| state.session_id = session_id)
            .await
    }

    pub async fn reset_for_new_session(&self) -> Result<SessionState> {
        self.update_settings(|state| {
            state.session_id = None;
        })
        .await
    }

    async fn persist(&self) -> Result<()> {
        let raw = serde_json::to_string_pretty(&*self.state.read().await)?;
        tokio::fs::write(&self.state_path, raw)
            .await
            .with_context(|| format!("failed to write {}", self.state_path.display()))?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::session::state::ReasoningEffort;

    use super::SessionStore;

    #[tokio::test]
    async fn persists_settings_and_reset() {
        let dir = tempdir().unwrap();
        let store = SessionStore::load_or_init(dir.path()).await.unwrap();
        store
            .update_settings(|state| {
                state.session_id = Some("thread-1".into());
                state.settings.model_override = Some("gpt-test".into());
                state.settings.reasoning_effort = Some(ReasoningEffort::High);
                state.settings.plan_mode = true;
            })
            .await
            .unwrap();
        let snapshot = store.snapshot().await;
        assert_eq!(snapshot.session_id.as_deref(), Some("thread-1"));
        assert_eq!(
            snapshot.settings.model_override.as_deref(),
            Some("gpt-test")
        );
        assert_eq!(
            snapshot.settings.reasoning_effort,
            Some(ReasoningEffort::High)
        );
        assert!(snapshot.settings.plan_mode);

        let reset = store.reset_for_new_session().await.unwrap();
        assert!(reset.session_id.is_none());
        assert_eq!(reset.settings.model_override.as_deref(), Some("gpt-test"));
        assert_eq!(
            reset.settings.reasoning_effort,
            Some(ReasoningEffort::High)
        );
        assert!(reset.settings.plan_mode);
    }
}
