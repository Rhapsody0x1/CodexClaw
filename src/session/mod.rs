pub mod state;
pub mod store;

// Façade: the store handle and the two list-query types are all the rest of the
// crate reaches for. The value types this module used to own now live in
// [`crate::model::settings`] — `state.rs` is the compatibility shim for those,
// so they are deliberately *not* re-exported here as a third path.
pub use store::{DiskSessionMeta, SessionListScope, SessionStore};
