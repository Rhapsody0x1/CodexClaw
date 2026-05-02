rust_i18n::i18n!("locales", fallback = "en");

pub mod app;
pub mod codex;
pub mod commands;
pub mod config;
pub mod message;
pub mod qq;
pub mod self_update;
pub mod scheduler;
pub mod session;
pub mod time;

pub fn normalize_lang(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "zh" | "zh-cn" | "zh_cn" | "cn" | "chinese" | "中文" => "zh",
        _ => "en",
    }
}
