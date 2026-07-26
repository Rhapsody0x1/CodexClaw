rust_i18n::i18n!("locales", fallback = "en");

pub mod app;
pub mod codex;
pub(crate) mod commands;
pub mod config;
pub mod memory;
pub(crate) mod model;
pub mod qq;
pub mod scheduler;
pub(crate) mod self_update;
pub mod session;
pub mod shadow;
pub mod skills;
pub(crate) mod util;

/// The inbound-message value types now live in [`model::message`]; re-exported
/// at the crate root so `crate::message::*` keeps resolving.
pub(crate) use model::message;

/// `util` is crate-internal, but the binary's composition root needs the
/// on-disk layout to wire the data directories, so that one type is re-exported
/// here.
pub use util::layout::DataLayout;
