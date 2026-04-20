use std::{
    collections::{BTreeMap, HashMap},
    fs::{File, OpenOptions},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use rand::{Rng, seq::SliceRandom};
use tokio::sync::RwLock;

use crate::session::state::{
    CommandAlias, ContextMode, DialogOrigin, DialogProfile, DialogState, ImportedSessionProfile,
    PendingSetting, PersistedSessionState, ReasoningEffort, ServiceTier, SessionSettings,
    SessionState, TokenUsageSnapshot, UserSessionState,
};

const ALIAS_WORDS: &[&str] = &[
    "sage", "oak", "mint", "lark", "wave", "nova", "reef", "kite", "fern", "dawn", "ember",
    "cedar", "sprout", "peak", "ridge", "orbit", "pixel", "frost", "drift", "meadow", "echo",
    "river", "flint", "atlas", "bloom", "cloud", "maple", "cobalt", "quill", "harbor",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionListScope {
    All,
    Local,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskSessionMeta {
    pub id: String,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub last_user_message: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
    pub origin: DialogOrigin,
    pub rollout_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SwitchResult {
    pub parked_alias: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StopResult {
    pub had_session: bool,
    pub saved: bool,
    pub dropped_unsaved: bool,
    pub restored_alias: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImportResult {
    pub copied: bool,
    pub profile: ImportedSessionProfile,
}

pub struct SessionStore {
    root: PathBuf,
    state_path: PathBuf,
    attachment_workspace_dir: PathBuf,
    inbox_dir: PathBuf,
    global_codex_home: PathBuf,
    system_codex_home: PathBuf,
    default_workspace_dir: PathBuf,
    state: RwLock<PersistedSessionState>,
}

impl SessionStore {
    pub async fn load_or_init(
        data_dir: &Path,
        global_codex_home: &Path,
        system_codex_home: &Path,
        _default_workspace_dir: &Path,
    ) -> Result<Self> {
        let root = data_dir.join("session");
        let attachment_workspace_dir = root.join("workspace");
        let inbox_dir = attachment_workspace_dir.join("inbox");
        tokio::fs::create_dir_all(&inbox_dir).await?;
        tokio::fs::create_dir_all(global_codex_home.join("sessions")).await?;
        let state_path = root.join("state.json");
        let state = match tokio::fs::read_to_string(&state_path).await {
            Ok(raw) => serde_json::from_str::<PersistedSessionState>(&raw)
                .with_context(|| format!("failed to parse {}", state_path.display()))?,
            Err(_) => load_legacy_state(data_dir, &attachment_workspace_dir)?,
        };
        let store = Self {
            root,
            state_path,
            attachment_workspace_dir: attachment_workspace_dir.clone(),
            inbox_dir,
            global_codex_home: global_codex_home.to_path_buf(),
            system_codex_home: system_codex_home.to_path_buf(),
            // Keep the shared attachment workspace as the default temporary workspace root.
            default_workspace_dir: attachment_workspace_dir,
            state: RwLock::new(state),
        };
        store.persist().await?;
        Ok(store)
    }

    fn new_temporary_dialog(&self) -> Result<DialogState> {
        let workspace_dir = prepare_workspace_dir(&self.attachment_workspace_dir)?;
        Ok(DialogState::new_temporary(workspace_dir))
    }

    fn temporary_dialog_for_workspace(&self, workspace_dir: &Path) -> Result<DialogState> {
        let workspace_dir = prepare_workspace_dir(workspace_dir)?;
        Ok(DialogState::new_temporary(workspace_dir))
    }

    pub async fn snapshot_for_user(&self, openid: &str) -> Result<UserSessionState> {
        if let Some(snapshot) = self.state.read().await.users.get(openid).cloned() {
            return Ok(snapshot);
        }
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            Ok(user.clone())
        })
        .await
    }

    pub async fn foreground_runtime_state(&self, openid: &str) -> Result<SessionState> {
        let user = self.snapshot_for_user(openid).await?;
        Ok(SessionState {
            session_id: user.foreground.session_id.clone(),
            settings: user
                .settings
                .merged_with_profile(user.foreground.profile.as_ref()),
        })
    }

    pub async fn update_settings_for_user<F>(
        &self,
        openid: &str,
        mutator: F,
    ) -> Result<UserSessionState>
    where
        F: FnOnce(&mut SessionSettings),
    {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            mutator(&mut user.settings);
            Ok(user.clone())
        })
        .await
    }

    pub async fn set_model_override_for_active(
        &self,
        openid: &str,
        value: Option<String>,
    ) -> Result<UserSessionState> {
        self.mutate_state(|state| {
            let (snapshot, cached_profile) = {
                let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
                if user.foreground.is_temporary() {
                    user.settings.model_override = value.clone();
                } else {
                    let profile = user
                        .foreground
                        .profile
                        .get_or_insert_with(DialogProfile::default);
                    profile.model_override = value.clone();
                }
                let snapshot = user.clone();
                let cached_profile = cached_profile_from_dialog(&snapshot.foreground);
                (snapshot, cached_profile)
            };
            persist_cached_profile(state, cached_profile);
            Ok(snapshot)
        })
        .await
    }

    pub async fn set_service_tier_for_active(
        &self,
        openid: &str,
        value: Option<ServiceTier>,
    ) -> Result<UserSessionState> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            user.settings.service_tier = value;
            Ok(user.clone())
        })
        .await
    }

    pub async fn set_context_mode_for_active(
        &self,
        openid: &str,
        value: Option<ContextMode>,
    ) -> Result<UserSessionState> {
        self.mutate_state(|state| {
            let (snapshot, cached_profile) = {
                let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
                if user.foreground.is_temporary() {
                    user.settings.context_mode = value;
                } else {
                    let profile = user
                        .foreground
                        .profile
                        .get_or_insert_with(DialogProfile::default);
                    profile.context_mode = value;
                }
                let snapshot = user.clone();
                let cached_profile = cached_profile_from_dialog(&snapshot.foreground);
                (snapshot, cached_profile)
            };
            persist_cached_profile(state, cached_profile);
            Ok(snapshot)
        })
        .await
    }

    pub async fn set_reasoning_for_active(
        &self,
        openid: &str,
        value: Option<ReasoningEffort>,
    ) -> Result<UserSessionState> {
        self.mutate_state(|state| {
            let (snapshot, cached_profile) = {
                let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
                if user.foreground.is_temporary() {
                    user.settings.reasoning_effort = value;
                } else {
                    let profile = user
                        .foreground
                        .profile
                        .get_or_insert_with(DialogProfile::default);
                    profile.reasoning_effort = value;
                }
                let snapshot = user.clone();
                let cached_profile = cached_profile_from_dialog(&snapshot.foreground);
                (snapshot, cached_profile)
            };
            persist_cached_profile(state, cached_profile);
            Ok(snapshot)
        })
        .await
    }

    pub async fn bind_foreground_session_profile(
        &self,
        openid: &str,
        session_id: Option<String>,
        profile: DialogProfile,
    ) -> Result<UserSessionState> {
        self.mutate_state(|state| {
            let (snapshot, cached_profile) = {
                let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
                user.foreground.session_id = session_id;
                user.foreground.profile =
                    user.foreground.session_id.as_ref().map(|_| profile.clone());
                let snapshot = user.clone();
                let cached_profile = cached_profile_from_dialog(&snapshot.foreground);
                (snapshot, cached_profile)
            };
            persist_cached_profile(state, cached_profile);
            Ok(snapshot)
        })
        .await
    }

    pub async fn set_foreground_session_id(
        &self,
        openid: &str,
        session_id: Option<String>,
    ) -> Result<UserSessionState> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            user.foreground.session_id = session_id;
            Ok(user.clone())
        })
        .await
    }

    pub async fn set_foreground_usage(
        &self,
        openid: &str,
        usage: TokenUsageSnapshot,
    ) -> Result<()> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            user.foreground.last_usage = Some(usage);
            Ok(())
        })
        .await
    }

    pub async fn set_pending_setting(
        &self,
        openid: &str,
        pending: Option<PendingSetting>,
    ) -> Result<()> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            user.pending_setting = pending;
            Ok(())
        })
        .await
    }

    pub async fn add_command_alias(
        &self,
        openid: &str,
        alias: CommandAlias,
    ) -> Result<CommandAlias> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            user.command_aliases
                .insert(alias.name.clone(), alias.clone());
            Ok(alias)
        })
        .await
    }

    pub async fn remove_command_alias(&self, openid: &str, name: &str) -> Result<bool> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            Ok(user.command_aliases.remove(name).is_some())
        })
        .await
    }

    pub async fn get_command_alias(
        &self,
        openid: &str,
        name: &str,
    ) -> Result<Option<CommandAlias>> {
        let state = self.state.read().await;
        Ok(state
            .users
            .get(openid)
            .and_then(|user| user.command_aliases.get(name).cloned()))
    }

    pub async fn list_command_aliases(&self, openid: &str) -> Result<Vec<CommandAlias>> {
        let state = self.state.read().await;
        Ok(state
            .users
            .get(openid)
            .map(|user| user.command_aliases.values().cloned().collect())
            .unwrap_or_default())
    }

    pub async fn new_foreground(&self, openid: &str) -> Result<SwitchResult> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            let parked_alias = park_foreground(user, None, &self.attachment_workspace_dir, || {
                self.new_temporary_dialog()
            })?;
            Ok(SwitchResult { parked_alias })
        })
        .await
    }

    pub async fn new_foreground_in_workspace(
        &self,
        openid: &str,
        workspace_dir: &Path,
    ) -> Result<SwitchResult> {
        let workspace_dir = workspace_dir.to_path_buf();
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            let parked_alias = park_foreground(user, None, &self.attachment_workspace_dir, || {
                self.temporary_dialog_for_workspace(&workspace_dir)
            })?;
            Ok(SwitchResult { parked_alias })
        })
        .await
    }

    pub async fn move_foreground_to_background(
        &self,
        openid: &str,
        requested_alias: Option<&str>,
    ) -> Result<SwitchResult> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            let parked_alias = park_foreground(
                user,
                requested_alias,
                &self.attachment_workspace_dir,
                || self.new_temporary_dialog(),
            )?;
            Ok(SwitchResult { parked_alias })
        })
        .await
    }

    pub async fn foreground_from_background(
        &self,
        openid: &str,
        alias: &str,
    ) -> Result<SwitchResult> {
        let mut target = {
            let guard = self.state.read().await;
            guard
                .users
                .get(openid)
                .and_then(|user| user.background.get(alias))
                .cloned()
        }
        .ok_or_else(|| anyhow!("后台会话 `{alias}` 不存在"))?;
        let target_profile = resolve_profile_for_dialog(&self.global_codex_home, Some(&target))?;
        if let Some(profile) = target_profile.clone() {
            target.workspace_dir = profile.workspace_dir.clone();
            target.profile = Some(profile.dialog_profile());
        };
        self.mutate_state(|state| {
            if let Some(profile) = target_profile.clone()
                && let Some(session_id) = target.session_id.clone()
            {
                state.imported_profiles.insert(session_id, profile);
            }
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            user.background
                .remove(alias)
                .ok_or_else(|| anyhow!("后台会话 `{alias}` 不存在"))?;
            remove_background_alias(user, alias);
            let parked_alias = park_foreground(user, None, &self.attachment_workspace_dir, || {
                self.new_temporary_dialog()
            })?;
            user.foreground = target.clone();
            Ok(SwitchResult { parked_alias })
        })
        .await
    }

    pub async fn resume_disk_session(
        &self,
        openid: &str,
        target: &DiskSessionMeta,
    ) -> Result<SwitchResult> {
        let resolved_profile = self
            .state
            .read()
            .await
            .imported_profiles
            .get(&target.id)
            .cloned()
            .or(extract_session_profile(
                &target.rollout_path,
                target.cwd.clone(),
            )?);
        self.mutate_state(|state| {
            if let Some(profile) = resolved_profile.clone() {
                state
                    .imported_profiles
                    .entry(target.id.clone())
                    .or_insert(profile);
            }
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            let parked_alias = park_foreground(user, None, &self.attachment_workspace_dir, || {
                self.new_temporary_dialog()
            })?;
            user.foreground = DialogState {
                session_id: Some(target.id.clone()),
                origin: target.origin,
                workspace_dir: resolved_profile
                    .as_ref()
                    .map(|value| value.workspace_dir.clone())
                    .unwrap_or_else(|| target.cwd.clone()),
                saved: true,
                profile: resolved_profile.clone().map(|value| value.dialog_profile()),
                last_usage: None,
            };
            Ok(SwitchResult { parked_alias })
        })
        .await
    }

    pub async fn load_disk_session_to_background(
        &self,
        openid: &str,
        target: &DiskSessionMeta,
        requested_alias: Option<&str>,
    ) -> Result<String> {
        let resolved_profile = self
            .state
            .read()
            .await
            .imported_profiles
            .get(&target.id)
            .cloned()
            .or(extract_session_profile(
                &target.rollout_path,
                target.cwd.clone(),
            )?);
        self.mutate_state(|state| {
            if let Some(profile) = resolved_profile.clone() {
                state
                    .imported_profiles
                    .entry(target.id.clone())
                    .or_insert(profile);
            }
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            let alias = pick_alias(user, requested_alias)?;
            user.background.insert(
                alias.clone(),
                DialogState {
                    session_id: Some(target.id.clone()),
                    origin: target.origin,
                    workspace_dir: resolved_profile
                        .as_ref()
                        .map(|value| value.workspace_dir.clone())
                        .unwrap_or_else(|| target.cwd.clone()),
                    saved: true,
                    profile: resolved_profile.clone().map(|value| value.dialog_profile()),
                    last_usage: None,
                },
            );
            record_background_alias(user, &alias);
            Ok(alias)
        })
        .await
    }

    pub async fn rename_background_alias(
        &self,
        openid: &str,
        old_alias: &str,
        new_alias: &str,
    ) -> Result<()> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            let new_alias = normalize_alias(new_alias)?;
            if user.background.contains_key(&new_alias) {
                return Err(anyhow!("标签 `{new_alias}` 已存在"));
            }
            let Some(dialog) = user.background.remove(old_alias) else {
                return Err(anyhow!("后台会话 `{old_alias}` 不存在"));
            };
            rename_background_alias_in_order(user, old_alias, &new_alias);
            user.background.insert(new_alias, dialog);
            Ok(())
        })
        .await
    }

    pub async fn save_foreground(&self, openid: &str) -> Result<bool> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            if user.foreground.saved {
                return Ok(false);
            }
            user.foreground.saved = true;
            if user.foreground.origin == DialogOrigin::Local
                && let Some(session_id) = user.foreground.session_id.clone()
            {
                register_local_saved_session(user, &session_id);
            }
            Ok(true)
        })
        .await
    }

    pub async fn stop_foreground(&self, openid: &str) -> Result<StopResult> {
        let current = self.snapshot_for_user(openid).await?.foreground;
        let had_session = current.session_id.is_some();
        let saved = current.saved;
        let dropped_unsaved =
            current.origin == DialogOrigin::Local && current.session_id.is_some() && !saved;
        let restored = {
            let guard = self.state.read().await;
            let dialog = guard.users.get(openid).and_then(|user| {
                user.background_order.iter().rev().find_map(|alias| {
                    user.background
                        .get(alias)
                        .cloned()
                        .map(|dialog| (alias.clone(), dialog))
                })
            });
            drop(guard);
            match dialog {
                Some((alias, dialog)) => {
                    let session_id = dialog.session_id.clone();
                    let profile =
                        resolve_profile_for_dialog(&self.global_codex_home, Some(&dialog))?;
                    Some((alias, session_id, profile))
                }
                None => None,
            }
        };
        if dropped_unsaved && let Some(session_id) = current.session_id.as_deref() {
            prune_session_files(&self.global_codex_home, session_id)?;
        }
        let restored_alias = self
            .mutate_state(|state| {
                if let Some((_, Some(session_id), Some(profile))) = restored.clone() {
                    state.imported_profiles.insert(session_id, profile);
                }
                let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
                if dropped_unsaved && let Some(session_id) = current.session_id.as_deref() {
                    user.saved_local_session_ids
                        .retain(|value| value != session_id);
                } else if saved
                    && current.origin == DialogOrigin::Local
                    && let Some(session_id) = current.session_id.as_deref()
                {
                    register_local_saved_session(user, session_id);
                }
                if let Some((alias, _, profile)) = restored.clone() {
                    let mut dialog = user
                        .background
                        .remove(&alias)
                        .ok_or_else(|| anyhow!("后台会话 `{alias}` 不存在"))?;
                    remove_background_alias(user, &alias);
                    if let Some(profile) = profile {
                        dialog.workspace_dir = profile.workspace_dir.clone();
                        dialog.profile = Some(profile.dialog_profile());
                    }
                    user.foreground = dialog;
                    Ok(Some(alias))
                } else {
                    user.foreground = self.new_temporary_dialog()?;
                    Ok(None)
                }
            })
            .await?;
        if current.origin == DialogOrigin::Local
            && !saved
            && current.workspace_dir != self.attachment_workspace_dir
        {
            cleanup_workspace_if_empty(&current.workspace_dir);
        }
        Ok(StopResult {
            had_session,
            saved,
            dropped_unsaved,
            restored_alias,
        })
    }

    pub async fn set_last_sessions_view(&self, openid: &str, ids: Vec<String>) -> Result<()> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            user.last_sessions_view = ids;
            Ok(())
        })
        .await
    }

    pub async fn set_last_projects_view(&self, openid: &str, ids: Vec<String>) -> Result<()> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            user.last_projects_view = ids;
            Ok(())
        })
        .await
    }

    pub async fn set_last_import_sessions_view(
        &self,
        openid: &str,
        ids: Vec<String>,
    ) -> Result<()> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            user.last_import_sessions_view = ids;
            Ok(())
        })
        .await
    }

    pub async fn set_last_import_projects_view(
        &self,
        openid: &str,
        ids: Vec<String>,
    ) -> Result<()> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            user.last_import_projects_view = ids;
            Ok(())
        })
        .await
    }

    pub async fn last_sessions_view(&self, openid: &str) -> Result<Vec<String>> {
        Ok(self.snapshot_for_user(openid).await?.last_sessions_view)
    }

    pub async fn last_projects_view(&self, openid: &str) -> Result<Vec<String>> {
        Ok(self.snapshot_for_user(openid).await?.last_projects_view)
    }

    pub async fn last_import_sessions_view(&self, openid: &str) -> Result<Vec<String>> {
        Ok(self
            .snapshot_for_user(openid)
            .await?
            .last_import_sessions_view)
    }

    pub async fn last_import_projects_view(&self, openid: &str) -> Result<Vec<String>> {
        Ok(self
            .snapshot_for_user(openid)
            .await?
            .last_import_projects_view)
    }

    pub async fn list_disk_sessions(
        &self,
        _openid: &str,
        scope: SessionListScope,
    ) -> Result<Vec<DiskSessionMeta>> {
        let mut by_id = BTreeMap::new();

        for session in scan_home_sessions(&self.global_codex_home)? {
            insert_prefer_recent(&mut by_id, session);
        }

        let mut values = by_id.into_values().collect::<Vec<_>>();
        values.retain(|session| match scope {
            SessionListScope::All => true,
            SessionListScope::Local => {
                let _ = session;
                true
            }
            SessionListScope::Global => {
                let _ = session;
                true
            }
        });
        values.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(values)
    }

    pub fn list_importable_sessions(&self) -> Result<Vec<DiskSessionMeta>> {
        let mut values = scan_home_sessions(&self.system_codex_home)?;
        values.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(values)
    }

    pub async fn import_disk_session(&self, target: &DiskSessionMeta) -> Result<ImportResult> {
        let copied = copy_session_rollout(
            &self.system_codex_home,
            &self.global_codex_home,
            &target.rollout_path,
        )?;
        copy_session_index_entry(&self.system_codex_home, &self.global_codex_home, &target.id)?;
        let profile = extract_session_profile(&target.rollout_path, target.cwd.clone())?
            .unwrap_or_else(|| ImportedSessionProfile {
                workspace_dir: target.cwd.clone(),
                ..ImportedSessionProfile::default()
            });
        self.mutate_state(|state| {
            state
                .imported_profiles
                .insert(target.id.clone(), profile.clone());
            Ok(())
        })
        .await?;
        Ok(ImportResult { copied, profile })
    }

    pub async fn imported_profile_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<ImportedSessionProfile>> {
        Ok(self
            .state
            .read()
            .await
            .imported_profiles
            .get(session_id)
            .cloned())
    }

    pub async fn import_sessions_for_workspace(&self, workspace: &Path) -> Result<usize> {
        let sessions = self.list_importable_sessions()?;
        let mut count = 0usize;
        for session in sessions {
            if session.cwd != workspace {
                continue;
            }
            let result = self.import_disk_session(&session).await?;
            if result.copied {
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn codex_home(&self) -> &Path {
        &self.global_codex_home
    }

    pub fn inbox_dir(&self) -> &Path {
        &self.inbox_dir
    }

    pub fn attachment_workspace_dir(&self) -> &Path {
        &self.attachment_workspace_dir
    }

    pub fn default_workspace_dir(&self) -> &Path {
        &self.default_workspace_dir
    }

    async fn mutate_state<T, F>(&self, mutator: F) -> Result<T>
    where
        F: FnOnce(&mut PersistedSessionState) -> Result<T>,
    {
        let mut guard = self.state.write().await;
        let result = mutator(&mut guard)?;
        let snapshot = guard.clone();
        drop(guard);
        self.persist_snapshot(&snapshot).await?;
        Ok(result)
    }

    async fn persist(&self) -> Result<()> {
        let snapshot = self.state.read().await.clone();
        self.persist_snapshot(&snapshot).await
    }

    async fn persist_snapshot(&self, snapshot: &PersistedSessionState) -> Result<()> {
        tokio::fs::create_dir_all(&self.root).await?;
        let raw = serde_json::to_string_pretty(snapshot)?;
        tokio::fs::write(&self.state_path, raw)
            .await
            .with_context(|| format!("failed to write {}", self.state_path.display()))?;
        Ok(())
    }
}

fn load_legacy_state(
    data_dir: &Path,
    shared_workspace_dir: &Path,
) -> Result<PersistedSessionState> {
    let legacy_path = data_dir.join("session").join("main").join("settings.json");
    let Ok(raw) = std::fs::read_to_string(&legacy_path) else {
        return Ok(PersistedSessionState::default());
    };
    let legacy = serde_json::from_str::<SessionState>(&raw)
        .with_context(|| format!("failed to parse {}", legacy_path.display()))?;
    let mut users = BTreeMap::new();
    users.insert(
        "default".to_string(),
        UserSessionState {
            foreground: DialogState {
                session_id: legacy.session_id,
                origin: DialogOrigin::Local,
                workspace_dir: prepare_workspace_dir(shared_workspace_dir)?,
                saved: false,
                profile: None,
                last_usage: None,
            },
            background: BTreeMap::new(),
            background_order: Vec::new(),
            settings: legacy.settings,
            alias_seq: 0,
            last_projects_view: Vec::new(),
            last_sessions_view: Vec::new(),
            last_import_projects_view: Vec::new(),
            last_import_sessions_view: Vec::new(),
            saved_local_session_ids: Vec::new(),
            command_aliases: BTreeMap::new(),
            pending_setting: None,
        },
    );
    Ok(PersistedSessionState {
        users,
        imported_profiles: BTreeMap::new(),
    })
}

fn ensure_user_mut<'a>(
    state: &'a mut PersistedSessionState,
    openid: &str,
    build_temporary: impl FnOnce() -> Result<DialogState>,
) -> Result<&'a mut UserSessionState> {
    if state.users.contains_key(openid) {
        // Safety: we just checked the key exists.
        return Ok(state.users.get_mut(openid).expect("user entry must exist"));
    }
    let temporary = build_temporary()?;
    state.users.insert(
        openid.to_string(),
        UserSessionState {
            foreground: temporary,
            background: BTreeMap::new(),
            background_order: Vec::new(),
            settings: SessionSettings::default(),
            alias_seq: 0,
            last_projects_view: Vec::new(),
            last_sessions_view: Vec::new(),
            last_import_projects_view: Vec::new(),
            last_import_sessions_view: Vec::new(),
            saved_local_session_ids: Vec::new(),
            command_aliases: BTreeMap::new(),
            pending_setting: None,
        },
    );
    Ok(state.users.get_mut(openid).expect("user entry must exist"))
}

fn persist_cached_profile(
    state: &mut PersistedSessionState,
    cached_profile: Option<(String, ImportedSessionProfile)>,
) {
    let Some((session_id, profile)) = cached_profile else {
        return;
    };
    state.imported_profiles.insert(session_id, profile);
}

fn record_background_alias(user: &mut UserSessionState, alias: &str) {
    user.background_order.retain(|value| value != alias);
    user.background_order.push(alias.to_string());
}

fn remove_background_alias(user: &mut UserSessionState, alias: &str) {
    user.background_order.retain(|value| value != alias);
}

fn rename_background_alias_in_order(user: &mut UserSessionState, old_alias: &str, new_alias: &str) {
    for value in &mut user.background_order {
        if value == old_alias {
            *value = new_alias.to_string();
        }
    }
    let mut deduped = Vec::with_capacity(user.background_order.len());
    for value in &user.background_order {
        if !deduped.contains(value) {
            deduped.push(value.clone());
        }
    }
    user.background_order = deduped;
}

fn cached_profile_from_dialog(dialog: &DialogState) -> Option<(String, ImportedSessionProfile)> {
    let session_id = dialog.session_id.clone()?;
    let profile = dialog.profile.as_ref()?;
    Some((
        session_id,
        ImportedSessionProfile {
            workspace_dir: dialog.workspace_dir.clone(),
            model_override: profile.model_override.clone(),
            reasoning_effort: profile.reasoning_effort,
            service_tier: None,
            context_mode: profile.context_mode,
        },
    ))
}

fn resolve_profile_for_dialog(
    global_codex_home: &Path,
    dialog: Option<&DialogState>,
) -> Result<Option<ImportedSessionProfile>> {
    let Some(dialog) = dialog else {
        return Ok(None);
    };
    if let Some(cached) = cached_profile_from_dialog(dialog) {
        return Ok(Some(cached.1));
    }
    let Some(session_id) = dialog.session_id.as_deref() else {
        return Ok(None);
    };
    let Some(target) = scan_home_sessions(global_codex_home)?
        .into_iter()
        .find(|session| session.id == session_id)
    else {
        return Ok(None);
    };
    extract_session_profile(&target.rollout_path, target.cwd)
}

fn park_foreground(
    user: &mut UserSessionState,
    requested_alias: Option<&str>,
    shared_workspace_dir: &Path,
    new_temporary: impl FnOnce() -> Result<DialogState>,
) -> Result<Option<String>> {
    if user.foreground.session_id.is_none() && !user.foreground.saved {
        let discarded_workspace = user.foreground.workspace_dir.clone();
        user.foreground = new_temporary()?;
        if discarded_workspace != user.foreground.workspace_dir
            && discarded_workspace != shared_workspace_dir
        {
            cleanup_workspace_if_empty(&discarded_workspace);
        }
        return Ok(None);
    }
    let alias = pick_alias(user, requested_alias)?;
    let mut parked = user.foreground.clone();
    parked.saved = true;
    if parked.origin == DialogOrigin::Local
        && let Some(session_id) = parked.session_id.clone()
    {
        register_local_saved_session(user, &session_id);
    }
    user.background.insert(alias.clone(), parked);
    record_background_alias(user, &alias);
    user.foreground = new_temporary()?;
    Ok(Some(alias))
}

fn pick_alias(user: &mut UserSessionState, requested: Option<&str>) -> Result<String> {
    if let Some(alias) = requested {
        let normalized = normalize_alias(alias)?;
        if user.background.contains_key(&normalized) {
            return Err(anyhow!("标签 `{normalized}` 已存在"));
        }
        return Ok(normalized);
    }
    let mut rng = rand::thread_rng();
    for _ in 0..(ALIAS_WORDS.len() * 8) {
        let Some(base) = ALIAS_WORDS.choose(&mut rng).copied() else {
            break;
        };
        if !user.background.contains_key(base) {
            return Ok(base.to_string());
        }
        // Keep alias shape simple: `<word><digits>` and within the existing 16-char limit.
        let suffix = rng.gen_range(2..=9999);
        let candidate = format!("{base}{suffix}");
        if candidate.len() <= 16 && !user.background.contains_key(&candidate) {
            return Ok(candidate);
        }
    }
    for base in ALIAS_WORDS {
        if !user.background.contains_key(*base) {
            return Ok((*base).to_string());
        }
    }
    for _ in 0..10_000 {
        user.alias_seq = user.alias_seq.saturating_add(1);
        let index = (user.alias_seq % (ALIAS_WORDS.len() as u64)) as usize;
        let base = ALIAS_WORDS[index];
        let candidate = format!("{base}{}", user.alias_seq % 10_000);
        if candidate.len() <= 16 && !user.background.contains_key(&candidate) {
            return Ok(candidate);
        }
    }
    Err(anyhow!("无法分配新的会话标签，请手动指定标签"))
}

fn normalize_alias(input: &str) -> Result<String> {
    let alias = input.trim().to_ascii_lowercase();
    let is_valid = !alias.is_empty()
        && alias.len() <= 16
        && alias
            .chars()
            .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit());
    if !is_valid {
        return Err(anyhow!(
            "标签仅允许 1-16 位小写英文或数字，例如 `sage`、`mint2`"
        ));
    }
    Ok(alias)
}

fn register_local_saved_session(user: &mut UserSessionState, session_id: &str) {
    if !user
        .saved_local_session_ids
        .iter()
        .any(|value| value == session_id)
    {
        user.saved_local_session_ids.push(session_id.to_string());
        user.saved_local_session_ids.sort();
    }
}

fn prune_session_files(codex_home: &Path, session_id: &str) -> Result<()> {
    let sessions_root = codex_home.join("sessions");
    if !sessions_root.exists() {
        return Ok(());
    }
    let mut stack = vec![sessions_root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("failed to read directory {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if name.ends_with(".jsonl") && name.contains(session_id) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    Ok(())
}

fn cleanup_workspace_if_empty(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let Ok(mut entries) = std::fs::read_dir(path) else {
        return;
    };
    if entries.next().is_some() {
        return;
    }
    let _ = std::fs::remove_dir(path);
}

fn scan_home_sessions(codex_home: &Path) -> Result<Vec<DiskSessionMeta>> {
    let index = read_session_index(codex_home)?;
    let mut files = Vec::new();
    collect_rollout_files(&codex_home.join("sessions"), &mut files)?;
    let mut sessions = Vec::new();
    for path in files {
        if let Some(meta) = parse_rollout_meta(&path, &index)? {
            sessions.push(meta);
        }
    }
    Ok(sessions)
}

fn collect_rollout_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("failed to read directory {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if file_type.is_file()
                && let Some(name) = path.file_name().and_then(|value| value.to_str())
                && name.starts_with("rollout-")
                && name.ends_with(".jsonl")
            {
                out.push(path);
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct IndexEntry {
    thread_name: Option<String>,
    first_user_message: Option<String>,
    updated_at: Option<DateTime<Utc>>,
}

fn read_session_index(codex_home: &Path) -> Result<HashMap<String, IndexEntry>> {
    let path = codex_home.join("session_index.jsonl");
    let Ok(file) = File::open(&path) else {
        return Ok(HashMap::new());
    };
    let reader = BufReader::new(file);
    let mut map = HashMap::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let id = value
            .get("id")
            .and_then(|item| item.as_str())
            .map(str::to_string);
        let Some(id) = id else {
            continue;
        };
        let thread_name = value
            .get("thread_name")
            .and_then(|item| item.as_str())
            .map(str::to_string);
        let first_user_message = value
            .get("first_user_message")
            .and_then(|item| item.as_str())
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty());
        let updated_at = value
            .get("updated_at")
            .and_then(|item| item.as_str())
            .and_then(parse_utc);
        map.insert(
            id,
            IndexEntry {
                thread_name,
                first_user_message,
                updated_at,
            },
        );
    }
    Ok(map)
}

fn parse_rollout_meta(
    path: &Path,
    index: &HashMap<String, IndexEntry>,
) -> Result<Option<DiskSessionMeta>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    if reader.read_line(&mut first_line)? == 0 {
        return Ok(None);
    }
    let value = match serde_json::from_str::<serde_json::Value>(&first_line) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if value.get("type").and_then(|item| item.as_str()) != Some("session_meta") {
        return Ok(None);
    }
    let payload = value.get("payload").and_then(|item| item.as_object());
    let Some(payload) = payload else {
        return Ok(None);
    };
    let id = payload
        .get("id")
        .and_then(|item| item.as_str())
        .ok_or_else(|| anyhow!("session meta missing id in {}", path.display()))?
        .to_string();
    let cwd = payload
        .get("cwd")
        .and_then(|item| item.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let created_at = payload
        .get("timestamp")
        .and_then(|item| item.as_str())
        .and_then(parse_utc);
    let index_entry = index.get(&id);
    let updated_at = index_entry
        .and_then(|entry| entry.updated_at)
        .or(created_at);
    let last_user_message = parse_last_user_message(&mut reader);
    let title = index_entry
        .and_then(|entry| entry.thread_name.clone())
        .or_else(|| last_user_message.clone())
        .or_else(|| {
            index_entry
                .and_then(|entry| entry.first_user_message.as_deref())
                .and_then(extract_user_message_preview)
        });
    Ok(Some(DiskSessionMeta {
        id,
        cwd,
        title,
        last_user_message,
        updated_at,
        origin: DialogOrigin::Global,
        rollout_path: path.to_path_buf(),
    }))
}

fn insert_prefer_recent(map: &mut BTreeMap<String, DiskSessionMeta>, candidate: DiskSessionMeta) {
    match map.get(&candidate.id) {
        Some(existing) if existing.updated_at >= candidate.updated_at => {}
        _ => {
            map.insert(candidate.id.clone(), candidate);
        }
    }
}

fn parse_last_user_message(reader: &mut impl BufRead) -> Option<String> {
    let mut line = String::new();
    let mut last_message = None;
    loop {
        line.clear();
        let size = reader.read_line(&mut line).ok()?;
        if size == 0 {
            return last_message;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if value.get("type").and_then(|item| item.as_str()) != Some("response_item") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(|item| item.as_str()) != Some("message") {
            continue;
        }
        if payload.get("role").and_then(|item| item.as_str()) != Some("user") {
            continue;
        }
        let Some(content) = payload.get("content").and_then(|item| item.as_array()) else {
            continue;
        };
        for item in content {
            let Some(text) = item.get("text").and_then(|value| value.as_str()) else {
                continue;
            };
            if let Some(message) = extract_user_message_preview(text) {
                last_message = Some(message);
            }
        }
    }
}

fn extract_user_message_preview(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let message = if let Some((_, tail)) = trimmed.rsplit_once("User message:\n") {
        tail.trim()
    } else {
        trimmed
    };
    if message.is_empty()
        || message == "(User sent no text, only attachments.)"
        || message.starts_with("<environment_context>")
    {
        return None;
    }
    Some(message.to_string())
}

fn copy_session_rollout(
    source_home: &Path,
    destination_home: &Path,
    source_rollout_path: &Path,
) -> Result<bool> {
    let source_root = source_home.join("sessions");
    let rel = source_rollout_path
        .strip_prefix(&source_root)
        .with_context(|| {
            format!(
                "session rollout {} is not under {}",
                source_rollout_path.display(),
                source_root.display()
            )
        })?;
    let destination = destination_home.join("sessions").join(rel);
    if destination.exists() {
        return Ok(false);
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::copy(source_rollout_path, &destination).with_context(|| {
        format!(
            "failed to copy session rollout from {} to {}",
            source_rollout_path.display(),
            destination.display()
        )
    })?;
    Ok(true)
}

fn copy_session_index_entry(
    source_home: &Path,
    destination_home: &Path,
    session_id: &str,
) -> Result<()> {
    let source_path = source_home.join("session_index.jsonl");
    let Ok(source_raw) = std::fs::read_to_string(&source_path) else {
        return Ok(());
    };
    let Some(line) = source_raw
        .lines()
        .find(|value| value.contains(&format!("\"id\":\"{session_id}\"")))
    else {
        return Ok(());
    };

    let destination_path = destination_home.join("session_index.jsonl");
    if let Ok(existing) = std::fs::read_to_string(&destination_path)
        && existing
            .lines()
            .any(|value| value.contains(&format!("\"id\":\"{session_id}\"")))
    {
        return Ok(());
    }
    if let Some(parent) = destination_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&destination_path)
        .with_context(|| format!("failed to open {}", destination_path.display()))?;
    use std::io::Write;
    writeln!(file, "{line}")
        .with_context(|| format!("failed to append {}", destination_path.display()))?;
    Ok(())
}

fn extract_session_profile(
    rollout_path: &Path,
    fallback_workspace: PathBuf,
) -> Result<Option<ImportedSessionProfile>> {
    let file = File::open(rollout_path)
        .with_context(|| format!("failed to open {}", rollout_path.display()))?;
    let reader = BufReader::new(file);
    let mut profile = ImportedSessionProfile {
        workspace_dir: fallback_workspace,
        ..ImportedSessionProfile::default()
    };
    let mut seen = false;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(item_type) = value.get("type").and_then(|item| item.as_str()) else {
            continue;
        };
        match item_type {
            "turn_context" => {
                let Some(payload) = value.get("payload").and_then(|item| item.as_object()) else {
                    continue;
                };
                if let Some(cwd) = payload.get("cwd").and_then(|item| item.as_str()) {
                    profile.workspace_dir = PathBuf::from(cwd);
                    seen = true;
                }
                if let Some(model) = payload.get("model").and_then(|item| item.as_str()) {
                    let model = model.trim();
                    if !model.is_empty() {
                        profile.model_override = Some(model.to_string());
                        seen = true;
                    }
                }
                if let Some(effort) = payload.get("effort").and_then(|item| item.as_str())
                    && let Some(parsed) = ReasoningEffort::parse(effort)
                {
                    profile.reasoning_effort = Some(parsed);
                    seen = true;
                }
                if let Some(service_tier) =
                    payload.get("service_tier").and_then(|item| item.as_str())
                    && let Some(parsed) = ServiceTier::parse(service_tier)
                {
                    profile.service_tier = Some(parsed);
                    seen = true;
                }
            }
            "event_msg" => {
                let Some(payload) = value.get("payload").and_then(|item| item.as_object()) else {
                    continue;
                };
                if payload.get("type").and_then(|item| item.as_str()) != Some("token_count") {
                    continue;
                }
                let Some(window) = payload
                    .get("info")
                    .and_then(|item| item.get("model_context_window"))
                    .and_then(|item| item.as_u64())
                else {
                    continue;
                };
                profile.context_mode = Some(ContextMode::from_model_context_window(window));
                seen = true;
            }
            _ => {}
        }
    }

    Ok(if seen { Some(profile) } else { None })
}

fn prepare_workspace_dir(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        anyhow::ensure!(path.is_dir(), "工作目录不是文件夹：{}", path.display());
    } else {
        std::fs::create_dir_all(path)
            .with_context(|| format!("failed to create workspace {}", path.display()))?;
    }
    std::fs::canonicalize(path).or_else(|_| Ok(path.to_path_buf()))
}

fn parse_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use tokio::fs;

    use crate::session::state::{
        ContextMode, DialogOrigin, DialogProfile, DialogState, ReasoningEffort, ServiceTier,
    };

    use super::{ALIAS_WORDS, SessionListScope, SessionStore};

    #[tokio::test]
    async fn moves_foreground_to_background_with_generated_alias() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        store
            .set_foreground_session_id("u1", Some("thread-1".into()))
            .await
            .unwrap();
        let moved = store
            .move_foreground_to_background("u1", None)
            .await
            .unwrap();
        assert!(moved.parked_alias.is_some());
        let alias = moved.parked_alias.unwrap();
        assert!(
            ALIAS_WORDS.contains(&alias.as_str())
                || ALIAS_WORDS.iter().any(|word| alias.starts_with(word)
                    && alias[word.len()..].chars().all(|v| v.is_ascii_digit()))
        );
        let snapshot = store.snapshot_for_user("u1").await.unwrap();
        assert!(snapshot.foreground.session_id.is_none());
        assert_eq!(snapshot.background.len(), 1);
    }

    #[tokio::test]
    async fn supports_multiple_background_dialogs() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        store
            .set_foreground_session_id("u1", Some("thread-1".into()))
            .await
            .unwrap();
        let alias_1 = store
            .move_foreground_to_background("u1", None)
            .await
            .unwrap()
            .parked_alias
            .unwrap();
        store
            .set_foreground_session_id("u1", Some("thread-2".into()))
            .await
            .unwrap();
        let alias_2 = store
            .move_foreground_to_background("u1", None)
            .await
            .unwrap()
            .parked_alias
            .unwrap();

        assert_ne!(alias_1, alias_2);
        let snapshot = store.snapshot_for_user("u1").await.unwrap();
        assert_eq!(snapshot.background.len(), 2);
    }

    #[tokio::test]
    async fn stop_foreground_restores_most_recent_background_dialog() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        store
            .bind_foreground_session_profile(
                "u1",
                Some("thread-older".into()),
                DialogProfile {
                    model_override: Some("gpt-older".into()),
                    reasoning_effort: Some(ReasoningEffort::Low),
                    service_tier: None,
                    context_mode: Some(ContextMode::Standard),
                },
            )
            .await
            .unwrap();
        store
            .move_foreground_to_background("u1", Some("older"))
            .await
            .unwrap();

        store
            .bind_foreground_session_profile(
                "u1",
                Some("thread-newer".into()),
                DialogProfile {
                    model_override: Some("gpt-newer".into()),
                    reasoning_effort: Some(ReasoningEffort::High),
                    service_tier: None,
                    context_mode: Some(ContextMode::OneM),
                },
            )
            .await
            .unwrap();
        store
            .move_foreground_to_background("u1", Some("newer"))
            .await
            .unwrap();

        store
            .set_foreground_session_id("u1", Some("thread-current".into()))
            .await
            .unwrap();
        let result = store.stop_foreground("u1").await.unwrap();
        assert_eq!(result.restored_alias.as_deref(), Some("newer"));

        let snapshot = store.snapshot_for_user("u1").await.unwrap();
        assert_eq!(
            snapshot.foreground.session_id.as_deref(),
            Some("thread-newer")
        );
        assert_eq!(
            snapshot
                .foreground
                .profile
                .as_ref()
                .and_then(|profile| profile.model_override.as_deref()),
            Some("gpt-newer")
        );
        assert!(snapshot.background.contains_key("older"));
        assert!(!snapshot.background.contains_key("newer"));
    }

    #[tokio::test]
    async fn new_foreground_reuses_shared_workspace() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        let before = store.snapshot_for_user("u1").await.unwrap();
        let old_workspace = before.foreground.workspace_dir.clone();
        assert!(old_workspace.exists());

        let switched = store.new_foreground("u1").await.unwrap();
        assert!(switched.parked_alias.is_none());
        assert!(old_workspace.exists());

        let after = store.snapshot_for_user("u1").await.unwrap();
        assert_eq!(after.foreground.workspace_dir, old_workspace);
        assert!(after.foreground.workspace_dir.exists());
    }

    #[tokio::test]
    async fn new_foreground_keeps_non_empty_temporary_workspace() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        let before = store.snapshot_for_user("u1").await.unwrap();
        let old_workspace = before.foreground.workspace_dir.clone();
        std::fs::write(old_workspace.join("note.txt"), "keep").unwrap();

        let switched = store.new_foreground("u1").await.unwrap();
        assert!(switched.parked_alias.is_none());
        assert!(old_workspace.exists());
    }

    #[tokio::test]
    async fn new_foreground_in_workspace_uses_requested_directory() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace_root = tempdir().unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace_root.path(),
        )
        .await
        .unwrap();
        let requested = workspace_root.path().join("manual workspace");

        let switched = store
            .new_foreground_in_workspace("u1", &requested)
            .await
            .unwrap();

        assert!(switched.parked_alias.is_none());
        let snapshot = store.snapshot_for_user("u1").await.unwrap();
        assert_eq!(
            snapshot.foreground.workspace_dir,
            std::fs::canonicalize(&requested).unwrap()
        );
        assert!(requested.is_dir());
    }

    #[tokio::test]
    async fn temporary_dialog_settings_update_global_defaults() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        store
            .set_model_override_for_active("u1", Some("gpt-global".into()))
            .await
            .unwrap();
        store
            .set_reasoning_for_active("u1", Some(ReasoningEffort::High))
            .await
            .unwrap();
        store
            .set_context_mode_for_active("u1", Some(ContextMode::OneM))
            .await
            .unwrap();

        let snapshot = store.snapshot_for_user("u1").await.unwrap();
        assert!(snapshot.foreground.profile.is_none());
        assert_eq!(
            snapshot.settings.model_override.as_deref(),
            Some("gpt-global")
        );
        assert_eq!(
            snapshot
                .settings
                .reasoning_effort
                .map(|value| value.as_str()),
            Some("high")
        );
        assert_eq!(snapshot.settings.context_mode, Some(ContextMode::OneM));
    }

    #[tokio::test]
    async fn non_temporary_dialog_settings_bind_to_session_profile() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        store
            .update_settings_for_user("u1", |settings| {
                settings.model_override = Some("gpt-global".into());
                settings.reasoning_effort = Some(ReasoningEffort::Low);
                settings.context_mode = Some(ContextMode::Standard);
                settings.service_tier = Some(ServiceTier::Flex);
            })
            .await
            .unwrap();
        store
            .set_foreground_session_id("u1", Some("thread-1".into()))
            .await
            .unwrap();

        store
            .set_model_override_for_active("u1", Some("gpt-dialog".into()))
            .await
            .unwrap();
        store
            .set_reasoning_for_active("u1", Some(ReasoningEffort::High))
            .await
            .unwrap();
        store
            .set_context_mode_for_active("u1", Some(ContextMode::OneM))
            .await
            .unwrap();
        store
            .set_service_tier_for_active("u1", Some(ServiceTier::Fast))
            .await
            .unwrap();

        let snapshot = store.snapshot_for_user("u1").await.unwrap();
        let profile = snapshot.foreground.profile.unwrap();
        assert_eq!(
            snapshot.settings.model_override.as_deref(),
            Some("gpt-global")
        );
        assert_eq!(
            snapshot
                .settings
                .reasoning_effort
                .map(|value| value.as_str()),
            Some("low")
        );
        assert_eq!(snapshot.settings.context_mode, Some(ContextMode::Standard));
        assert_eq!(snapshot.settings.service_tier, Some(ServiceTier::Fast));
        assert_eq!(profile.model_override.as_deref(), Some("gpt-dialog"));
        assert_eq!(profile.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(profile.context_mode, Some(ContextMode::OneM));

        let cached = store
            .imported_profile_for_session("thread-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cached.model_override.as_deref(), Some("gpt-dialog"));
        assert_eq!(cached.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(cached.context_mode, Some(ContextMode::OneM));
        assert_eq!(cached.service_tier, None);
    }

    #[tokio::test]
    async fn foreground_from_background_hydrates_legacy_session_profile() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session_dir = global_home.path().join("sessions/2026/04/11");
        fs::create_dir_all(&session_dir).await.unwrap();
        let rollout = session_dir.join("rollout-2026-04-11T00-00-00-thread-legacy.jsonl");
        fs::write(
            &rollout,
            r#"{"type":"session_meta","payload":{"id":"thread-legacy","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp/project-legacy"}}
{"type":"turn_context","payload":{"cwd":"/tmp/project-legacy","model":"gpt-5.4","effort":"medium"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":950000}}}
"#,
        )
        .await
        .unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        store
            .update_settings_for_user("u1", |settings| {
                settings.model_override = Some("gpt-global".into());
                settings.reasoning_effort = Some(ReasoningEffort::Xhigh);
                settings.context_mode = Some(ContextMode::Standard);
            })
            .await
            .unwrap();
        store
            .mutate_state(|state| {
                let user = state.users.get_mut("u1").unwrap();
                user.background.insert(
                    "quill".into(),
                    DialogState {
                        session_id: Some("thread-legacy".into()),
                        origin: DialogOrigin::Local,
                        workspace_dir: workspace.path().to_path_buf(),
                        saved: true,
                        profile: None,
                        last_usage: None,
                    },
                );
                Ok(())
            })
            .await
            .unwrap();

        store
            .foreground_from_background("u1", "quill")
            .await
            .unwrap();
        let snapshot = store.snapshot_for_user("u1").await.unwrap();
        let profile = snapshot.foreground.profile.unwrap();
        assert_eq!(profile.model_override.as_deref(), Some("gpt-5.4"));
        assert_eq!(profile.reasoning_effort, Some(ReasoningEffort::Medium));
        assert_eq!(profile.context_mode, Some(ContextMode::OneM));
    }

    #[tokio::test]
    async fn resume_local_session_extracts_profile_and_last_user_message() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session_dir = global_home.path().join("sessions/2026/04/11");
        fs::create_dir_all(&session_dir).await.unwrap();
        let rollout = session_dir.join("rollout-2026-04-11T00-00-00-thread-local.jsonl");
        fs::write(
            &rollout,
            r#"{"type":"session_meta","payload":{"id":"thread-local","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp/project-a"}}
{"type":"turn_context","payload":{"cwd":"/tmp/project-a","model":"gpt-5.4","effort":"high"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":950000}}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"You are CodexClaw running behind QQ official bot.\n\nUser message:\n请帮我修复登录接口"}]}}
"#,
        )
        .await
        .unwrap();

        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        let sessions = store
            .list_disk_sessions("u1", SessionListScope::All)
            .await
            .unwrap();
        assert_eq!(
            sessions[0].last_user_message.as_deref(),
            Some("请帮我修复登录接口")
        );

        store.resume_disk_session("u1", &sessions[0]).await.unwrap();
        let snapshot = store.snapshot_for_user("u1").await.unwrap();
        let profile = snapshot.foreground.profile.unwrap();
        assert_eq!(profile.model_override.as_deref(), Some("gpt-5.4"));
        assert_eq!(profile.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(profile.context_mode, Some(ContextMode::OneM));
    }

    #[tokio::test]
    async fn stop_drops_unsaved_local_session() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session_path = global_home.path().join("sessions/2026/04/11");
        tokio::fs::create_dir_all(&session_path).await.unwrap();
        tokio::fs::write(
            session_path.join("rollout-2026-04-11T00-00-00-thread-1.jsonl"),
            "{}",
        )
        .await
        .unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        store
            .set_foreground_session_id("u1", Some("thread-1".into()))
            .await
            .unwrap();
        let result = store.stop_foreground("u1").await.unwrap();
        assert!(result.dropped_unsaved);
        assert!(
            !session_path
                .join("rollout-2026-04-11T00-00-00-thread-1.jsonl")
                .exists()
        );
    }

    #[tokio::test]
    async fn stop_keeps_shared_workspace_for_unsaved_temporary_dialog() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        let before = store.snapshot_for_user("u1").await.unwrap();
        let old_workspace = before.foreground.workspace_dir.clone();
        assert!(old_workspace.exists());

        let result = store.stop_foreground("u1").await.unwrap();
        assert!(!result.saved);
        assert!(!result.had_session);
        assert!(!result.dropped_unsaved);
        assert!(old_workspace.exists());
        let snapshot = store.snapshot_for_user("u1").await.unwrap();
        assert_eq!(snapshot.foreground.workspace_dir, old_workspace);
    }

    #[tokio::test]
    async fn legacy_scope_aliases_map_to_all_sessions() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session_path = global_home.path().join("sessions/2026/04/11");
        tokio::fs::create_dir_all(&session_path).await.unwrap();
        tokio::fs::write(
            session_path.join("rollout-2026-04-11T00-00-00-thread-2.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"thread-2","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp"}}"#,
        )
        .await
        .unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        let sessions = store
            .list_disk_sessions("u1", SessionListScope::Local)
            .await
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "thread-2");
    }

    #[tokio::test]
    async fn local_and_global_scopes_are_legacy_aliases() {
        let data = tempdir().unwrap();
        let global_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session_path = global_home.path().join("sessions/2026/04/11");
        tokio::fs::create_dir_all(&session_path).await.unwrap();
        tokio::fs::write(
            session_path.join("rollout-2026-04-11T00-00-00-thread-3.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"thread-3","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp/p1"}}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            session_path.join("rollout-2026-04-11T00-00-00-thread-4.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"thread-4","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp/p2"}}"#,
        )
        .await
        .unwrap();
        let store = SessionStore::load_or_init(
            data.path(),
            global_home.path(),
            global_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();
        let all = store
            .list_disk_sessions("u1", SessionListScope::All)
            .await
            .unwrap();
        let local = store
            .list_disk_sessions("u1", SessionListScope::Local)
            .await
            .unwrap();
        let global = store
            .list_disk_sessions("u1", SessionListScope::Global)
            .await
            .unwrap();
        assert_eq!(local.len(), all.len());
        assert_eq!(global.len(), all.len());
    }

    #[tokio::test]
    async fn imports_system_session_and_records_profile() {
        let data = tempdir().unwrap();
        let system_home = tempdir().unwrap();
        let claw_home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let session_dir = system_home.path().join("sessions/2026/04/11");
        fs::create_dir_all(&session_dir).await.unwrap();
        let rollout = session_dir.join("rollout-2026-04-11T00-00-00-thread-import.jsonl");
        fs::write(
            &rollout,
            r#"{"type":"session_meta","payload":{"id":"thread-import","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp/project-a"}}
{"type":"turn_context","payload":{"cwd":"/tmp/project-a","model":"gpt-5.4","effort":"high"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":950000}}}
"#,
        )
        .await
        .unwrap();

        let store = SessionStore::load_or_init(
            data.path(),
            claw_home.path(),
            system_home.path(),
            workspace.path(),
        )
        .await
        .unwrap();

        let importable = store.list_importable_sessions().unwrap();
        assert_eq!(importable.len(), 1);
        let result = store.import_disk_session(&importable[0]).await.unwrap();
        assert!(result.copied);
        assert_eq!(result.profile.model_override.as_deref(), Some("gpt-5.4"));
        assert_eq!(
            result.profile.reasoning_effort.map(|value| value.as_str()),
            Some("high")
        );
        assert_eq!(
            result.profile.context_mode,
            Some(crate::session::state::ContextMode::OneM)
        );
        let imported = claw_home
            .path()
            .join("sessions/2026/04/11/rollout-2026-04-11T00-00-00-thread-import.jsonl");
        assert!(imported.exists());
    }
}
