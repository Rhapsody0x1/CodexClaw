rust_i18n::i18n!("locales", fallback = "en");

pub mod app;
pub mod codex;
pub mod commands;
pub mod config;
pub mod memory;
pub mod model;
pub mod qq;
pub mod scheduler;
pub mod self_update;
pub mod session;
pub mod shadow;
pub mod skills;
pub mod util;

/// The inbound-message value types now live in [`model::message`]; re-exported
/// at the crate root so `crate::message::*` keeps resolving.
pub use model::message;
