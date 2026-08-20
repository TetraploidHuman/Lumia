//! OS worker / io pool threads for the cooperative scheduler.

use super::sched_core::{sched_notify, sched_wait_while, with_sched, SchedulerKind};
use super::sched_env::{io_threads, worker_threads};
use super::scheduler::{pop_ready_kind, resume_fiber};
use crate::mutator::ensure_mutator_registered;
use std::sync::Once;
use std::time::Duration;

pub(super) fn ensure_pool_for_kind(kind: SchedulerKind) {
    match kind {
        SchedulerKind::Worker if worker_threads() > 0 => start_pool_once(),
        SchedulerKind::Io if io_threads() > 0 => start_pool_once(),
        _ => {}
    }
}

fn start_pool_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for _ in 0..worker_threads() {
            std::thread::Builder::new()
                .name("lumia-sched-worker".into())
                .spawn(|| pool_thread_main(SchedulerKind::Worker))
                .expect("lumia: spawn worker thread");
        }
        for _ in 0..io_threads() {
            std::thread::Builder::new()
                .name("lumia-sched-io".into())
                .spawn(|| pool_thread_main(SchedulerKind::Io))
                .expect("lumia: spawn io thread");
        }
    });
}

fn pool_thread_main(kind: SchedulerKind) {
    ensure_mutator_registered();
    crate::task::ensure_trap_hook();
    let tid = std::thread::current().id();
    loop {
        let fid = loop {
            if let Some(fid) = pop_ready_kind(kind) {
                break fid;
            }
            // Wait only when nothing on this queue is runnable for *this* thread
            // (foreign-home entries must not busy-spin).
            sched_wait_while(
                |s| !s.queue_has_runnable_for(kind, tid),
                Duration::from_millis(50),
            );
        };
        with_sched(|s| s.pool_runners = s.pool_runners.saturating_add(1));
        resume_fiber(fid);
        with_sched(|s| {
            s.pool_runners = s.pool_runners.saturating_sub(1);
        });
        sched_notify();
    }
}
