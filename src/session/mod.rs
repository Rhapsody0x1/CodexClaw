mod dialogs;
pub(crate) mod jobs_file;
mod rollout;
pub mod state;
pub(crate) mod store;

// Façade: the store handle and the two list-query types are all the rest of the
// crate reaches for. The value types this module used to own now live in
// [`crate::model::settings`] — `state.rs` is the compatibility shim for those,
// so they are deliberately *not* re-exported here as a third path.
//
// Only `SessionStore` is `pub`: it is what the binary wires up. Everything else
// is `pub(crate)`, so an unused item is reported instead of being excused by a
// hypothetical external consumer.
pub(crate) use dialogs::DialogError;
pub use store::SessionStore;
pub(crate) use store::{DiskSessionMeta, SessionListScope};
