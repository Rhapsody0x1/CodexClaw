use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use tokio::sync::{Mutex, RwLock};

use super::dialogs::Dialogs;
use super::jobs_file;
use crate::model::cron::{CronJob, JobAction, SessionStrategy};
use crate::session::rollout::{
    cache_imported_profile, copy_session_index_entry, copy_session_rollout,
    dialog_from_disk_session, extract_session_profile, insert_prefer_recent, prune_session_files,
    scan_home_sessions,
};
use crate::session::state::{
    CommandAlias, ContextMode, DialogOrigin, DialogProfile, DialogState, ImportedSessionProfile,
    PendingSetting, PersistedSessionState, ReasoningEffort, SessionSettings, SessionState,
    TokenUsageSnapshot, UserSessionState,
};
use crate::util::{fs::atomic_write, layout::DataLayout};

/// `Local`/`Global` are matched by the list formatters but nothing constructs
/// them yet — every caller asks for `All`. Kept because the scope is still the
/// encoding used for the project key on the wire.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionListScope {
    All,
    Local,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiskSessionMeta {
    pub(crate) id: String,
    pub(crate) cwd: PathBuf,
    pub(crate) title: Option<String>,
    pub(crate) last_user_message: Option<String>,
    pub(crate) updated_at: Option<DateTime<Utc>>,
    pub(crate) origin: DialogOrigin,
    pub(crate) rollout_path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct SwitchResult {
    pub(crate) parked_alias: Option<String>,
    /// Set when the foreground was a blank temporary and therefore discarded
    /// rather than parked: the alias the user asked for, validated and still
    /// free. See `SessionStore::set_pending_park_alias`.
    pub(crate) reserved_alias: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct StopResult {
    pub(crate) had_session: bool,
    pub(crate) saved: bool,
    pub(crate) dropped_unsaved: bool,
    pub(crate) restored_alias: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ImportResult {
    pub(crate) copied: bool,
    pub(crate) profile: ImportedSessionProfile,
}

pub struct SessionStore {
    root: PathBuf,
    state_path: PathBuf,
    cron_jobs_path: PathBuf,
    attachment_workspace_dir: PathBuf,
    inbox_dir: PathBuf,
    global_codex_home: PathBuf,
    system_codex_home: PathBuf,
    default_workspace_dir: PathBuf,
    state: RwLock<PersistedSessionState>,
    /// Monotonic commit sequence, incremented under the state write lock so it
    /// reflects commit order.
    mutation_seq: AtomicU64,
    /// Guards disk persistence and holds the highest commit seq already written.
    /// Serializes writers so a stale snapshot never clobbers a newer one, and
    /// lets a superseded write be skipped entirely. Held only across the disk
    /// I/O — never together with the state lock — so readers are not blocked.
    persist_lock: Mutex<u64>,
}

impl SessionStore {
    pub async fn load_or_init(
        data_dir: &Path,
        global_codex_home: &Path,
        system_codex_home: &Path,
    ) -> Result<Self> {
        let layout = DataLayout::new(data_dir);
        let root = layout.session_dir();
        let attachment_workspace_dir = layout.shared_workspace_dir();
        let inbox_dir = attachment_workspace_dir.join("inbox");
        tokio::fs::create_dir_all(&inbox_dir).await?;
        tokio::fs::create_dir_all(global_codex_home.join("sessions")).await?;
        let state_path = layout.session_state_file();
        let cron_jobs_path = layout.cron_jobs_file();
        let state = match tokio::fs::read_to_string(&state_path).await {
            Ok(raw) => serde_json::from_str::<PersistedSessionState>(&raw)
                .with_context(|| format!("failed to parse {}", state_path.display()))?,
            Err(_) => load_legacy_state(data_dir, &attachment_workspace_dir)?,
        };
        let store = Self {
            root,
            state_path,
            cron_jobs_path,
            attachment_workspace_dir: attachment_workspace_dir.clone(),
            inbox_dir,
            global_codex_home: global_codex_home.to_path_buf(),
            system_codex_home: system_codex_home.to_path_buf(),
            // Keep the shared attachment workspace as the default temporary workspace root.
            default_workspace_dir: attachment_workspace_dir,
            state: RwLock::new(state),
            mutation_seq: AtomicU64::new(0),
            persist_lock: Mutex::new(0),
        };
        store.migrate_inline_cron_jobs().await?;
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

    /// Cheap locale lookup: clones only the language string under the read
    /// lock, without deep-cloning the whole UserSessionState and without the
    /// ensure-user side effect of snapshot_for_user. Returns None for a user
    /// with no session record yet (callers fall back to the default language).
    pub(crate) async fn language_for_user(&self, openid: &str) -> Option<String> {
        self.state
            .read()
            .await
            .users
            .get(openid)
            // Normalize so a legacy/hand-edited value like "zh-CN" resolves to a
            // canonical locale instead of silently falling back to English.
            .map(|user| crate::util::lang::normalize_lang(&user.settings.language).to_string())
    }

    /// Resolve a user's UI language, falling back to the canonical default
    /// when they have no session record yet. Shared by the app's command
    /// handlers and the scheduler so locale resolution stays consistent in one
    /// place.
    pub(crate) async fn command_locale(&self, openid: &str) -> String {
        self.language_for_user(openid)
            .await
            .unwrap_or_else(crate::session::state::default_language)
    }

    pub(crate) async fn snapshot_for_user(&self, openid: &str) -> Result<UserSessionState> {
        if let Some(snapshot) = self.state.read().await.users.get(openid).cloned() {
            return Ok(snapshot);
        }
        self.mutate_user(openid, |user| Ok(user.clone())).await
    }

    /// Runs `mutator` against the (ensured) user record inside one
    /// [`Self::mutate_state`] commit. Use it when the mutation touches only
    /// that user's record; mutations that also touch sibling state (e.g.
    /// `imported_profiles`) stay on `mutate_state` directly.
    async fn mutate_user<T>(
        &self,
        openid: &str,
        mutator: impl FnOnce(&mut UserSessionState) -> Result<T>,
    ) -> Result<T> {
        self.mutate_state(|state| {
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            mutator(user)
        })
        .await
    }

    pub(crate) async fn update_settings_for_user<F>(
        &self,
        openid: &str,
        mutator: F,
    ) -> Result<UserSessionState>
    where
        F: FnOnce(&mut SessionSettings),
    {
        self.mutate_user(openid, |user| {
            mutator(&mut user.settings);
            Ok(user.clone())
        })
        .await
    }

    /// Applies one setting to whatever the "active" target is: a temporary
    /// foreground dialog routes the value into the user-wide defaults
    /// (`set_settings`), a bound dialog routes it into that dialog's profile
    /// (`set_profile`). Exactly one of the two closures runs.
    async fn set_active_setting<T>(
        &self,
        openid: &str,
        value: T,
        set_settings: impl FnOnce(&mut SessionSettings, T),
        set_profile: impl FnOnce(&mut DialogProfile, T),
    ) -> Result<UserSessionState> {
        self.mutate_state(|state| {
            let (snapshot, cached_profile) = {
                let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
                if user.foreground.is_temporary() {
                    set_settings(&mut user.settings, value);
                } else {
                    let profile = user
                        .foreground
                        .profile
                        .get_or_insert_with(DialogProfile::default);
                    set_profile(profile, value);
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

    pub(crate) async fn set_model_override_for_active(
        &self,
        openid: &str,
        value: Option<String>,
    ) -> Result<UserSessionState> {
        self.set_active_setting(
            openid,
            value,
            |settings, value| settings.model_override = value,
            |profile, value| profile.model_override = value,
        )
        .await
    }

    pub(crate) async fn set_context_mode_for_active(
        &self,
        openid: &str,
        value: Option<ContextMode>,
    ) -> Result<UserSessionState> {
        self.set_active_setting(
            openid,
            value,
            |settings, value| settings.context_mode = value,
            |profile, value| profile.context_mode = value,
        )
        .await
    }

    pub(crate) async fn set_reasoning_for_active(
        &self,
        openid: &str,
        value: Option<ReasoningEffort>,
    ) -> Result<UserSessionState> {
        self.set_active_setting(
            openid,
            value,
            |settings, value| settings.reasoning_effort = value,
            |profile, value| profile.reasoning_effort = value,
        )
        .await
    }

    pub(crate) async fn bind_foreground_session_profile(
        &self,
        openid: &str,
        session_id: Option<String>,
        profile: DialogProfile,
    ) -> Result<UserSessionState> {
        self.mutate_state(|state| {
            let (snapshot, cached_profile) = {
                let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
                Dialogs::of(user).bind(session_id, profile.clone());
                let snapshot = user.clone();
                let cached_profile = cached_profile_from_dialog(&snapshot.foreground);
                (snapshot, cached_profile)
            };
            persist_cached_profile(state, cached_profile);
            Ok(snapshot)
        })
        .await
    }

    /// Compare-and-set variant of [`Self::bind_foreground_session_profile`]
    /// for turns that ended without completing (/stop or a mid-flight
    /// failure): the binding is applied only while the foreground is still
    /// the dialog the turn started on — same generation, session id and
    /// workspace as the `expected` snapshot taken at turn start. If /stop,
    /// /new or the cron sweeper swapped the foreground mid-turn, the
    /// comparison fails and nothing is written, so the orphaned turn cannot
    /// point the foreground at its dead thread and lose the conversation the
    /// user switched to. The generation counter is what makes a temp→temp
    /// swap visible: the outgoing and incoming dialogs are value-identical in
    /// every other field. Returns whether the binding was applied. The check
    /// and the write happen under one state lock, so no swap can slip in
    /// between.
    pub(crate) async fn bind_foreground_session_profile_if_matches(
        &self,
        openid: &str,
        expected: &DialogState,
        session_id: String,
        profile: DialogProfile,
    ) -> Result<bool> {
        self.mutate_state(|state| {
            let (applied, cached_profile) = {
                let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
                if !Dialogs::of(user).bind_if_current(expected, session_id, profile) {
                    return Ok(false);
                }
                let cached_profile = cached_profile_from_dialog(&user.foreground);
                (true, cached_profile)
            };
            persist_cached_profile(state, cached_profile);
            Ok(applied)
        })
        .await
    }

    /// Persist a *successful* turn's result onto the right dialog. While the
    /// foreground is still the dialog the turn started on (same generation),
    /// this behaves like the plain bind: thread id + profile + usage land on
    /// the foreground and `None` is returned. When /bg, /new, /fg or /stop
    /// swapped the foreground mid-turn, the completed thread must not clobber
    /// the dialog the user switched to — instead it is parked as a background
    /// entry under a generated alias (returned as `Some(alias)`), so the
    /// finished conversation stays reachable rather than silently resurfacing
    /// or getting lost. A turn that produced no thread id only clears the
    /// foreground binding, and only while the foreground is still its own.
    pub(crate) async fn bind_turn_result(
        &self,
        openid: &str,
        expected: &DialogState,
        session_id: Option<String>,
        profile: DialogProfile,
        usage: Option<TokenUsageSnapshot>,
    ) -> Result<Option<String>> {
        self.mutate_state(|state| {
            let (parked_alias, cached_profile) = {
                let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
                // The reservation belongs to *this* turn either way: consume it
                // up front so it can never leak into a later park.
                let reserved = user
                    .pending_park_alias
                    .take()
                    .filter(|alias| !user.background.contains_key(alias));
                let Some(session_id) = session_id else {
                    // No thread came out of the turn: clear the stale binding,
                    // but only if the foreground is still the turn's dialog.
                    if user.foreground.generation == expected.generation {
                        user.foreground.session_id = None;
                        if let Some(usage) = usage {
                            user.foreground.last_usage = Some(usage);
                        }
                    }
                    return Ok(None);
                };
                if Dialogs::of(user).bind_if_current(expected, session_id.clone(), profile.clone())
                {
                    if let Some(usage) = usage {
                        user.foreground.last_usage = Some(usage);
                    }
                    (None, cached_profile_from_dialog(&user.foreground))
                } else {
                    let parked = DialogState {
                        session_id: Some(session_id.clone()),
                        origin: DialogOrigin::Local,
                        workspace_dir: expected.workspace_dir.clone(),
                        saved: true,
                        profile: Some(profile),
                        last_usage: usage,
                        generation: 0,
                    };
                    let cached = cached_profile_from_dialog(&parked);
                    // Honor an alias reserved by `/bg <alias>` mid-turn; fall
                    // back to a generated one if it got taken meanwhile.
                    let mut dialogs = Dialogs::of(user);
                    let alias = dialogs.add_background(reserved.as_deref(), parked)?;
                    dialogs.register_saved_session(&session_id);
                    (Some(alias), cached)
                }
            };
            persist_cached_profile(state, cached_profile);
            Ok(parked_alias)
        })
        .await
    }

    /// Test-only since `bind_turn_result` took over the production path.
    #[cfg(test)]
    pub(crate) async fn set_foreground_session_id(
        &self,
        openid: &str,
        session_id: Option<String>,
    ) -> Result<UserSessionState> {
        self.mutate_user(openid, |user| {
            user.foreground.session_id = session_id;
            Ok(user.clone())
        })
        .await
    }

    /// Test-only since `bind_turn_result` took over the production path.
    #[cfg(test)]
    pub(crate) async fn set_foreground_usage(
        &self,
        openid: &str,
        usage: TokenUsageSnapshot,
    ) -> Result<()> {
        self.mutate_user(openid, |user| {
            user.foreground.last_usage = Some(usage);
            Ok(())
        })
        .await
    }

    pub(crate) async fn set_pending_setting(
        &self,
        openid: &str,
        pending: Option<PendingSetting>,
    ) -> Result<()> {
        self.mutate_user(openid, |user| {
            user.pending_setting = pending;
            Ok(())
        })
        .await
    }

    pub(crate) async fn add_command_alias(
        &self,
        openid: &str,
        alias: CommandAlias,
    ) -> Result<CommandAlias> {
        self.mutate_user(openid, |user| {
            user.command_aliases
                .insert(alias.name.clone(), alias.clone());
            Ok(alias)
        })
        .await
    }

    pub(crate) async fn remove_command_alias(&self, openid: &str, name: &str) -> Result<bool> {
        self.mutate_user(openid, |user| {
            Ok(user.command_aliases.remove(name).is_some())
        })
        .await
    }

    pub(crate) async fn get_command_alias(
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

    pub(crate) async fn list_command_aliases(&self, openid: &str) -> Result<Vec<CommandAlias>> {
        let state = self.state.read().await;
        Ok(state
            .users
            .get(openid)
            .map(|user| user.command_aliases.values().cloned().collect())
            .unwrap_or_default())
    }

    pub(crate) async fn list_cron_jobs(&self) -> Result<Vec<CronJob>> {
        Ok(self
            .read_cron_jobs_from_disk()
            .await?
            .values()
            .cloned()
            .collect())
    }

    pub(crate) async fn get_cron_job(&self, job_id: &str) -> Result<Option<CronJob>> {
        Ok(self.read_cron_jobs_from_disk().await?.get(job_id).cloned())
    }

    pub(crate) async fn upsert_cron_job(&self, job: CronJob) -> Result<()> {
        self.mutate_cron_jobs_on_disk(move |jobs| {
            jobs.insert(job.id.clone(), job);
            Ok(())
        })
        .await
    }

    pub(crate) async fn remove_cron_job(&self, job_id: &str) -> Result<Option<CronJob>> {
        let job_id = job_id.to_string();
        self.mutate_cron_jobs_on_disk(move |jobs| Ok(jobs.remove(&job_id)))
            .await
    }

    pub(crate) async fn update_cron_job<F>(
        &self,
        job_id: &str,
        updater: F,
    ) -> Result<Option<CronJob>>
    where
        F: FnOnce(&mut CronJob) -> Result<()> + Send + 'static,
    {
        let job_id = job_id.to_string();
        self.mutate_cron_jobs_on_disk(move |jobs| {
            let Some(job) = jobs.get_mut(&job_id) else {
                return Ok(None);
            };
            updater(job)?;
            Ok(Some(job.clone()))
        })
        .await
    }

    /// The shell half of every foreground swap: park the current foreground
    /// (under `requested` or a generated alias), install `incoming`, and act
    /// on the workspace-GC decision [`Dialogs::park`] hands back. Every switch
    /// entry point routes through here, so a new one cannot silently forget
    /// the cleanup — and a second decision added to `ParkOutcome` is honoured
    /// in one place.
    fn park_and_install(
        &self,
        user: &mut UserSessionState,
        requested: Option<&str>,
        incoming: DialogState,
    ) -> Result<SwitchResult> {
        let outcome =
            Dialogs::of(user).park(requested, &self.attachment_workspace_dir, incoming)?;
        if let Some(workspace) = outcome.cleanup_workspace {
            cleanup_workspace_if_empty(&workspace);
        }
        Ok(SwitchResult {
            parked_alias: outcome.parked_alias,
            reserved_alias: outcome.reserved_alias,
        })
    }

    pub(crate) async fn new_foreground(&self, openid: &str) -> Result<SwitchResult> {
        self.mutate_user(openid, |user| {
            self.park_and_install(user, None, self.new_temporary_dialog()?)
        })
        .await
    }

    pub(crate) async fn new_foreground_in_workspace(
        &self,
        openid: &str,
        workspace_dir: &Path,
    ) -> Result<SwitchResult> {
        let workspace_dir = workspace_dir.to_path_buf();
        self.mutate_user(openid, |user| {
            let incoming = self.temporary_dialog_for_workspace(&workspace_dir)?;
            self.park_and_install(user, None, incoming)
        })
        .await
    }

    pub(crate) async fn move_foreground_to_background(
        &self,
        openid: &str,
        requested_alias: Option<&str>,
    ) -> Result<SwitchResult> {
        self.mutate_user(openid, |user| {
            self.park_and_install(user, requested_alias, self.new_temporary_dialog()?)
        })
        .await
    }

    pub(crate) async fn foreground_from_background(
        &self,
        openid: &str,
        alias: &str,
    ) -> Result<SwitchResult> {
        let (target, available) = {
            let guard = self.state.read().await;
            let user = guard.users.get(openid);
            (
                user.and_then(|user| user.background.get(alias)).cloned(),
                user.map(|user| user.background_order.iter().rev().cloned().collect())
                    .unwrap_or_default(),
            )
        };
        let mut target = target.ok_or_else(|| {
            anyhow::Error::new(super::dialogs::DialogError::BackgroundNotFound {
                alias: alias.to_string(),
                available,
            })
        })?;
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
            Dialogs::of(user).take_background(alias)?;
            self.park_and_install(user, None, target.clone())
        })
        .await
    }

    /// Shared profile lookup for `resume_disk_session` /
    /// `load_disk_session_to_background`: the cached imported profile wins,
    /// otherwise the profile is re-extracted from the rollout file.
    async fn resolve_disk_session_profile(
        &self,
        target: &DiskSessionMeta,
    ) -> Result<Option<ImportedSessionProfile>> {
        Ok(self
            .state
            .read()
            .await
            .imported_profiles
            .get(&target.id)
            .cloned()
            .or(extract_session_profile(
                &target.rollout_path,
                target.cwd.clone(),
            )?))
    }

    pub(crate) async fn resume_disk_session(
        &self,
        openid: &str,
        target: &DiskSessionMeta,
    ) -> Result<SwitchResult> {
        let resolved_profile = self.resolve_disk_session_profile(target).await?;
        self.mutate_state(|state| {
            cache_imported_profile(state, &target.id, resolved_profile.as_ref());
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            let incoming = dialog_from_disk_session(target, resolved_profile.as_ref());
            self.park_and_install(user, None, incoming)
        })
        .await
    }

    pub(crate) async fn load_disk_session_to_background(
        &self,
        openid: &str,
        target: &DiskSessionMeta,
        requested_alias: Option<&str>,
    ) -> Result<String> {
        let resolved_profile = self.resolve_disk_session_profile(target).await?;
        self.mutate_state(|state| {
            cache_imported_profile(state, &target.id, resolved_profile.as_ref());
            let user = ensure_user_mut(state, openid, || self.new_temporary_dialog())?;
            Dialogs::of(user).add_background(
                requested_alias,
                dialog_from_disk_session(target, resolved_profile.as_ref()),
            )
        })
        .await
    }

    pub(crate) async fn rename_background_alias(
        &self,
        openid: &str,
        old_alias: &str,
        new_alias: &str,
    ) -> Result<()> {
        self.mutate_user(openid, |user| {
            Dialogs::of(user).rename_background(old_alias, new_alias)
        })
        .await
    }

    pub(crate) async fn save_foreground(&self, openid: &str) -> Result<bool> {
        self.mutate_user(openid, |user| Ok(Dialogs::of(user).save()))
            .await
    }

    pub(crate) async fn stop_foreground(&self, openid: &str) -> Result<StopResult> {
        let current = self.snapshot_for_user(openid).await?.foreground;
        let had_session = current.session_id.is_some();
        let saved = current.saved;
        let dropped_unsaved =
            current.origin == DialogOrigin::Local && current.session_id.is_some() && !saved;
        let restored = {
            let guard = self.state.read().await;
            let dialog = guard
                .users
                .get(openid)
                .and_then(super::dialogs::most_recent_background);
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
                let mut dialogs = Dialogs::of(user);
                if dropped_unsaved && let Some(session_id) = current.session_id.as_deref() {
                    dialogs.drop_saved_session(session_id);
                } else if saved
                    && current.origin == DialogOrigin::Local
                    && let Some(session_id) = current.session_id.as_deref()
                {
                    dialogs.register_saved_session(session_id);
                }
                if let Some((alias, _, profile)) = restored.clone() {
                    let mut dialog = dialogs.take_background(&alias)?;
                    if let Some(profile) = profile {
                        dialog.workspace_dir = profile.workspace_dir.clone();
                        dialog.profile = Some(profile.dialog_profile());
                    }
                    dialogs.install(dialog);
                    Ok(Some(alias))
                } else {
                    dialogs.install(self.new_temporary_dialog()?);
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

    /// Shared body of the four `set_last_*_view` setters: `field` selects
    /// which cached view list on the user record receives `ids`.
    async fn set_view(
        &self,
        openid: &str,
        ids: Vec<String>,
        field: impl FnOnce(&mut UserSessionState) -> &mut Vec<String>,
    ) -> Result<()> {
        self.mutate_user(openid, |user| {
            *field(user) = ids;
            Ok(())
        })
        .await
    }

    /// Shared body of the four `last_*_view` getters: `field` moves the
    /// selected view list out of the user snapshot.
    async fn view(
        &self,
        openid: &str,
        field: impl FnOnce(UserSessionState) -> Vec<String>,
    ) -> Result<Vec<String>> {
        Ok(field(self.snapshot_for_user(openid).await?))
    }

    pub(crate) async fn set_last_sessions_view(
        &self,
        openid: &str,
        ids: Vec<String>,
    ) -> Result<()> {
        self.set_view(openid, ids, |user| &mut user.last_sessions_view)
            .await
    }

    pub(crate) async fn set_last_projects_view(
        &self,
        openid: &str,
        ids: Vec<String>,
    ) -> Result<()> {
        self.set_view(openid, ids, |user| &mut user.last_projects_view)
            .await
    }

    pub(crate) async fn set_last_import_sessions_view(
        &self,
        openid: &str,
        ids: Vec<String>,
    ) -> Result<()> {
        self.set_view(openid, ids, |user| &mut user.last_import_sessions_view)
            .await
    }

    pub(crate) async fn set_last_import_projects_view(
        &self,
        openid: &str,
        ids: Vec<String>,
    ) -> Result<()> {
        self.set_view(openid, ids, |user| &mut user.last_import_projects_view)
            .await
    }

    pub(crate) async fn last_sessions_view(&self, openid: &str) -> Result<Vec<String>> {
        self.view(openid, |user| user.last_sessions_view).await
    }

    /// Reserve the alias the turn-end park should use (see
    /// `pending_park_alias`); the alias is already normalized by the caller.
    pub(crate) async fn set_pending_park_alias(
        &self,
        openid: &str,
        alias: Option<String>,
    ) -> Result<()> {
        self.mutate_user(openid, |user| {
            user.pending_park_alias = alias;
            Ok(())
        })
        .await
    }

    pub(crate) async fn set_last_cron_view(&self, openid: &str, ids: Vec<String>) -> Result<()> {
        self.set_view(openid, ids, |user| &mut user.last_cron_view)
            .await
    }

    pub(crate) async fn last_cron_view(&self, openid: &str) -> Result<Vec<String>> {
        self.view(openid, |user| user.last_cron_view.clone()).await
    }

    pub(crate) async fn last_projects_view(&self, openid: &str) -> Result<Vec<String>> {
        self.view(openid, |user| user.last_projects_view).await
    }

    pub(crate) async fn last_import_sessions_view(&self, openid: &str) -> Result<Vec<String>> {
        self.view(openid, |user| user.last_import_sessions_view)
            .await
    }

    pub(crate) async fn last_import_projects_view(&self, openid: &str) -> Result<Vec<String>> {
        self.view(openid, |user| user.last_import_projects_view)
            .await
    }

    /// The scope is currently a no-op filter (every caller passes `All`), but
    /// the parameter stays: commands.rs still encodes/decodes the project key
    /// with [`SessionListScope`].
    pub(crate) async fn list_disk_sessions(
        &self,
        _scope: SessionListScope,
    ) -> Result<Vec<DiskSessionMeta>> {
        // scan_home_sessions recursively walks the sessions dir and reads every
        // rollout file to EOF; run it off the reactor so a /sessions with many
        // or large sessions can't stall a tokio worker (and the gateway).
        let home = self.global_codex_home.clone();
        let mut values = tokio::task::spawn_blocking(move || -> Result<Vec<DiskSessionMeta>> {
            let mut by_id = BTreeMap::new();
            for session in scan_home_sessions(&home)? {
                insert_prefer_recent(&mut by_id, session);
            }
            Ok(by_id.into_values().collect::<Vec<_>>())
        })
        .await??;
        self.retain_listable_cron_sessions(&mut values).await?;
        values.sort_by_key(|value| std::cmp::Reverse(value.updated_at));
        Ok(values)
    }

    /// Cron jobs run in `data/cron-jobs/<job_id>/workspace`, and every
    /// per-invocation run leaves its own rollout — without a filter a daily
    /// job manufactures one `/sessions` row per day. Only sessions belonging
    /// to a *persistent* CodexTurn job stay listed: that strategy keeps one
    /// resumable thread across runs, which is exactly the "worth replaying"
    /// case (e.g. a daily exercise dialog). Sessions of removed or
    /// per-invocation jobs are hidden; `/cron tail` remains their home.
    async fn retain_listable_cron_sessions(&self, values: &mut Vec<DiskSessionMeta>) -> Result<()> {
        let cron_root = DataLayout::new(self.data_dir()).cron_jobs_dir();
        if !values
            .iter()
            .any(|session| session.cwd.starts_with(&cron_root))
        {
            return Ok(());
        }
        let jobs = self.read_cron_jobs_from_disk().await?;
        values.retain(|session| {
            let Ok(relative) = session.cwd.strip_prefix(&cron_root) else {
                return true;
            };
            let Some(job_id) = relative.components().next() else {
                return true;
            };
            let job_id = job_id.as_os_str().to_string_lossy();
            jobs.get(job_id.as_ref()).is_some_and(|job| {
                matches!(
                    &job.action,
                    JobAction::CodexTurn {
                        session_strategy: SessionStrategy::Persistent,
                        ..
                    }
                )
            })
        });
        Ok(())
    }

    pub(crate) fn list_importable_sessions(&self) -> Result<Vec<DiskSessionMeta>> {
        let mut values = scan_home_sessions(&self.system_codex_home)?;
        values.sort_by_key(|value| std::cmp::Reverse(value.updated_at));
        Ok(values)
    }

    pub(crate) async fn import_disk_session(
        &self,
        target: &DiskSessionMeta,
    ) -> Result<ImportResult> {
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

    pub(crate) async fn imported_profile_for_session(
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

    pub(crate) fn codex_home(&self) -> &Path {
        &self.global_codex_home
    }

    pub(crate) fn data_dir(&self) -> &Path {
        self.root.parent().unwrap_or(&self.root)
    }

    pub(crate) fn inbox_dir(&self) -> &Path {
        &self.inbox_dir
    }

    pub(crate) fn attachment_workspace_dir(&self) -> &Path {
        &self.attachment_workspace_dir
    }

    pub(crate) fn default_workspace_dir(&self) -> &Path {
        &self.default_workspace_dir
    }

    async fn migrate_inline_cron_jobs(&self) -> Result<()> {
        if self.cron_jobs_path.exists() {
            return Ok(());
        }
        let jobs = self.state.read().await.cron_jobs.clone();
        if jobs.is_empty() {
            return Ok(());
        }
        self.persist_cron_jobs(&jobs).await?;
        self.state.write().await.cron_jobs.clear();
        Ok(())
    }

    async fn read_cron_jobs_from_disk(&self) -> Result<BTreeMap<String, CronJob>> {
        let path = self.cron_jobs_path.clone();
        let fallback = self.state.read().await.cron_jobs.clone();
        tokio::task::spawn_blocking(move || jobs_file::read_jobs(&path, fallback)).await?
    }

    async fn persist_cron_jobs(&self, jobs: &BTreeMap<String, CronJob>) -> Result<()> {
        let path = self.cron_jobs_path.clone();
        let jobs = jobs.clone();
        tokio::task::spawn_blocking(move || jobs_file::write_jobs(&path, &jobs)).await?
    }

    async fn mutate_cron_jobs_on_disk<T, F>(&self, mutator: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut BTreeMap<String, CronJob>) -> Result<T> + Send + 'static,
    {
        let path = self.cron_jobs_path.clone();
        let fallback = self.state.read().await.cron_jobs.clone();
        tokio::task::spawn_blocking(move || jobs_file::mutate_jobs(&path, fallback, mutator))
            .await?
    }

    async fn mutate_state<T, F>(&self, mutator: F) -> Result<T>
    where
        F: FnOnce(&mut PersistedSessionState) -> Result<T>,
    {
        let mut guard = self.state.write().await;
        let result = mutator(&mut guard)?;
        let snapshot = guard.clone();
        // Assign the commit seq under the exclusive lock so it matches commit
        // order, then release the lock BEFORE any disk I/O so readers never
        // block on persistence.
        let seq = self.mutation_seq.fetch_add(1, Ordering::SeqCst) + 1;
        drop(guard);

        // Serialize writers on persist_lock (holds the highest seq written).
        // Because each later commit's snapshot is a superset of earlier ones, a
        // write whose seq is already superseded can be skipped; and a stale
        // snapshot can never overwrite a newer one that already reached disk.
        let mut last_persisted = self.persist_lock.lock().await;
        if seq > *last_persisted {
            self.persist_snapshot(&snapshot).await?;
            *last_persisted = seq;
        }
        Ok(result)
    }

    async fn persist(&self) -> Result<()> {
        let snapshot = self.state.read().await.clone();
        self.persist_snapshot(&snapshot).await
    }

    async fn persist_snapshot(&self, snapshot: &PersistedSessionState) -> Result<()> {
        tokio::fs::create_dir_all(&self.root).await?;
        let raw = serde_json::to_string_pretty(snapshot)?;
        let path = self.state_path.clone();
        // Write atomically (temp file + fsync + rename) so an interrupted or
        // crashed write can never leave a truncated state.json that fails to
        // parse on the next startup and wipes every user's session state.
        tokio::task::spawn_blocking(move || atomic_write(&path, &raw)).await??;
        Ok(())
    }
}

fn load_legacy_state(
    data_dir: &Path,
    shared_workspace_dir: &Path,
) -> Result<PersistedSessionState> {
    let legacy_path = DataLayout::new(data_dir).legacy_settings_file();
    let Ok(raw) = std::fs::read_to_string(&legacy_path) else {
        return Ok(PersistedSessionState::default());
    };
    let legacy = serde_json::from_str::<SessionState>(&raw)
        .with_context(|| format!("failed to parse {}", legacy_path.display()))?;
    let mut user = UserSessionState::new(prepare_workspace_dir(shared_workspace_dir)?);
    user.foreground.session_id = legacy.session_id;
    user.settings = legacy.settings;
    let mut users = BTreeMap::new();
    users.insert("default".to_string(), user);
    Ok(PersistedSessionState {
        users,
        ..PersistedSessionState::default()
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
    let mut user = UserSessionState::new(temporary.workspace_dir.clone());
    Dialogs::of(&mut user).install(temporary);
    state.users.insert(openid.to_string(), user);
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

fn prepare_workspace_dir(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        anyhow::ensure!(path.is_dir(), "工作目录不是文件夹：{}", path.display());
    } else {
        std::fs::create_dir_all(path)
            .with_context(|| format!("failed to create workspace {}", path.display()))?;
    }
    std::fs::canonicalize(path).or_else(|_| Ok(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use tempfile::{TempDir, tempdir};
    use tokio::fs;

    use crate::model::cron::fixtures::{shell_job, ts};
    use crate::model::cron::{CronJob, CronKind};
    use crate::session::state::{
        ContextMode, DialogOrigin, DialogProfile, DialogState, PersistedSessionState,
        ReasoningEffort, ServiceTier, UserSessionState,
    };

    use super::{SessionListScope, SessionStore};
    use crate::session::dialogs::ALIAS_WORDS;

    /// Shared harness for the store tests.
    ///
    /// The `TempDir` handles must outlive the store — dropping them is what
    /// deletes the directories — so they are owned here rather than by a
    /// helper that returns only the store.
    struct TestEnv {
        data: TempDir,
        home: TempDir,
        store: SessionStore,
    }

    impl TestEnv {
        async fn new() -> Self {
            Self::with_persisted_state(None).await
        }

        /// Same as `new`, but seeds `session/state.json` before the store is
        /// loaded so migrations run against it.
        async fn with_persisted_state(state: Option<&PersistedSessionState>) -> Self {
            let data = tempdir().unwrap();
            let home = tempdir().unwrap();
            if let Some(state) = state {
                let state_path = data.path().join("session/state.json");
                fs::create_dir_all(state_path.parent().unwrap())
                    .await
                    .unwrap();
                fs::write(&state_path, serde_json::to_string_pretty(state).unwrap())
                    .await
                    .unwrap();
            }
            let store = SessionStore::load_or_init(data.path(), home.path(), home.path())
                .await
                .unwrap();
            Self { data, home, store }
        }

        fn data_path(&self) -> &Path {
            self.data.path()
        }

        /// Codex home used both as the global and the system rollout root.
        fn home_path(&self) -> &Path {
            self.home.path()
        }

        async fn snapshot(&self, openid: &str) -> UserSessionState {
            self.store.snapshot_for_user(openid).await.unwrap()
        }

        /// Writes one rollout file under the codex home and returns its path.
        async fn write_rollout(&self, name: &str, body: &str) -> PathBuf {
            let session_dir = self.home_path().join("sessions/2026/04/11");
            fs::create_dir_all(&session_dir).await.unwrap();
            let path = session_dir.join(name);
            fs::write(&path, body).await.unwrap();
            path
        }
    }

    fn sample_cron_job(id: &str, workspace_dir: PathBuf) -> CronJob {
        let next_run_at = ts("2026-05-10T12:00:00Z");
        let mut job = shell_job(id, workspace_dir, ts("2026-05-10T10:00:00Z"));
        job.owner_openid = "owner-1".to_string();
        job.title = "drink reminder".to_string();
        job.kind = CronKind::OneShot { at: next_run_at };
        job.next_run_at = Some(next_run_at);
        job
    }

    #[tokio::test]
    async fn cron_jobs_persist_to_scheduler_file() {
        let env = TestEnv::new().await;
        let workspace = tempdir().unwrap();
        let job = sample_cron_job("job-1", workspace.path().join("cron-workspace"));

        env.store.upsert_cron_job(job.clone()).await.unwrap();

        let jobs_path = env.data_path().join("scheduler/jobs.json");
        let raw_jobs = fs::read_to_string(&jobs_path).await.unwrap();
        let persisted_jobs: BTreeMap<String, CronJob> = serde_json::from_str(&raw_jobs).unwrap();
        assert_eq!(persisted_jobs.get("job-1"), Some(&job));

        let state_raw = fs::read_to_string(env.data_path().join("session/state.json"))
            .await
            .unwrap();
        let state: PersistedSessionState = serde_json::from_str(&state_raw).unwrap();
        assert!(state.cron_jobs.is_empty());
    }

    #[tokio::test]
    async fn migrates_inline_cron_jobs_to_scheduler_file_once() {
        let workspace = tempdir().unwrap();
        let job = sample_cron_job("legacy-job", workspace.path().join("legacy-workspace"));
        let mut state = PersistedSessionState::default();
        state.cron_jobs.insert(job.id.clone(), job.clone());

        let env = TestEnv::with_persisted_state(Some(&state)).await;

        assert_eq!(
            env.store.get_cron_job("legacy-job").await.unwrap(),
            Some(job)
        );
        let jobs_path = env.data_path().join("scheduler/jobs.json");
        assert!(jobs_path.exists());
        let state_raw = fs::read_to_string(env.data_path().join("session/state.json"))
            .await
            .unwrap();
        let migrated_state: PersistedSessionState = serde_json::from_str(&state_raw).unwrap();
        assert!(migrated_state.cron_jobs.is_empty());
    }

    #[tokio::test]
    async fn session_state_persist_does_not_overwrite_scheduler_jobs() {
        let env = TestEnv::new().await;
        let workspace = tempdir().unwrap();
        let job = sample_cron_job("job-keep", workspace.path().join("cron-workspace"));
        env.store.upsert_cron_job(job.clone()).await.unwrap();

        env.store
            .update_settings_for_user("u1", |settings| {
                settings.model_override = Some("gpt-test".to_string());
            })
            .await
            .unwrap();

        assert_eq!(env.store.get_cron_job("job-keep").await.unwrap(), Some(job));
        let raw_jobs = fs::read_to_string(env.data_path().join("scheduler/jobs.json"))
            .await
            .unwrap();
        let persisted_jobs: BTreeMap<String, CronJob> = serde_json::from_str(&raw_jobs).unwrap();
        assert!(persisted_jobs.contains_key("job-keep"));
    }

    #[tokio::test]
    async fn update_cron_job_mutates_existing_record_without_replacing_it() {
        let env = TestEnv::new().await;
        let workspace = tempdir().unwrap();
        let mut job = sample_cron_job("job-update", workspace.path().join("cron-workspace"));
        job.title = "original title".to_string();
        env.store.upsert_cron_job(job).await.unwrap();

        let updated = env
            .store
            .update_cron_job("job-update", |job| {
                job.failure_streak = 3;
                Ok(())
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(updated.title, "original title");
        assert_eq!(updated.failure_streak, 3);
        assert_eq!(
            env.store
                .get_cron_job("job-update")
                .await
                .unwrap()
                .unwrap()
                .failure_streak,
            3
        );
    }

    #[tokio::test]
    async fn moves_foreground_to_background_with_generated_alias() {
        let env = TestEnv::new().await;
        env.store
            .set_foreground_session_id("u1", Some("thread-1".into()))
            .await
            .unwrap();
        let moved = env
            .store
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
        let snapshot = env.snapshot("u1").await;
        assert!(snapshot.foreground.session_id.is_none());
        assert_eq!(snapshot.background.len(), 1);
    }

    #[tokio::test]
    async fn bind_if_matches_applies_only_while_foreground_unchanged() {
        let env = TestEnv::new().await;

        // Foreground unchanged since the turn started: the binding lands,
        // profile included.
        let turn_start = env.snapshot("u1").await;
        let applied = env
            .store
            .bind_foreground_session_profile_if_matches(
                "u1",
                &turn_start.foreground,
                "interrupted-thread".to_string(),
                DialogProfile {
                    model_override: Some("gpt-x".into()),
                    reasoning_effort: Some(ReasoningEffort::High),
                    service_tier: None,
                    context_mode: Some(ContextMode::Standard),
                },
            )
            .await
            .unwrap();
        assert!(applied);
        let snapshot = env.snapshot("u1").await;
        assert_eq!(
            snapshot.foreground.session_id.as_deref(),
            Some("interrupted-thread")
        );
        assert_eq!(
            snapshot
                .foreground
                .profile
                .as_ref()
                .and_then(|profile| profile.model_override.as_deref()),
            Some("gpt-x")
        );

        // Foreground swapped mid-turn (e.g. /stop restored the parked
        // conversation): the stale turn must not clobber it.
        let turn_start = env.snapshot("u2").await;
        env.store
            .set_foreground_session_id("u2", Some("restored-thread".into()))
            .await
            .unwrap();
        let applied = env
            .store
            .bind_foreground_session_profile_if_matches(
                "u2",
                &turn_start.foreground,
                "orphaned-thread".to_string(),
                DialogProfile::default(),
            )
            .await
            .unwrap();
        assert!(!applied);
        let snapshot = env.snapshot("u2").await;
        assert_eq!(
            snapshot.foreground.session_id.as_deref(),
            Some("restored-thread")
        );

        // Same (None) session id but a different workspace — the foreground
        // was replaced by a fresh dialog elsewhere (cron per-invocation
        // stop): still refused.
        let turn_start = env.snapshot("u3").await;
        let other_workspace = tempdir().unwrap();
        env.store
            .new_foreground_in_workspace("u3", other_workspace.path())
            .await
            .unwrap();
        let applied = env
            .store
            .bind_foreground_session_profile_if_matches(
                "u3",
                &turn_start.foreground,
                "orphaned-thread".to_string(),
                DialogProfile::default(),
            )
            .await
            .unwrap();
        assert!(!applied);
        let snapshot = env.snapshot("u3").await;
        assert!(snapshot.foreground.session_id.is_none());
    }

    #[tokio::test]
    async fn bind_if_matches_rejects_after_temp_to_temp_swap() {
        let env = TestEnv::new().await;

        // First turn ever: the foreground is a fresh temporary dialog
        // (session_id None, shared workspace). /stop mid-turn installs a new
        // temporary that is value-identical in every field except the
        // generation counter.
        let turn_start = env.snapshot("u1").await;
        assert!(turn_start.foreground.session_id.is_none());
        env.store.stop_foreground("u1").await.unwrap();

        // The aborted turn's tail must not bind its dead thread to the
        // dialog the user just reset.
        let applied = env
            .store
            .bind_foreground_session_profile_if_matches(
                "u1",
                &turn_start.foreground,
                "stopped-thread".to_string(),
                DialogProfile::default(),
            )
            .await
            .unwrap();
        assert!(!applied, "temp→temp swap must invalidate the turn snapshot");
        let snapshot = env.snapshot("u1").await;
        assert!(
            snapshot.foreground.session_id.is_none(),
            "the reset foreground must stay unbound"
        );
    }

    #[tokio::test]
    async fn preview_skips_injected_user_texts() {
        let env = TestEnv::new().await;
        env.write_rollout(
            "rollout-2026-07-26T00-00-00-thread-injected.jsonl",
            concat!(
                r#"{"type":"session_meta","payload":{"id":"thread-injected","timestamp":"2026-07-26T00:00:00Z","cwd":"/tmp/p"}}"#, "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"真正的问题在这里"}]}}"#, "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<turn_aborted> The user interrupted the previous turn on purpose."}]}}"#, "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":">>> APPROVAL REQUEST END"}]}}"#, "\n",
            ),
        )
        .await;

        let sessions = env
            .store
            .list_disk_sessions(SessionListScope::All)
            .await
            .unwrap();
        let session = sessions
            .iter()
            .find(|s| s.id == "thread-injected")
            .expect("session listed");
        assert_eq!(
            session.last_user_message.as_deref(),
            Some("真正的问题在这里"),
            "injected user-role texts must not win the preview"
        );
    }

    #[tokio::test]
    async fn cron_sessions_listed_only_for_persistent_jobs() {
        use crate::model::cron::{JobAction, SessionStrategy, fixtures::shell_job};

        let env = TestEnv::new().await;
        let cron_root = env.store.data_dir().join("cron-jobs");
        for (job_id, thread) in [("job-per", "thread-per"), ("job-persist", "thread-persist")] {
            let cwd = cron_root.join(job_id).join("workspace");
            env.write_rollout(
                &format!("rollout-2026-07-26T00-00-00-{thread}.jsonl"),
                &format!(
                    "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{thread}\",\"timestamp\":\"2026-07-26T00:00:00Z\",\"cwd\":\"{}\"}}}}\n",
                    cwd.display()
                ),
            )
            .await;
        }
        let mut per = shell_job(
            "job-per",
            cron_root.join("job-per/workspace"),
            chrono::Utc::now(),
        );
        per.action = JobAction::CodexTurn {
            prompt: "p".into(),
            model: None,
            session_state: None,
            approval_policy: None,
            session_strategy: SessionStrategy::PerInvocation,
            interactive: None,
        };
        env.store.upsert_cron_job(per).await.unwrap();
        let mut persist = shell_job(
            "job-persist",
            cron_root.join("job-persist/workspace"),
            chrono::Utc::now(),
        );
        persist.action = JobAction::CodexTurn {
            prompt: "p".into(),
            model: None,
            session_state: None,
            approval_policy: None,
            session_strategy: SessionStrategy::Persistent,
            interactive: None,
        };
        env.store.upsert_cron_job(persist).await.unwrap();

        let sessions = env
            .store
            .list_disk_sessions(SessionListScope::All)
            .await
            .unwrap();
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert!(
            ids.contains(&"thread-persist"),
            "persistent job sessions are worth resuming: {ids:?}"
        );
        assert!(
            !ids.contains(&"thread-per"),
            "per-invocation cron runs must not pollute /sessions: {ids:?}"
        );
    }

    #[tokio::test]
    async fn bind_turn_result_parks_thread_when_foreground_moved_on() {
        let env = TestEnv::new().await;

        // A first turn starts on a fresh temporary foreground; the user sends
        // /bg (or /new) mid-turn, swapping in a value-identical temporary.
        let turn_start = env.snapshot("u1").await;
        env.store.new_foreground("u1").await.unwrap();

        // The turn then *succeeds*: its thread must not clobber the new
        // foreground — it gets parked as a background entry instead.
        let parked = env
            .store
            .bind_turn_result(
                "u1",
                &turn_start.foreground,
                Some("finished-thread".into()),
                DialogProfile::default(),
                None,
            )
            .await
            .unwrap();
        let alias = parked.expect("thread should be parked, not bound");
        let snapshot = env.snapshot("u1").await;
        assert!(
            snapshot.foreground.session_id.is_none(),
            "foreground untouched"
        );
        let entry = snapshot
            .background
            .get(&alias)
            .expect("parked entry exists");
        assert_eq!(entry.session_id.as_deref(), Some("finished-thread"));
        assert!(entry.saved, "parked turn results are kept");
        assert!(
            snapshot
                .saved_local_session_ids
                .iter()
                .any(|id| id == "finished-thread"),
            "rollout must survive pruning"
        );
    }

    #[tokio::test]
    async fn bind_turn_result_honors_the_alias_reserved_by_bg() {
        let env = TestEnv::new().await;

        // `/bg main` during the very first turn: nothing to park yet, so the
        // alias is only reserved.
        let turn_start = env.snapshot("u1").await;
        let moved = env
            .store
            .move_foreground_to_background("u1", Some("main"))
            .await
            .unwrap();
        assert!(moved.parked_alias.is_none());
        assert_eq!(moved.reserved_alias.as_deref(), Some("main"));
        env.store
            .set_pending_park_alias("u1", moved.reserved_alias)
            .await
            .unwrap();

        let parked = env
            .store
            .bind_turn_result(
                "u1",
                &turn_start.foreground,
                Some("finished-thread".into()),
                DialogProfile::default(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(parked.as_deref(), Some("main"));
        let snapshot = env.snapshot("u1").await;
        assert_eq!(
            snapshot
                .background
                .get("main")
                .unwrap()
                .session_id
                .as_deref(),
            Some("finished-thread")
        );
        assert!(
            snapshot.pending_park_alias.is_none(),
            "the reservation is consumed exactly once"
        );
    }

    #[tokio::test]
    async fn bind_turn_result_clears_a_reservation_it_did_not_use() {
        let env = TestEnv::new().await;
        env.store
            .set_pending_park_alias("u1", Some("main".into()))
            .await
            .unwrap();
        let turn_start = env.snapshot("u1").await;
        env.store
            .bind_turn_result(
                "u1",
                &turn_start.foreground,
                Some("live-thread".into()),
                DialogProfile::default(),
                None,
            )
            .await
            .unwrap();
        let snapshot = env.snapshot("u1").await;
        assert!(
            snapshot.pending_park_alias.is_none(),
            "a reservation must never leak into a later park"
        );
    }

    #[tokio::test]
    async fn bind_turn_result_binds_while_foreground_unchanged() {
        let env = TestEnv::new().await;
        let turn_start = env.snapshot("u1").await;
        let parked = env
            .store
            .bind_turn_result(
                "u1",
                &turn_start.foreground,
                Some("live-thread".into()),
                DialogProfile::default(),
                None,
            )
            .await
            .unwrap();
        assert!(parked.is_none());
        let snapshot = env.snapshot("u1").await;
        assert_eq!(
            snapshot.foreground.session_id.as_deref(),
            Some("live-thread")
        );
    }

    #[tokio::test]
    async fn supports_multiple_background_dialogs() {
        let env = TestEnv::new().await;
        env.store
            .set_foreground_session_id("u1", Some("thread-1".into()))
            .await
            .unwrap();
        let alias_1 = env
            .store
            .move_foreground_to_background("u1", None)
            .await
            .unwrap()
            .parked_alias
            .unwrap();
        env.store
            .set_foreground_session_id("u1", Some("thread-2".into()))
            .await
            .unwrap();
        let alias_2 = env
            .store
            .move_foreground_to_background("u1", None)
            .await
            .unwrap()
            .parked_alias
            .unwrap();

        assert_ne!(alias_1, alias_2);
        let snapshot = env.snapshot("u1").await;
        assert_eq!(snapshot.background.len(), 2);
    }

    #[tokio::test]
    async fn stop_foreground_restores_most_recent_background_dialog() {
        let env = TestEnv::new().await;

        env.store
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
        env.store
            .move_foreground_to_background("u1", Some("older"))
            .await
            .unwrap();

        env.store
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
        env.store
            .move_foreground_to_background("u1", Some("newer"))
            .await
            .unwrap();

        env.store
            .set_foreground_session_id("u1", Some("thread-current".into()))
            .await
            .unwrap();
        let result = env.store.stop_foreground("u1").await.unwrap();
        assert_eq!(result.restored_alias.as_deref(), Some("newer"));

        let snapshot = env.snapshot("u1").await;
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
        let env = TestEnv::new().await;
        let before = env.snapshot("u1").await;
        let old_workspace = before.foreground.workspace_dir.clone();
        assert!(old_workspace.exists());

        let switched = env.store.new_foreground("u1").await.unwrap();
        assert!(switched.parked_alias.is_none());
        assert!(old_workspace.exists());

        let after = env.snapshot("u1").await;
        assert_eq!(after.foreground.workspace_dir, old_workspace);
        assert!(after.foreground.workspace_dir.exists());
    }

    #[tokio::test]
    async fn new_foreground_in_workspace_uses_requested_directory() {
        let env = TestEnv::new().await;
        let workspace_root = tempdir().unwrap();
        let requested = workspace_root.path().join("manual workspace");

        let switched = env
            .store
            .new_foreground_in_workspace("u1", &requested)
            .await
            .unwrap();

        assert!(switched.parked_alias.is_none());
        let snapshot = env.snapshot("u1").await;
        assert_eq!(
            snapshot.foreground.workspace_dir,
            std::fs::canonicalize(&requested).unwrap()
        );
        assert!(requested.is_dir());
    }

    #[tokio::test]
    async fn temporary_dialog_settings_update_global_defaults() {
        let env = TestEnv::new().await;

        env.store
            .set_model_override_for_active("u1", Some("gpt-global".into()))
            .await
            .unwrap();
        env.store
            .set_reasoning_for_active("u1", Some(ReasoningEffort::High))
            .await
            .unwrap();
        env.store
            .set_context_mode_for_active("u1", Some(ContextMode::OneM))
            .await
            .unwrap();

        let snapshot = env.snapshot("u1").await;
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
        let env = TestEnv::new().await;
        env.store
            .update_settings_for_user("u1", |settings| {
                settings.model_override = Some("gpt-global".into());
                settings.reasoning_effort = Some(ReasoningEffort::Low);
                settings.context_mode = Some(ContextMode::Standard);
                settings.service_tier = Some(ServiceTier::Flex);
            })
            .await
            .unwrap();
        env.store
            .set_foreground_session_id("u1", Some("thread-1".into()))
            .await
            .unwrap();

        env.store
            .set_model_override_for_active("u1", Some("gpt-dialog".into()))
            .await
            .unwrap();
        env.store
            .set_reasoning_for_active("u1", Some(ReasoningEffort::High))
            .await
            .unwrap();
        env.store
            .set_context_mode_for_active("u1", Some(ContextMode::OneM))
            .await
            .unwrap();
        // Production never changes the service tier per dialog; drive the
        // shared routing helper directly to keep the profile branch covered.
        env.store
            .set_active_setting(
                "u1",
                Some(ServiceTier::Fast),
                |settings, value| settings.service_tier = value,
                |profile, value| profile.service_tier = value,
            )
            .await
            .unwrap();

        let snapshot = env.snapshot("u1").await;
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
        // The user-level service_tier stays at Flex because the foreground
        // dialog is saved → `/fast` writes into the per-session profile,
        // not the user-wide default.
        assert_eq!(snapshot.settings.service_tier, Some(ServiceTier::Flex));
        assert_eq!(profile.model_override.as_deref(), Some("gpt-dialog"));
        assert_eq!(profile.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(profile.service_tier, Some(ServiceTier::Fast));
        assert_eq!(profile.context_mode, Some(ContextMode::OneM));

        let cached = env
            .store
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
        let env = TestEnv::new().await;
        let workspace = tempdir().unwrap();
        env.write_rollout(
            "rollout-2026-04-11T00-00-00-thread-legacy.jsonl",
            r#"{"type":"session_meta","payload":{"id":"thread-legacy","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp/project-legacy"}}
{"type":"turn_context","payload":{"cwd":"/tmp/project-legacy","model":"gpt-5.4","effort":"medium"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":950000}}}
"#,
        )
        .await;
        env.store
            .update_settings_for_user("u1", |settings| {
                settings.model_override = Some("gpt-global".into());
                settings.reasoning_effort = Some(ReasoningEffort::Xhigh);
                settings.context_mode = Some(ContextMode::Standard);
            })
            .await
            .unwrap();
        env.store
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
                        generation: 0,
                    },
                );
                Ok(())
            })
            .await
            .unwrap();

        env.store
            .foreground_from_background("u1", "quill")
            .await
            .unwrap();
        let snapshot = env.snapshot("u1").await;
        let profile = snapshot.foreground.profile.unwrap();
        assert_eq!(profile.model_override.as_deref(), Some("gpt-5.4"));
        assert_eq!(profile.reasoning_effort, Some(ReasoningEffort::Medium));
        assert_eq!(profile.context_mode, Some(ContextMode::OneM));
    }

    #[tokio::test]
    async fn resume_local_session_extracts_profile_and_last_user_message() {
        let env = TestEnv::new().await;
        env.write_rollout(
            "rollout-2026-04-11T00-00-00-thread-local.jsonl",
            r#"{"type":"session_meta","payload":{"id":"thread-local","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp/project-a"}}
{"type":"turn_context","payload":{"cwd":"/tmp/project-a","model":"gpt-5.4","effort":"high"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":950000}}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"You are CodexClaw running behind QQ official bot.\n\nUser message:\n请帮我修复登录接口"}]}}
"#,
        )
        .await;

        let sessions = env
            .store
            .list_disk_sessions(SessionListScope::All)
            .await
            .unwrap();
        assert_eq!(
            sessions[0].last_user_message.as_deref(),
            Some("请帮我修复登录接口")
        );

        env.store
            .resume_disk_session("u1", &sessions[0])
            .await
            .unwrap();
        let snapshot = env.snapshot("u1").await;
        let profile = snapshot.foreground.profile.unwrap();
        assert_eq!(profile.model_override.as_deref(), Some("gpt-5.4"));
        assert_eq!(profile.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(profile.context_mode, Some(ContextMode::OneM));
    }

    #[tokio::test]
    async fn stop_drops_unsaved_local_session() {
        let env = TestEnv::new().await;
        let rollout = env
            .write_rollout("rollout-2026-04-11T00-00-00-thread-1.jsonl", "{}")
            .await;
        env.store
            .set_foreground_session_id("u1", Some("thread-1".into()))
            .await
            .unwrap();
        let result = env.store.stop_foreground("u1").await.unwrap();
        assert!(result.dropped_unsaved);
        assert!(!rollout.exists());
    }

    #[tokio::test]
    async fn save_then_stop_keeps_rollout_without_parking_to_background() {
        // Mirrors interactive cron finish for SessionStrategy::Persistent:
        // end the dialog without a bg entry, but keep the rollout so the job's
        // stored thread id can be resumed on the next run.
        let env = TestEnv::new().await;
        let rollout = env
            .write_rollout("rollout-2026-04-11T00-00-00-thread-keep.jsonl", "{}")
            .await;
        env.store
            .set_foreground_session_id("u1", Some("thread-keep".into()))
            .await
            .unwrap();
        assert!(env.store.save_foreground("u1").await.unwrap());
        let result = env.store.stop_foreground("u1").await.unwrap();
        assert!(!result.dropped_unsaved);
        assert!(result.saved);
        assert!(rollout.exists());
        let snapshot = env.snapshot("u1").await;
        assert!(snapshot.background.is_empty());
        assert!(snapshot.foreground.session_id.is_none());
    }

    #[tokio::test]
    async fn stop_keeps_shared_workspace_for_unsaved_temporary_dialog() {
        let env = TestEnv::new().await;
        let before = env.snapshot("u1").await;
        let old_workspace = before.foreground.workspace_dir.clone();
        assert!(old_workspace.exists());

        let result = env.store.stop_foreground("u1").await.unwrap();
        assert!(!result.saved);
        assert!(!result.had_session);
        assert!(!result.dropped_unsaved);
        assert!(old_workspace.exists());
        let snapshot = env.snapshot("u1").await;
        assert_eq!(snapshot.foreground.workspace_dir, old_workspace);
    }

    #[tokio::test]
    async fn imports_system_session_and_records_profile() {
        // The only test that needs the system codex home to differ from the
        // claw-managed one, so it wires the store up by hand.
        let data = tempdir().unwrap();
        let system_home = tempdir().unwrap();
        let claw_home = tempdir().unwrap();
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

        let store = SessionStore::load_or_init(data.path(), claw_home.path(), system_home.path())
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
        assert_eq!(result.profile.context_mode, Some(ContextMode::OneM));
        let imported = claw_home
            .path()
            .join("sessions/2026/04/11/rollout-2026-04-11T00-00-00-thread-import.jsonl");
        assert!(imported.exists());
    }
}
