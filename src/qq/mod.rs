pub mod api;
pub mod directive;
pub mod gateway;
pub mod render;
pub mod types;

// Façade: the symbols the rest of the crate reaches for, re-exported so callers
// import `crate::qq::X` instead of spelling out the submodule layout.
pub use api::QqApiClient;
pub use directive::{Directive, parse_output};
pub use gateway::spawn_gateway;
pub use render::{PassiveDispatchReport, PassiveTurnEmitter};
pub use types::{C2CMessageEvent, MSG_TYPE_QUOTE, MessageAttachment, MsgElement};
