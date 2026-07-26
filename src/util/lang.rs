//! Language-tag normalisation shared by the bot commands and the renderers.

/// Fold every accepted spelling of a supported language onto its locale key.
/// Anything unrecognised falls back to `"en"`, matching the i18n fallback.
pub(crate) fn normalize_lang(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "zh" | "zh-cn" | "zh_cn" | "cn" | "chinese" | "中文" => "zh",
        _ => "en",
    }
}

/// Whether `raw` names a language we actually ship, as opposed to merely
/// falling back to English. `normalize_lang` cannot answer this on its own
/// because it maps unknown input and explicit `en` to the same value.
pub(crate) fn is_supported_lang(raw: &str) -> bool {
    normalize_lang(raw) == "zh" || raw.trim().eq_ignore_ascii_case("en")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lang_folds_chinese_spellings() {
        for raw in ["zh", "ZH", "zh-cn", "zh_CN", " cn ", "Chinese", "中文"] {
            assert_eq!(normalize_lang(raw), "zh", "raw={raw:?}");
        }
        for raw in ["en", "EN", " en ", "fr", "", "zh-tw"] {
            assert_eq!(normalize_lang(raw), "en", "raw={raw:?}");
        }
    }

    #[test]
    fn is_supported_lang_separates_english_from_fallback() {
        for raw in ["en", " EN ", "zh", "中文", "chinese"] {
            assert!(is_supported_lang(raw), "raw={raw:?}");
        }
        for raw in ["fr", "zh-tw", "", "english"] {
            assert!(!is_supported_lang(raw), "raw={raw:?}");
        }
    }
}
