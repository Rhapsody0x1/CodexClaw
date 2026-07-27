//! Pure state machine for one user's dialog slots.
//!
//! Everything that changes the *topology* of a user's dialogs — which dialog
//! is the foreground, what sits in the background map, alias bookkeeping,
//! the saved-session ledger, the generation counter — goes through the
//! [`Dialogs`] view defined here. Nothing in this module performs I/O or
//! takes locks, so every transition is unit-testable on an in-memory
//! [`UserSessionState`] and the invariants live in exactly one place
//! ([`Dialogs::holds_invariants`], asserted after every mutation in debug
//! builds).
//!
//! The shell around this core is `SessionStore`: it owns the lock, the
//! persistence, and the filesystem side effects (workspace creation and
//! cleanup). Transitions that *imply* filesystem work return a decision —
//! e.g. [`ParkOutcome::cleanup_workspace`] — instead of doing it.

use anyhow::Result;
use rand::{Rng, seq::SliceRandom};
use std::path::{Path, PathBuf};

use crate::model::settings::{
    DialogOrigin, DialogProfile, DialogState, TokenUsageSnapshot, UserSessionState,
};

pub(super) const ALIAS_WORDS: &[&str] = &[
    "sage", "oak", "mint", "lark", "wave", "nova", "reef", "kite", "fern", "dawn", "ember",
    "cedar", "sprout", "peak", "ridge", "orbit", "pixel", "frost", "drift", "meadow", "echo",
    "river", "flint", "atlas", "bloom", "cloud", "maple", "cobalt", "quill", "harbor",
];

/// Structured user-input failures from the dialog state machine. The shell
/// (`app::inbound`) downcasts these to render localized replies; the
/// `Display` text is an English fallback for logs.
#[derive(Debug)]
pub(crate) enum DialogError {
    BackgroundNotFound {
        alias: String,
        available: Vec<String>,
    },
    AliasExists {
        alias: String,
    },
    /// Requested a name the *foreground* dialog will reclaim when it is next
    /// parked. It is absent from the background listing, so saying it "already
    /// exists" would only puzzle the user.
    AliasHeldByForeground {
        alias: String,
    },
    AliasInvalid,
    AliasAllocFailed,
}

impl std::fmt::Display for DialogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BackgroundNotFound { alias, .. } => {
                write!(f, "background session `{alias}` not found")
            }
            Self::AliasExists { alias } => write!(f, "alias `{alias}` already exists"),
            Self::AliasHeldByForeground { alias } => {
                write!(f, "alias `{alias}` belongs to the foreground dialog")
            }
            Self::AliasInvalid => write!(f, "aliases are 1-16 lowercase letters or digits"),
            Self::AliasAllocFailed => write!(f, "could not allocate a new alias"),
        }
    }
}

impl std::error::Error for DialogError {}

/// What [`Dialogs::park`] did with the previous foreground.
#[derive(Debug)]
pub(super) struct ParkOutcome {
    /// The alias the old foreground was parked under, or `None` when it was
    /// an unsaved, unbound temporary and got discarded instead.
    pub(super) parked_alias: Option<String>,
    /// On the discard path only: the requested alias, validated and free.
    /// Nothing was parked under it — the shell decides whether to hold it for
    /// a turn that is still in flight (see `UserSessionState::pending_park_alias`).
    pub(super) reserved_alias: Option<String>,
    /// A workspace directory the *shell* should try to garbage-collect: set
    /// only on the discard path, and never for the shared attachment
    /// workspace or the directory the incoming dialog is about to use.
    pub(super) cleanup_workspace: Option<PathBuf>,
}

/// Mutable view over one user's dialog slots. Construct it inside a state
/// lock, perform one or more transitions, drop it. Each `&mut self` method
/// re-checks the slot invariants in debug builds.
pub(super) struct Dialogs<'a> {
    user: &'a mut UserSessionState,
}

impl<'a> Dialogs<'a> {
    pub(super) fn of(user: &'a mut UserSessionState) -> Self {
        Self { user }
    }

    /// The single way to replace the foreground dialog: bumps the generation
    /// counter so that every swap is observable to the interrupted-turn CAS
    /// binding, even when the outgoing and incoming dialogs are
    /// value-identical (two fresh temporaries — the /stop-during-first-turn
    /// case).
    pub(super) fn install(&mut self, mut dialog: DialogState) {
        dialog.generation = self.user.foreground.generation.wrapping_add(1);
        self.user.foreground = dialog;
        self.assert_invariants();
    }

    /// Park the current foreground into the background (under `requested`
    /// or a generated alias) and install `incoming` in its place. An
    /// unsaved, unbound foreground is discarded instead of parked; in that
    /// case the outcome may name a workspace directory for the shell to
    /// clean up.
    pub(super) fn park(
        &mut self,
        requested: Option<&str>,
        shared_workspace_dir: &Path,
        incoming: DialogState,
    ) -> Result<ParkOutcome> {
        if self.user.foreground.session_id.is_none() && !self.user.foreground.saved {
            // Nothing to park. Validate the requested alias anyway — before
            // touching any slot — so an invalid or taken name is reported
            // instead of being swallowed along with the discarded dialog.
            let reserved_alias = requested
                .map(|alias| self.pick_alias(Some(alias), None))
                .transpose()?;
            let discarded_workspace = self.user.foreground.workspace_dir.clone();
            self.install(incoming);
            let cleanup_workspace = (discarded_workspace != self.user.foreground.workspace_dir
                && discarded_workspace != shared_workspace_dir)
                .then_some(discarded_workspace);
            return Ok(ParkOutcome {
                parked_alias: None,
                reserved_alias,
                cleanup_workspace,
            });
        }
        // With no explicit name, a dialog goes back under the alias it last
        // answered to, so repeated /fg ↔ /bg keeps calling it the same thing.
        // It can only have been taken by something else if the user renamed
        // over it, in which case fall through to a generated alias.
        let sticky = self
            .foreground_claim()
            .filter(|alias| requested.is_none() && !self.user.background.contains_key(alias));
        let alias = match sticky {
            Some(alias) => alias,
            None => self.pick_alias(requested, None)?,
        };
        let mut parked = self.user.foreground.clone();
        parked.saved = true;
        parked.alias = Some(alias.clone());
        if parked.origin == DialogOrigin::Local
            && let Some(session_id) = parked.session_id.clone()
        {
            self.register_saved_session(&session_id);
        }
        self.user.background.insert(alias.clone(), parked);
        record_alias(self.user, &alias);
        self.install(incoming);
        Ok(ParkOutcome {
            parked_alias: Some(alias),
            reserved_alias: None,
            cleanup_workspace: None,
        })
    }

    /// Remove and return the background dialog stored under `alias`,
    /// unregistering the alias.
    pub(super) fn take_background(&mut self, alias: &str) -> Result<DialogState> {
        let dialog = self.user.background.remove(alias).ok_or_else(|| {
            anyhow::Error::new(DialogError::BackgroundNotFound {
                alias: alias.to_string(),
                available: self.user.background_order.iter().rev().cloned().collect(),
            })
        })?;
        self.user.background_order.retain(|value| value != alias);
        self.assert_invariants();
        Ok(dialog)
    }

    /// Store `dialog` in the background under `requested` or a generated
    /// alias and return the alias used.
    pub(super) fn add_background(
        &mut self,
        requested: Option<&str>,
        mut dialog: DialogState,
    ) -> Result<String> {
        let claimed = self.foreground_claim();
        let alias = self.pick_alias(requested, claimed.as_deref())?;
        dialog.alias = Some(alias.clone());
        self.user.background.insert(alias.clone(), dialog);
        record_alias(self.user, &alias);
        self.assert_invariants();
        Ok(alias)
    }

    /// Rename a background alias, keeping its position in the recency order.
    pub(super) fn rename_background(&mut self, old_alias: &str, new_alias: &str) -> Result<()> {
        let new_alias = normalize_alias(new_alias)?;
        reject_taken_alias(
            self.user,
            new_alias.clone(),
            self.foreground_claim().as_deref(),
        )?;
        let Some(mut dialog) = self.user.background.remove(old_alias) else {
            return Err(anyhow::Error::new(DialogError::BackgroundNotFound {
                alias: old_alias.to_string(),
                available: self.user.background_order.iter().rev().cloned().collect(),
            }));
        };
        for value in &mut self.user.background_order {
            if value == old_alias {
                *value = new_alias.clone();
            }
        }
        let mut deduped = Vec::with_capacity(self.user.background_order.len());
        for value in &self.user.background_order {
            if !deduped.contains(value) {
                deduped.push(value.clone());
            }
        }
        self.user.background_order = deduped;
        dialog.alias = Some(new_alias.clone());
        self.user.background.insert(new_alias, dialog);
        self.assert_invariants();
        Ok(())
    }

    /// Mark the foreground saved. Returns false when it already was. A saved
    /// local dialog's thread id enters the saved-session ledger so disk
    /// pruning keeps its rollout.
    pub(super) fn save(&mut self) -> bool {
        if self.user.foreground.saved {
            return false;
        }
        self.user.foreground.saved = true;
        if self.user.foreground.origin == DialogOrigin::Local
            && let Some(session_id) = self.user.foreground.session_id.clone()
        {
            self.register_saved_session(&session_id);
        }
        true
    }

    /// Bind (or unbind, on `None`) the foreground's thread id and profile.
    pub(super) fn bind(&mut self, session_id: Option<String>, profile: DialogProfile) {
        self.user.foreground.session_id = session_id;
        self.user.foreground.profile = self
            .user
            .foreground
            .session_id
            .as_ref()
            .map(|_| profile.clone());
    }

    /// Compare-and-set variant of [`Self::bind`] for turns that ended
    /// without completing: applies only while the foreground is still the
    /// dialog the turn started on. The generation counter is what makes a
    /// temp→temp swap visible — the outgoing and incoming dialogs are
    /// value-identical in every other field. Returns whether the binding was
    /// applied.
    pub(super) fn bind_if_current(
        &mut self,
        expected: &DialogState,
        session_id: String,
        profile: DialogProfile,
    ) -> bool {
        if self.user.foreground.generation != expected.generation
            || self.user.foreground.session_id != expected.session_id
            || self.user.foreground.workspace_dir != expected.workspace_dir
        {
            return false;
        }
        self.bind(Some(session_id), profile);
        true
    }

    /// Land a finished turn's result on the background entry that already
    /// holds the dialog it ran on, i.e. the one `/bg`, `/new` or `/fg` parked
    /// mid-turn. Returns the alias, or `None` when the dialog is not in the
    /// background (it was discarded, or `/stop` dropped it) and the caller
    /// must create an entry instead.
    ///
    /// Identity is the `(generation, session_id)` pair [`Self::park`] copies
    /// verbatim off the foreground. [`Self::install`] hands out each
    /// generation exactly once, so no two dialogs the foreground slot ever
    /// held can collide; entries born directly in the background carry
    /// generation 0 together with a thread id, which no unparked foreground
    /// dialog can match.
    pub(super) fn attach_to_parked(
        &mut self,
        expected: &DialogState,
        session_id: &str,
        profile: DialogProfile,
        usage: Option<TokenUsageSnapshot>,
    ) -> Option<String> {
        let (alias, origin) = self.user.background.iter().find_map(|(alias, dialog)| {
            (dialog.generation == expected.generation
                && dialog.session_id == expected.session_id
                && dialog.workspace_dir == expected.workspace_dir)
                .then(|| (alias.clone(), dialog.origin))
        })?;
        let dialog = self.user.background.get_mut(&alias)?;
        dialog.session_id = Some(session_id.to_string());
        dialog.profile = Some(profile);
        if usage.is_some() {
            dialog.last_usage = usage;
        }
        if origin == DialogOrigin::Local {
            self.register_saved_session(session_id);
        }
        self.assert_invariants();
        Some(alias)
    }

    /// Add `session_id` to the saved-session ledger (idempotent).
    pub(super) fn register_saved_session(&mut self, session_id: &str) {
        if !self
            .user
            .saved_local_session_ids
            .iter()
            .any(|value| value == session_id)
        {
            self.user
                .saved_local_session_ids
                .push(session_id.to_string());
            self.user.saved_local_session_ids.sort();
        }
    }

    /// Drop `session_id` from the saved-session ledger.
    pub(super) fn drop_saved_session(&mut self, session_id: &str) {
        self.user
            .saved_local_session_ids
            .retain(|value| value != session_id);
    }

    /// The name the foreground dialog will reclaim the next time it is
    /// parked. Placing *another* dialog there would silently steal it, so
    /// every alias handed out to something other than the foreground has to
    /// route around it.
    fn foreground_claim(&self) -> Option<String> {
        self.user.foreground.alias.clone()
    }

    /// Resolve `requested`, or invent a free alias. `claimed` names one extra
    /// alias to treat as taken beyond the background keys — see
    /// [`Self::foreground_claim`]; pass `None` when the dialog being placed
    /// *is* the foreground, which cannot collide with its own name.
    fn pick_alias(&mut self, requested: Option<&str>, claimed: Option<&str>) -> Result<String> {
        if let Some(alias) = requested {
            let normalized = normalize_alias(alias)?;
            reject_taken_alias(self.user, normalized.clone(), claimed)?;
            return Ok(normalized);
        }
        let mut rng = rand::thread_rng();
        for _ in 0..(ALIAS_WORDS.len() * 8) {
            let Some(base) = ALIAS_WORDS.choose(&mut rng).copied() else {
                break;
            };
            if !is_taken(self.user, base, claimed) {
                return Ok(base.to_string());
            }
            // Keep alias shape simple: `<word><digits>` and within the existing 16-char limit.
            let suffix = rng.gen_range(2..=9999);
            let candidate = format!("{base}{suffix}");
            if candidate.len() <= 16 && !is_taken(self.user, &candidate, claimed) {
                return Ok(candidate);
            }
        }
        for base in ALIAS_WORDS {
            if !is_taken(self.user, base, claimed) {
                return Ok((*base).to_string());
            }
        }
        for _ in 0..10_000 {
            self.user.alias_seq = self.user.alias_seq.saturating_add(1);
            let index = (self.user.alias_seq % (ALIAS_WORDS.len() as u64)) as usize;
            let base = ALIAS_WORDS[index];
            let candidate = format!("{base}{}", self.user.alias_seq % 10_000);
            if candidate.len() <= 16 && !is_taken(self.user, &candidate, claimed) {
                return Ok(candidate);
            }
        }
        Err(anyhow::Error::new(DialogError::AliasAllocFailed))
    }

    /// The slot invariants: `background_order` is duplicate-free and lists
    /// exactly the keys of `background`, and every entry's own `alias` field
    /// agrees with the key it is filed under.
    fn holds_invariants(&self) -> bool {
        let order = &self.user.background_order;
        let no_dups = order
            .iter()
            .all(|alias| order.iter().filter(|value| *value == alias).count() == 1);
        let order_covers = order
            .iter()
            .all(|alias| self.user.background.contains_key(alias));
        let keys_covered = self
            .user
            .background
            .keys()
            .all(|alias| order.contains(alias));
        let aliases_agree = self
            .user
            .background
            .iter()
            .all(|(alias, dialog)| dialog.alias.as_deref() == Some(alias.as_str()));
        no_dups && order_covers && keys_covered && aliases_agree
    }

    #[inline]
    fn assert_invariants(&self) {
        debug_assert!(
            self.holds_invariants(),
            "dialog slot invariants violated: order={:?} keys={:?}",
            self.user.background_order,
            self.user.background.keys().collect::<Vec<_>>()
        );
    }
}

/// Most recently parked entry that still exists in the background, as an
/// (alias, dialog) pair. Works on a shared reference so read-lock peeks can
/// use it too.
pub(super) fn most_recent_background(user: &UserSessionState) -> Option<(String, DialogState)> {
    user.background_order.iter().rev().find_map(|alias| {
        user.background
            .get(alias)
            .cloned()
            .map(|dialog| (alias.clone(), dialog))
    })
}

fn record_alias(user: &mut UserSessionState, alias: &str) {
    user.background_order.retain(|value| value != alias);
    user.background_order.push(alias.to_string());
}

fn is_taken(user: &UserSessionState, alias: &str, claimed: Option<&str>) -> bool {
    user.background.contains_key(alias) || claimed == Some(alias)
}

/// The error half of [`is_taken`]: which of the two ways an alias can be
/// unavailable, so the shell can explain the right one.
fn reject_taken_alias(user: &UserSessionState, alias: String, claimed: Option<&str>) -> Result<()> {
    if user.background.contains_key(&alias) {
        return Err(anyhow::Error::new(DialogError::AliasExists { alias }));
    }
    if claimed == Some(alias.as_str()) {
        return Err(anyhow::Error::new(DialogError::AliasHeldByForeground {
            alias,
        }));
    }
    Ok(())
}

fn normalize_alias(input: &str) -> Result<String> {
    let alias = input.trim().to_ascii_lowercase();
    let is_valid = !alias.is_empty()
        && alias.len() <= 16
        && alias
            .chars()
            .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit());
    if !is_valid {
        return Err(anyhow::Error::new(DialogError::AliasInvalid));
    }
    Ok(alias)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn user() -> UserSessionState {
        UserSessionState::new(PathBuf::from("/shared"))
    }

    fn bound(session: &str, ws: &str) -> DialogState {
        DialogState {
            session_id: Some(session.to_string()),
            saved: false,
            ..DialogState::new_temporary(PathBuf::from(ws))
        }
    }

    #[test]
    fn install_bumps_generation_monotonically() {
        let mut u = user();
        let start = u.foreground.generation;
        for round in 1..=3u64 {
            Dialogs::of(&mut u).install(DialogState::new_temporary(PathBuf::from("/shared")));
            assert_eq!(u.foreground.generation, start + round);
        }
    }

    #[test]
    fn park_discards_unsaved_unbound_and_reports_cleanup() {
        let mut u = user();
        u.foreground = DialogState::new_temporary(PathBuf::from("/old-ws"));
        let outcome = Dialogs::of(&mut u)
            .park(
                None,
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/new-ws")),
            )
            .unwrap();
        assert!(outcome.parked_alias.is_none());
        assert_eq!(
            outcome.cleanup_workspace.as_deref(),
            Some(Path::new("/old-ws"))
        );
        assert!(u.background.is_empty());
    }

    #[test]
    fn park_never_asks_to_clean_the_shared_workspace() {
        let mut u = user();
        let outcome = Dialogs::of(&mut u)
            .park(
                None,
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/new-ws")),
            )
            .unwrap();
        assert!(outcome.cleanup_workspace.is_none());
    }

    #[test]
    fn park_parks_bound_dialog_under_requested_alias() {
        let mut u = user();
        u.foreground = bound("thread-1", "/ws1");
        let outcome = Dialogs::of(&mut u)
            .park(
                Some("Mint"),
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/shared")),
            )
            .unwrap();
        assert_eq!(outcome.parked_alias.as_deref(), Some("mint"));
        let parked = u.background.get("mint").unwrap();
        assert!(parked.saved, "parking must mark the dialog saved");
        assert_eq!(parked.session_id.as_deref(), Some("thread-1"));
        assert_eq!(
            u.saved_local_session_ids,
            vec!["thread-1".to_string()],
            "a parked local thread enters the saved-session ledger"
        );
    }

    #[test]
    fn park_reserves_the_requested_alias_when_there_is_nothing_to_park() {
        let mut u = user();
        let outcome = Dialogs::of(&mut u)
            .park(
                Some("Main"),
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/shared")),
            )
            .unwrap();
        assert!(outcome.parked_alias.is_none());
        assert_eq!(outcome.reserved_alias.as_deref(), Some("main"));
        assert!(u.background.is_empty(), "nothing was parked");
    }

    #[test]
    fn park_rejects_a_bad_alias_before_discarding_the_foreground() {
        let mut u = user();
        u.background.insert("mint".to_string(), bound("t", "/ws"));
        record_alias(&mut u, "mint");
        let before = u.foreground.clone();
        for requested in ["Mint", "not a valid alias"] {
            let err = Dialogs::of(&mut u)
                .park(
                    Some(requested),
                    Path::new("/shared"),
                    DialogState::new_temporary(PathBuf::from("/new-ws")),
                )
                .unwrap_err();
            assert!(err.downcast_ref::<DialogError>().is_some());
            assert_eq!(
                u.foreground, before,
                "the foreground must survive `{requested}`"
            );
        }
    }

    #[test]
    fn park_rejects_duplicate_alias() {
        let mut u = user();
        u.foreground = bound("thread-1", "/ws1");
        Dialogs::of(&mut u)
            .park(
                Some("mint"),
                Path::new("/shared"),
                bound("thread-2", "/ws2"),
            )
            .unwrap();
        let err = Dialogs::of(&mut u)
            .park(
                Some("mint"),
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/shared")),
            )
            .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn a_dialog_keeps_its_alias_across_repeated_fg_and_bg() {
        let mut u = user();
        u.foreground = bound("thread-1", "/ws1");
        Dialogs::of(&mut u)
            .park(
                Some("main"),
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/shared")),
            )
            .unwrap();

        // Three round trips through the foreground must not rename it.
        for round in 0..3 {
            let dialog = Dialogs::of(&mut u).take_background("main").unwrap();
            Dialogs::of(&mut u).install(dialog);
            let outcome = Dialogs::of(&mut u)
                .park(
                    None,
                    Path::new("/shared"),
                    DialogState::new_temporary(PathBuf::from("/shared")),
                )
                .unwrap();
            assert_eq!(
                outcome.parked_alias.as_deref(),
                Some("main"),
                "round {round} renamed the dialog"
            );
        }
    }

    #[test]
    fn a_generated_alias_is_sticky_too() {
        let mut u = user();
        u.foreground = bound("thread-1", "/ws1");
        let first = Dialogs::of(&mut u)
            .park(
                None,
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/shared")),
            )
            .unwrap()
            .parked_alias
            .unwrap();

        let dialog = Dialogs::of(&mut u).take_background(&first).unwrap();
        Dialogs::of(&mut u).install(dialog);
        let again = Dialogs::of(&mut u)
            .park(
                None,
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/shared")),
            )
            .unwrap();

        assert_eq!(again.parked_alias.as_deref(), Some(first.as_str()));
    }

    #[test]
    fn the_foreground_alias_is_not_handed_out_to_another_dialog() {
        let mut u = user();
        u.foreground = bound("thread-1", "/ws1");
        Dialogs::of(&mut u)
            .park(
                Some("main"),
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/shared")),
            )
            .unwrap();
        // `main` leaves the background listing but stays reserved for it.
        let dialog = Dialogs::of(&mut u).take_background("main").unwrap();
        Dialogs::of(&mut u).install(dialog);

        let err = Dialogs::of(&mut u)
            .add_background(Some("main"), bound("thread-2", "/ws2"))
            .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<DialogError>(),
            Some(DialogError::AliasHeldByForeground { .. })
        ));

        let other = Dialogs::of(&mut u)
            .add_background(None, bound("thread-3", "/ws3"))
            .unwrap();
        assert_ne!(other, "main", "generated aliases must route around it too");

        let err = Dialogs::of(&mut u)
            .rename_background(&other, "main")
            .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<DialogError>(),
            Some(DialogError::AliasHeldByForeground { .. })
        ));
    }

    #[test]
    fn a_sticky_alias_taken_by_someone_else_falls_back_to_a_fresh_one() {
        let mut u = user();
        u.foreground = DialogState {
            alias: Some("main".to_string()),
            ..bound("thread-1", "/ws1")
        };
        // Something else is filed under `main` — a hand-edited state file, or
        // an entry renamed onto it before the invariant existed.
        u.background
            .insert("main".to_string(), bound("thread-2", "/ws2"));
        u.background.get_mut("main").unwrap().alias = Some("main".to_string());
        record_alias(&mut u, "main");

        let outcome = Dialogs::of(&mut u)
            .park(
                None,
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/shared")),
            )
            .unwrap();

        let alias = outcome.parked_alias.unwrap();
        assert_ne!(alias, "main");
        assert_eq!(
            u.background["main"].session_id.as_deref(),
            Some("thread-2"),
            "the squatter must not be overwritten"
        );
    }

    #[test]
    fn attach_to_parked_finds_the_dialog_park_just_stored() {
        let mut u = user();
        u.foreground = bound("thread-1", "/ws1");
        let turn_start = u.foreground.clone();
        Dialogs::of(&mut u)
            .park(
                Some("main"),
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/shared")),
            )
            .unwrap();

        let alias = Dialogs::of(&mut u).attach_to_parked(
            &turn_start,
            "thread-1",
            DialogProfile {
                model_override: Some("gpt-finished".into()),
                ..DialogProfile::default()
            },
            None,
        );

        assert_eq!(alias.as_deref(), Some("main"));
        assert_eq!(u.background.len(), 1);
        assert_eq!(
            u.background["main"]
                .profile
                .as_ref()
                .unwrap()
                .model_override
                .as_deref(),
            Some("gpt-finished")
        );
    }

    #[test]
    fn attach_to_parked_ignores_look_alike_background_entries() {
        let mut u = user();
        // Same thread id, but an entry that was never this foreground: it was
        // loaded straight into the background, so it carries generation 0.
        Dialogs::of(&mut u)
            .add_background(Some("disk"), bound("thread-1", "/ws1"))
            .unwrap();
        let turn_start = DialogState {
            generation: 4,
            ..bound("thread-1", "/ws1")
        };

        let alias = Dialogs::of(&mut u).attach_to_parked(
            &turn_start,
            "thread-1",
            DialogProfile::default(),
            None,
        );

        assert!(
            alias.is_none(),
            "the turn's dialog is not in the background"
        );
    }

    #[test]
    fn take_background_unregisters_the_alias() {
        let mut u = user();
        u.foreground = bound("thread-1", "/ws1");
        Dialogs::of(&mut u)
            .park(
                Some("mint"),
                Path::new("/shared"),
                DialogState::new_temporary(PathBuf::from("/shared")),
            )
            .unwrap();
        let taken = Dialogs::of(&mut u).take_background("mint").unwrap();
        assert_eq!(taken.session_id.as_deref(), Some("thread-1"));
        assert!(u.background_order.is_empty());
        assert!(Dialogs::of(&mut u).take_background("mint").is_err());
    }

    #[test]
    fn rename_background_keeps_recency_position() {
        let mut u = user();
        let mut d = Dialogs::of(&mut u);
        d.add_background(Some("first"), bound("t1", "/a")).unwrap();
        d.add_background(Some("second"), bound("t2", "/b")).unwrap();
        d.add_background(Some("third"), bound("t3", "/c")).unwrap();
        d.rename_background("second", "renamed").unwrap();
        assert_eq!(u.background_order, vec!["first", "renamed", "third"]);
        assert!(u.background.contains_key("renamed"));
        assert!(!u.background.contains_key("second"));
    }

    #[test]
    fn rename_background_rejects_existing_target() {
        let mut u = user();
        let mut d = Dialogs::of(&mut u);
        d.add_background(Some("one"), bound("t1", "/a")).unwrap();
        d.add_background(Some("two"), bound("t2", "/b")).unwrap();
        assert!(d.rename_background("one", "two").is_err());
    }

    #[test]
    fn most_recent_background_is_the_last_recorded() {
        let mut u = user();
        assert!(most_recent_background(&u).is_none());
        let mut d = Dialogs::of(&mut u);
        d.add_background(Some("older"), bound("t1", "/a")).unwrap();
        d.add_background(Some("newer"), bound("t2", "/b")).unwrap();
        assert_eq!(
            most_recent_background(&u)
                .map(|(alias, _)| alias)
                .as_deref(),
            Some("newer")
        );
    }

    #[test]
    fn save_is_idempotent_and_ledgers_local_threads() {
        let mut u = user();
        u.foreground = bound("thread-9", "/ws");
        assert!(Dialogs::of(&mut u).save());
        assert!(!Dialogs::of(&mut u).save(), "second save is a no-op");
        assert_eq!(u.saved_local_session_ids, vec!["thread-9".to_string()]);
    }

    #[test]
    fn bind_if_current_rejects_generation_mismatch() {
        let mut u = user();
        Dialogs::of(&mut u).install(DialogState::new_temporary(PathBuf::from("/shared")));
        let turn_start = u.foreground.clone();
        // temp→temp swap: value-identical foreground, new generation.
        Dialogs::of(&mut u).install(DialogState::new_temporary(PathBuf::from("/shared")));
        let applied = Dialogs::of(&mut u).bind_if_current(
            &turn_start,
            "dead-thread".into(),
            DialogProfile::default(),
        );
        assert!(!applied);
        assert!(u.foreground.session_id.is_none());
    }

    #[test]
    fn bind_if_current_applies_while_unchanged() {
        let mut u = user();
        Dialogs::of(&mut u).install(DialogState::new_temporary(PathBuf::from("/shared")));
        let turn_start = u.foreground.clone();
        let applied = Dialogs::of(&mut u).bind_if_current(
            &turn_start,
            "live-thread".into(),
            DialogProfile::default(),
        );
        assert!(applied);
        assert_eq!(u.foreground.session_id.as_deref(), Some("live-thread"));
    }

    #[test]
    fn invariants_hold_across_a_transition_sequence() {
        let mut u = user();
        let mut d = Dialogs::of(&mut u);
        d.install(bound("t1", "/a"));
        d.park(None, Path::new("/shared"), bound("t2", "/b"))
            .unwrap();
        d.park(Some("keep"), Path::new("/shared"), bound("t3", "/c"))
            .unwrap();
        let _ = d;
        let (restored, _) = most_recent_background(&u).unwrap();
        let mut d = Dialogs::of(&mut u);
        let dialog = d.take_background(&restored).unwrap();
        d.install(dialog);
        assert!(d.holds_invariants());
    }
}
