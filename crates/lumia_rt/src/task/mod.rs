//! Stackful Task / Channel runtime (DESIGN §11.2).
//!
//! Cooperative fibers + OS pools for `Scheduler.worker` / `.io` (BUILD §7.7-D).
//! Coroutines are created on first resume (thread-local; corosensei is `!Send`).

mod channel;
mod fiber;
pub(crate) mod sched_core;
pub(crate) mod scheduler;
#[cfg(test)]
mod stress;

pub use channel::{
    lumia_channel_close, lumia_channel_new, lumia_channel_recv, lumia_channel_recv_opt,
    lumia_channel_send,
};
pub use fiber::{
    lumia_scope_cancel, lumia_scope_enter, lumia_scope_leave, lumia_task_join, lumia_task_join_opt,
    lumia_task_spawn, lumia_task_spawn_nullary,
};
pub use scheduler::{
    lumia_abi_handoff_set, lumia_scheduler_drain, lumia_scheduler_kind, snapshot_sched_gc_roots,
    task_runtime_active, SCHEDULER_IO, SCHEDULER_WORKER,
};

pub(super) fn ensure_trap_hook() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        crate::common::set_before_trap(scheduler::cancel_all_scopes);
    });
}
