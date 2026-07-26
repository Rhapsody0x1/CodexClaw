//! Re-export shim: the session state document now lives in
//! [`crate::model::settings`]. The paths stay here so `crate::session::state::*`
//! keeps resolving for every existing call site.

pub(crate) use crate::model::settings::default_language;
pub use crate::model::settings::{
    ApprovalPolicySetting, CommandAlias, ContextMode, DialogOrigin, DialogProfile, DialogState,
    ImportedSessionProfile, PendingSetting, PersistedSessionState, ReasoningEffort, ServiceTier,
    SessionSettings, SessionState, TokenUsageSnapshot, UserSessionState,
};

/// Token-usage snapshots shared by the tests in this crate, so the same
/// numbers are not re-typed in `app.rs` / `commands.rs`.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::TokenUsageSnapshot;

    /// Plain in-window usage: only `total_tokens` and `window` matter to the
    /// context-percentage paths, the per-kind counters stay zero.
    pub(crate) fn usage(total_tokens: u64, window: u64) -> TokenUsageSnapshot {
        TokenUsageSnapshot {
            total_tokens,
            window,
            input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            updated_at: chrono::Utc::now(),
        }
    }

    /// Snapshot taken from a legacy rollout where `total_tokens` accumulated
    /// across turns and therefore far exceeds the context window.
    pub(crate) fn legacy_cumulative_usage() -> TokenUsageSnapshot {
        TokenUsageSnapshot {
            total_tokens: 19_668_612,
            window: 1_000_000,
            input_tokens: 19_568_077,
            cached_input_tokens: 18_968_448,
            output_tokens: 100_535,
            updated_at: chrono::Utc::now(),
        }
    }
}

#[cfg(test)]
mod token_usage_tests {
    use super::fixtures::legacy_cumulative_usage;

    #[test]
    fn percent_remaining_returns_none_for_implausible_legacy_cumulative_usage() {
        let snapshot = legacy_cumulative_usage();

        assert_eq!(snapshot.context_tokens(), None);
        assert_eq!(snapshot.percent_remaining(), None);
        assert_eq!(snapshot.percent_used(), None);
    }
}
