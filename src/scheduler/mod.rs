pub mod cli;
pub mod cron_expr;
pub mod interactive;
pub mod loop_;
pub mod runner;
pub mod store;

// Façade: the symbols the rest of the crate reaches for, re-exported so callers
// import `crate::scheduler::X` instead of spelling out the submodule layout.
// `cli::run` stays behind its submodule: it is the `codex-claw cron` CLI entry
// point, and a bare `scheduler::run` would read like the tick loop.
pub use cron_expr::next_after;
pub use interactive::{finish_job_for_owner, on_fg_turn_completed, pending_for_owner};
pub use loop_::Scheduler;
pub use store::{queue_pending_delivery, remove_job_files, take_pending_deliveries};
