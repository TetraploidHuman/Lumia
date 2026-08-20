//! Scheduler kind ids for `scope` / `ScopeEnter` (HIR Int / RT ABI).

/// `0` (and any other value) means the default cooperative scheduler.
pub const SCHEDULER_WORKER: i64 = 1;
pub const SCHEDULER_IO: i64 = 2;
