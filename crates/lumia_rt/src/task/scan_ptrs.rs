//! Process-shared GC scan words and other sched metadata pointers.
//!
//! Raw pointers are not `Send` in the type system. These newtypes document the
//! contract and make [`super::SchedCore`] naturally `Send` once coroutines live
//! in [`super::home_coro`] TLS.
//!
//! # Safety
//! Words are only **scanned** or read under heap → sched locks (or on the fiber
//! home thread for yielders); they are not owned stacks moved across OS threads.

use corosensei::Yielder;
use std::cell::Cell;

/// Parked mutator root slots (`*mut *mut u8` — pointer to a rooted i64 cell).
#[derive(Default)]
pub struct ParkedRootSlots(Vec<*mut *mut u8>);

/// Parked debug / trap call-stack frames (`*const u8` code pointers).
#[derive(Default)]
pub struct ParkedCallFrames(Vec<*const u8>);

/// Task handle object pointer (`TYPE_TASK`); live handles are also mutator roots.
#[derive(Clone, Copy)]
pub struct TaskHandlePtr(*mut u8);

/// Yielder address for the running/parked fiber (home-thread only).
#[derive(Clone, Copy)]
pub struct YielderAddr(*const Yielder<(), ()>);

// SAFETY: see module docs — lock-guarded / home-thread metadata only.
unsafe impl Send for ParkedRootSlots {}
unsafe impl Send for ParkedCallFrames {}
unsafe impl Send for TaskHandlePtr {}
unsafe impl Send for YielderAddr {}

impl ParkedRootSlots {
    pub fn from_vec(v: Vec<*mut *mut u8>) -> Self {
        Self(v)
    }

    pub fn into_vec(self) -> Vec<*mut *mut u8> {
        self.0
    }

    pub fn iter(&self) -> impl Iterator<Item = &*mut *mut u8> {
        self.0.iter()
    }
}

impl ParkedCallFrames {
    pub fn from_vec(v: Vec<*const u8>) -> Self {
        Self(v)
    }

    pub fn into_vec(self) -> Vec<*const u8> {
        self.0
    }
}

impl From<Vec<*mut *mut u8>> for ParkedRootSlots {
    fn from(v: Vec<*mut *mut u8>) -> Self {
        Self::from_vec(v)
    }
}

impl From<Vec<*const u8>> for ParkedCallFrames {
    fn from(v: Vec<*const u8>) -> Self {
        Self::from_vec(v)
    }
}

impl TaskHandlePtr {
    pub const fn null() -> Self {
        Self(std::ptr::null_mut())
    }

    pub fn from_raw(p: *mut u8) -> Self {
        Self(p)
    }

    pub fn as_raw(self) -> *mut u8 {
        self.0
    }

    pub fn is_null(self) -> bool {
        self.0.is_null()
    }
}

impl YielderAddr {
    pub const fn null() -> Self {
        Self(std::ptr::null())
    }

    pub fn from_raw(p: *const Yielder<(), ()>) -> Self {
        Self(p)
    }

    pub fn as_raw(self) -> *const Yielder<(), ()> {
        self.0
    }
}

/// Home-thread yielder cell stored on [`super::FiberSlot`].
pub type YielderCell = Cell<YielderAddr>;
