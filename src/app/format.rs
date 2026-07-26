//! Pure formatting helpers for the turn path: plan-mode blocks, token-usage
//! snapshots and the context-pressure warning.

use rust_i18n::t;

use crate::{
    codex::TokenUsageInfo,
    session::state::TokenUsageSnapshot,
    util::{lang::normalize_lang, text::format_tokens_compact},
};

const CONTEXT_WARNING_THRESHOLD: f64 = 0.80;

/// Pull a `<proposed_plan>...</proposed_plan>` block out of a plan-mode turn's
/// final output. Tolerates extra whitespace and unwrapped code fences.
pub(crate) fn extract_proposed_plan(text: &str) -> Option<String> {
    const OPEN: &str = "<proposed_plan>";
    const CLOSE: &str = "</proposed_plan>";
    let start = text.find(OPEN)? + OPEN.len();
    let relative_end = text[start..].find(CLOSE)?;
    let plan = text[start..start + relative_end].trim();
    if plan.is_empty() {
        None
    } else {
        Some(plan.to_string())
    }
}

/// Follow-up QQ prompt shown after a plan-mode turn emits a `<proposed_plan>`
/// block.
pub(crate) fn build_plan_followup_prompt(lang: &str) -> String {
    let zh = lang.starts_with("zh");
    if zh {
        "Codex 已生成执行计划。接下来请选择：\n\
         /实施          退出 Plan 模式并按此计划执行\n\
         /继续规划      保持 Plan 模式，继续打磨\n\
         /取消计划      丢弃此计划"
            .to_string()
    } else {
        "Codex produced an execution plan. Next step:\n\
         /execute-plan   leave plan mode and run the plan\n\
         /keep-planning  stay in plan mode and refine\n\
         /cancel-plan    discard the plan"
            .to_string()
    }
}

#[cfg(test)]
mod plan_followup_tests {
    use super::extract_proposed_plan;

    #[test]
    fn extracts_plan_block() {
        let text = "Intro text\n<proposed_plan>\n1. Do X\n2. Do Y\n</proposed_plan>\nOutro";
        assert_eq!(
            extract_proposed_plan(text).as_deref(),
            Some("1. Do X\n2. Do Y")
        );
    }

    #[test]
    fn returns_none_without_block() {
        assert_eq!(extract_proposed_plan("no plan here"), None);
    }

    #[test]
    fn returns_none_for_empty_block() {
        assert_eq!(
            extract_proposed_plan("<proposed_plan>   \n</proposed_plan>"),
            None
        );
    }
}

pub(super) fn build_context_warning(snapshot: &TokenUsageSnapshot, lang: &str) -> Option<String> {
    let percent = snapshot.percent_used()?;
    if (percent as f64 / 100.0) < CONTEXT_WARNING_THRESHOLD {
        return None;
    }
    let used_tokens = snapshot.context_tokens()?;
    let lang = normalize_lang(lang);
    Some(
        t!(
            "warnings.context_near_limit",
            percent = percent,
            used = format_tokens_compact(used_tokens),
            total = format_tokens_compact(snapshot.window),
            locale = lang
        )
        .into_owned(),
    )
}

pub(super) fn build_usage_snapshot(
    info: &TokenUsageInfo,
    context_window: Option<u64>,
) -> Option<TokenUsageSnapshot> {
    let window = info.model_context_window.or(context_window)?;
    let context_usage = info.context_window_usage().clone();
    Some(TokenUsageSnapshot {
        total_tokens: context_usage.tokens_in_context_window(),
        window,
        input_tokens: context_usage.input_tokens,
        cached_input_tokens: context_usage.cached_input_tokens,
        output_tokens: context_usage.output_tokens,
        updated_at: chrono::Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use crate::codex::TokenUsageInfo;
    use crate::codex::events::TokenUsage;
    use crate::session::state::fixtures::{legacy_cumulative_usage, usage};

    use super::{build_context_warning, build_usage_snapshot};

    #[test]
    fn context_warning_is_localized_per_language() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            (
                "en",
                &["80% used", "220K used / 272K", "`/compact`"],
                &["`/压缩`"],
            ),
            ("zh", &["`/压缩`"], &["`/compact`"]),
        ];
        for (lang, expected, forbidden) in cases {
            let warning = build_context_warning(&usage(220_000, 272_000), lang)
                .unwrap_or_else(|| panic!("case: {lang} produced no warning"));
            for needle in *expected {
                assert!(
                    warning.contains(needle),
                    "case: {lang} missing {needle:?} in {warning}"
                );
            }
            for needle in *forbidden {
                assert!(
                    !warning.contains(needle),
                    "case: {lang} unexpectedly contains {needle:?} in {warning}"
                );
            }
        }
    }

    #[test]
    fn usage_snapshot_requires_context_window() {
        let info = TokenUsageInfo {
            total_token_usage: TokenUsage {
                input_tokens: 100,
                cached_input_tokens: 0,
                output_tokens: 50,
                reasoning_output_tokens: 0,
                total_tokens: 150,
            },
            last_token_usage: TokenUsage {
                input_tokens: 80,
                cached_input_tokens: 0,
                output_tokens: 20,
                reasoning_output_tokens: 0,
                total_tokens: 100,
            },
            model_context_window: None,
        };

        assert!(build_usage_snapshot(&info, None).is_none());

        let snapshot = build_usage_snapshot(&info, Some(272_000)).expect("snapshot");
        assert_eq!(snapshot.window, 272_000);
        assert_eq!(snapshot.total_tokens, 100);
    }

    #[test]
    fn context_warning_skips_implausible_legacy_cumulative_usage() {
        let warning = build_context_warning(&legacy_cumulative_usage(), "zh");
        assert!(warning.is_none());
    }
}
