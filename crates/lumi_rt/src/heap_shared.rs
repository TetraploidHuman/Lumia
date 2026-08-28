//! Optional process-wide heap membership mirror (`LUMI_HEAP_SHARED=1`).
//!
//! Default / `cfg(test)`: TLS-only [`crate::common::HEAP_SET`] (parallel cargo tests).
//! With the env flag outside tests: every insert/remove also updates a process-global
//! set so `is_heap_payload` is visible across threads. Young/old vectors stay TLS
//! (per-thread nursery); full multi-mutator nursery sharing remains future work.

use crate::common::ObjectHeader;
use rustc_hash::FxHashSet;
use std::cell::Cell;
use std::sync::{Mutex, OnceLock};

/// Header pointer key for the process-global set (`*mut` is not `Send`/`Sync`).
#[derive(Clone, Copy)]
struct SharedHdr(*mut ObjectHeader);
// SAFETY: headers are only inserted/removed by the owning runtime; membership is
// advisory for `is_heap_payload` across threads under STW or single-mutator use.
unsafe impl Send for SharedHdr {}
unsafe impl Sync for SharedHdr {}

impl SharedHdr {
    fn as_ptr(self) -> *mut ObjectHeader {
        self.0
    }
}

impl PartialEq for SharedHdr {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for SharedHdr {}
impl std::hash::Hash for SharedHdr {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.0 as usize).hash(state);
    }
}

static SHARED_SET: OnceLock<Mutex<FxHashSet<SharedHdr>>> = OnceLock::new();
static SHARED_ENV: OnceLock<()> = OnceLock::new();

thread_local! {
    /// Test override: `None` = follow env / cfg; `Some(true/false)` forces on/off.
    static SHARED_FORCE: Cell<Option<bool>> = const { Cell::new(None) };
}

fn shared_set() -> &'static Mutex<FxHashSet<SharedHdr>> {
    SHARED_SET.get_or_init(|| Mutex::new(FxHashSet::default()))
}

fn init_shared_env() {
    SHARED_ENV.get_or_init(|| {});
}

/// Whether the process-global membership mirror is active on this thread.
pub(crate) fn heap_shared_enabled() -> bool {
    init_shared_env();
    if let Some(force) = SHARED_FORCE.with(|c| c.get()) {
        return force;
    }
    // Tests stay TLS-isolated unless a unit test forces the mirror on.
    if cfg!(test) {
        return false;
    }
    match std::env::var("LUMI_HEAP_SHARED") {
        Ok(v) => {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("on")
        }
        Err(_) => false,
    }
}

pub(crate) fn heap_shared_insert(h: *mut ObjectHeader) {
    if !heap_shared_enabled() || h.is_null() {
        return;
    }
    match shared_set().lock() {
        Ok(mut g) => {
            g.insert(SharedHdr(h));
        }
        Err(_) => crate::common::trap_abort("lumi: heap_shared insert lock poisoned"),
    }
}

pub(crate) fn heap_shared_remove(h: *mut ObjectHeader) {
    if !heap_shared_enabled() || h.is_null() {
        return;
    }
    match shared_set().lock() {
        Ok(mut g) => {
            g.remove(&SharedHdr(h));
        }
        Err(_) => crate::common::trap_abort("lumi: heap_shared remove lock poisoned"),
    }
}

pub(crate) fn heap_shared_contains(h: *mut ObjectHeader) -> bool {
    if !heap_shared_enabled() || h.is_null() {
        return false;
    }
    shared_set()
        .lock()
        .map(|g| g.contains(&SharedHdr(h)))
        .unwrap_or(false)
}

/// Snapshot for STW parallel mark when the shared mirror is active.
/// Always unions the calling thread's TLS `HEAP_SET` so a dual-write miss cannot
/// under-mark local objects.
pub(crate) fn heap_shared_snapshot() -> Option<FxHashSet<*mut ObjectHeader>> {
    if !heap_shared_enabled() {
        return None;
    }
    let mut snap: FxHashSet<*mut ObjectHeader> = shared_set()
        .lock()
        .ok()
        .map(|g| g.iter().map(|h| h.as_ptr()).collect())
        .unwrap_or_default();
    crate::common::HEAP_SET.with(|s| {
        snap.extend(s.borrow().iter().copied());
    });
    Some(snap)
}

#[cfg(test)]
pub(crate) fn heap_shared_set_for_test(on: bool) {
    SHARED_FORCE.with(|c| c.set(Some(on)));
    if !on {
        if let Ok(mut g) = shared_set().lock() {
            g.clear();
        }
    }
}

#[cfg(test)]
pub(crate) fn heap_shared_clear_for_test() {
    SHARED_FORCE.with(|c| c.set(None));
    if let Ok(mut g) = shared_set().lock() {
        g.clear();
    }
}
