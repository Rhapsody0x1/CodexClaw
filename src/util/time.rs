//! Timestamp parsing and formatting shared by the session store, the
//! scheduler and the CLI.

use chrono::{DateTime, ParseError, Utc};

/// Parse an RFC3339 timestamp into UTC, keeping the parse error so callers can
/// attach context to it.
pub(crate) fn parse_utc_strict(value: &str) -> Result<DateTime<Utc>, ParseError> {
    DateTime::parse_from_rfc3339(value).map(|value| value.with_timezone(&Utc))
}

/// Parse an RFC3339 timestamp into UTC, discarding the reason it failed.
pub(crate) fn parse_utc(value: &str) -> Option<DateTime<Utc>> {
    parse_utc_strict(value).ok()
}

/// Compact UTC stamp usable inside a file or directory name.
pub(crate) fn ts_slug(value: DateTime<Utc>) -> String {
    value.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Render an optional timestamp as RFC3339, or `fallback` when it is absent.
pub(crate) fn fmt_rfc3339_or(value: Option<DateTime<Utc>>, fallback: &str) -> String {
    value
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| fallback.to_string())
}

/// Human-readable "how long ago" for user-facing lists. Recent moments use
/// duration buckets (locale-aware); anything a week old or more falls back to
/// a short calendar date rendered in `tz` — month/day within the current
/// year, a full date across years.
pub(crate) fn fmt_relative(
    t: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    tz: chrono_tz::Tz,
    locale: &str,
) -> String {
    use rust_i18n::t;
    let delta = now.signed_duration_since(t);
    if delta < chrono::Duration::minutes(1) {
        return t!("time.just_now", locale = locale).into_owned();
    }
    if delta < chrono::Duration::hours(1) {
        return t!("time.minutes_ago", n = delta.num_minutes(), locale = locale).into_owned();
    }
    if delta < chrono::Duration::hours(24) {
        return t!("time.hours_ago", n = delta.num_hours(), locale = locale).into_owned();
    }
    if delta < chrono::Duration::days(7) {
        return t!("time.days_ago", n = delta.num_days(), locale = locale).into_owned();
    }
    short_date(t, now, tz, locale)
}

/// `fmt_relative`, with a placeholder for "never".
pub(crate) fn fmt_relative_or(
    t: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
    tz: chrono_tz::Tz,
    locale: &str,
) -> String {
    t.map(|value| fmt_relative(value, now, tz, locale))
        .unwrap_or_else(|| "—".to_string())
}

/// Human-readable "when will it run" for scheduled tasks.
pub(crate) fn fmt_next(
    t: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    tz: chrono_tz::Tz,
    locale: &str,
) -> String {
    use chrono::Datelike;
    use rust_i18n::t;
    let delta = t.signed_duration_since(now);
    if delta < chrono::Duration::minutes(1) {
        return t!("time.soon", locale = locale).into_owned();
    }
    if delta < chrono::Duration::hours(1) {
        return t!("time.in_minutes", n = delta.num_minutes(), locale = locale).into_owned();
    }
    let local = t.with_timezone(&tz);
    let local_now = now.with_timezone(&tz);
    let time = local.format("%H:%M").to_string();
    if local.date_naive() == local_now.date_naive() {
        return t!("time.today_at", time = time, locale = locale).into_owned();
    }
    if local.date_naive() == local_now.date_naive() + chrono::Duration::days(1) {
        return t!("time.tomorrow_at", time = time, locale = locale).into_owned();
    }
    if local.year() == local_now.year() {
        format!("{} {}", short_date(t, now, tz, locale), time)
    } else {
        format!("{} {}", local.format("%Y-%m-%d"), time)
    }
}

fn short_date(
    t: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    tz: chrono_tz::Tz,
    locale: &str,
) -> String {
    use chrono::Datelike;
    use rust_i18n::t;
    let local = t.with_timezone(&tz);
    let local_now = now.with_timezone(&tz);
    if local.year() == local_now.year() {
        t!(
            "time.date_same_year",
            m = local.month(),
            d = local.day(),
            locale = locale
        )
        .into_owned()
    } else {
        local.format("%Y-%m-%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_utc_converts_offsets_to_utc() {
        let parsed = parse_utc("2024-05-01T08:30:00+08:00").unwrap();
        assert_eq!(parsed.to_rfc3339(), "2024-05-01T00:30:00+00:00");
        assert!(parse_utc("not a timestamp").is_none());
    }

    #[test]
    fn parse_utc_strict_keeps_the_failure_reason() {
        assert!(parse_utc_strict("2024-05-01T00:00:00Z").is_ok());
        assert!(!parse_utc_strict("nope").unwrap_err().to_string().is_empty());
    }

    #[test]
    fn fmt_relative_buckets() {
        let tz: chrono_tz::Tz = "Asia/Shanghai".parse().unwrap();
        let now = parse_utc("2026-07-26T12:00:00Z").unwrap();
        let cases = [
            ("2026-07-26T11:59:30Z", "刚刚"),
            ("2026-07-26T11:55:00Z", "5 分钟前"),
            ("2026-07-26T09:00:00Z", "3 小时前"),
            ("2026-07-24T12:00:00Z", "2 天前"),
            ("2026-05-13T12:00:00Z", "5月13日"),
            ("2025-05-13T12:00:00Z", "2025-05-13"),
        ];
        for (input, expected) in cases {
            let t = parse_utc(input).unwrap();
            assert_eq!(fmt_relative(t, now, tz, "zh"), expected, "input {input}");
        }
        assert_eq!(fmt_relative_or(None, now, tz, "zh"), "—");
    }

    #[test]
    fn fmt_next_buckets_use_display_timezone() {
        let tz: chrono_tz::Tz = "Asia/Shanghai".parse().unwrap();
        // 2026-07-26 20:00 Beijing
        let now = parse_utc("2026-07-26T12:00:00Z").unwrap();
        let cases = [
            ("2026-07-26T12:00:30Z", "马上"),
            ("2026-07-26T12:30:00Z", "30 分钟后"),
            ("2026-07-26T14:00:00Z", "今天 22:00"),
            // 02:00 UTC next day is still "tomorrow 10:00" in Beijing
            ("2026-07-27T02:00:00Z", "明天 10:00"),
            ("2026-08-01T02:00:00Z", "8月1日 10:00"),
            ("2027-01-01T02:00:00Z", "2027-01-01 10:00"),
        ];
        for (input, expected) in cases {
            let t = parse_utc(input).unwrap();
            assert_eq!(fmt_next(t, now, tz, "zh"), expected, "input {input}");
        }
    }

    #[test]
    fn ts_slug_is_filename_safe() {
        let value = parse_utc("2024-05-01T00:30:00Z").unwrap();
        assert_eq!(ts_slug(value), "20240501T003000Z");
    }

    #[test]
    fn fmt_rfc3339_or_uses_the_fallback_when_absent() {
        let value = parse_utc("2024-05-01T00:30:00Z").unwrap();
        assert_eq!(
            fmt_rfc3339_or(Some(value), "-"),
            "2024-05-01T00:30:00+00:00"
        );
        assert_eq!(fmt_rfc3339_or(None, "-"), "-");
        assert_eq!(fmt_rfc3339_or(None, "none"), "none");
    }
}
