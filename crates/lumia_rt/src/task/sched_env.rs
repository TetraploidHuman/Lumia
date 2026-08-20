//! Scheduler pool size env (`LUMIA_SCHED_WORKERS` / `LUMIA_SCHED_IO`).
//!
//! `SCHED_ENV` is registered in [`crate::globals`] (Mutex + optional reload for tests).

use std::sync::Mutex;

/// Cached `LUMIA_SCHED_WORKERS` / `LUMIA_SCHED_IO` (process lifetime; tests may reload).
/// Documented under crate [`globals`](crate::globals) lazy-pattern table.
static SCHED_ENV: Mutex<Option<(usize, usize)>> = Mutex::new(None);

pub(super) fn sched_pool_counts() -> (usize, usize) {
    let mut g = SCHED_ENV.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(c) = *g {
        return c;
    }
    let default = default_pool_size();
    let c = (
        parse_env_usize("LUMIA_SCHED_WORKERS", default),
        parse_env_usize("LUMIA_SCHED_IO", default),
    );
    *g = Some(c);
    c
}

pub(super) fn worker_threads() -> usize {
    sched_pool_counts().0
}

pub(super) fn io_threads() -> usize {
    sched_pool_counts().1
}

/// Default OS-thread pool size when env is unset: host `available_parallelism`, else 1.
/// Tests may pin with `LUMIA_SCHED_WORKERS=0|1` (0 = cooperative / no dedicated pool).
fn default_pool_size() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn parse_env_usize(key: &str, default: usize) -> usize {
    match std::env::var(key) {
        Ok(v) => match v.trim().parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                // Typos like `abc` used to silently keep the default with no warning.
                eprintln!(
                    "lumia: warning: {key}={v:?} ignored \
                     (expected a non-negative integer); keeping default {default}"
                );
                default
            }
        },
        Err(_) => default,
    }
}

#[cfg(test)]
pub(super) fn reload_sched_env_for_test() {
    *SCHED_ENV.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

#[cfg(test)]
#[path = "sched_env_tests.rs"]
mod tests;
