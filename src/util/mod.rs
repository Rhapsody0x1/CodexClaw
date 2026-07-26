//! Leaf helpers shared across the crate.
//!
//! Everything here is dependency-free with respect to the rest of the codebase
//! (only `std` plus small third-party crates), so any module may use it without
//! creating a cycle. Nothing in `util` may import from another crate module.

pub(crate) mod fs;
pub(crate) mod lang;
pub mod layout;
pub mod path;
pub(crate) mod text;
pub(crate) mod time;
