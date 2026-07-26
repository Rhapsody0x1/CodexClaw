//! Pure value types shared across the crate.
//!
//! Everything here is data plus the formatting/parsing that belongs to the data
//! itself: no I/O, no crate-level services. Keeping these below `config`,
//! `session` and `scheduler` is what stops those three from forming a cycle
//! around a `CronJob` map and a `ReasoningEffort` field.

pub mod cron;
pub mod message;
pub mod settings;

#[cfg(test)]
mod wire_compat;
