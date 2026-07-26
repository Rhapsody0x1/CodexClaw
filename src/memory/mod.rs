pub(crate) mod inject;
pub(crate) mod scan;
pub(crate) mod store;

/// The binary constructs the store; the rest of the module tree is internal.
pub use store::MemoryStore;
