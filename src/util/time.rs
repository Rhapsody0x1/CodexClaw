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
