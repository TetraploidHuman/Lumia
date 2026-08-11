//! Shared runtime primitives: headers, TLS, traps, float-key helpers.

use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::CStr;
use std::sync::Mutex;

pub use lumia_abi::{
    list_elem_is_float, tid_base, tid_f_key, tid_f_val, MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS,
    MEMO_IDX_TABLE_BYTES, MEMO_PROCESS_BYTE_CAP, MEMO_TF_MAX_ARGS, MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS,
    TYPE_ADT, TYPE_BYTES, TYPE_CHAR, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_F64, TYPE_LIST_IOTA,
    TYPE_MAP, TYPE_MAP_ASSOC, TYPE_MAP_ASSOC_F64, TYPE_MAP_ASSOC_F64V, TYPE_MAP_ASSOC_VF64,
    TYPE_MAP_F64, TYPE_MAP_F64V, TYPE_MAP_VF64, TYPE_SET, TYPE_SET_ASSOC, TYPE_SET_F64,
    TYPE_STRING,
};

/// Object header placed before payload.
#[repr(C)]
pub struct ObjectHeader {
    pub type_id: u32,
    pub size: u32,
    pub marked: u32,
    pub _pad: u32,
}

/// Max frames retained for trap backtraces (DESIGN §2 / error table).
const CALL_STACK_CAP: usize = 256;

/// Fatal runtime error. Linked into user programs as abort (no FFI unwind).
/// Prints the Lumia call stack (pushed by codegen) then aborts.
/// Under `cfg(test)` panics so `#[should_panic]` unit tests can observe the message.
pub(crate) fn trap_abort(msg: &str) -> ! {
    let trace = format_call_stack();
    #[cfg(test)]
    {
        if trace.is_empty() {
            panic!("{msg}");
        }
        panic!("{msg}\nstack trace:\n{trace}");
    }
    #[cfg(not(test))]
    {
        eprintln!("{msg}");
        if !trace.is_empty() {
            eprintln!("stack trace:");
            eprint!("{trace}");
        }
        std::process::abort();
    }
}

fn format_call_stack() -> String {
    CALL_STACK.with(|s| {
        let frames = s.borrow();
        if frames.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        for (i, name) in frames.iter().rev().enumerate() {
            let label = if name.is_null() {
                "<unknown>".into()
            } else {
                unsafe { CStr::from_ptr(*name as *const i8) }
                    .to_string_lossy()
                    .into_owned()
            };
            out.push_str(&format!("  {i}: {label}\n"));
        }
        out
    })
}

pub(crate) fn frame_push(name: *const u8) {
    CALL_STACK.with(|s| {
        let mut frames = s.borrow_mut();
        if frames.len() < CALL_STACK_CAP {
            frames.push(name);
        }
    });
}

pub(crate) fn frame_pop() {
    CALL_STACK.with(|s| {
        s.borrow_mut().pop();
    });
}

thread_local! {
    /// Nursery (young generation). New allocations land here.
    pub(crate) static HEAP_YOUNG: RefCell<Vec<*mut ObjectHeader>> =
        const { RefCell::new(Vec::new()) };
    /// Tenured objects that survived at least one minor collection.
    pub(crate) static HEAP_OLD: RefCell<Vec<*mut ObjectHeader>> =
        const { RefCell::new(Vec::new()) };
    /// O(1) "is tenured" for the write barrier / minor mark.
    pub(crate) static HEAP_OLD_SET: RefCell<HashSet<*mut ObjectHeader>> =
        RefCell::new(HashSet::new());
    /// Old objects that may hold young pointers (remembered set / card table).
    pub(crate) static REMEMBERED: RefCell<HashSet<*mut ObjectHeader>> =
        RefCell::new(HashSet::new());
    /// O(1) membership for `is_heap_payload` (Int bits vs real headers).
    pub(crate) static HEAP_SET: RefCell<HashSet<*mut ObjectHeader>> =
        RefCell::new(HashSet::new());
    pub(crate) static ROOTS: RefCell<Vec<*mut *mut u8>> = const { RefCell::new(Vec::new()) };
    /// Immortal payloads (empty-list singleton, …) — always marked.
    pub(crate) static PERM_OBJECTS: RefCell<Vec<*mut u8>> = const { RefCell::new(Vec::new()) };
    /// Approximate live payload bytes in the nursery.
    pub(crate) static BYTES_YOUNG: RefCell<usize> = const { RefCell::new(0) };
    /// Approximate live payload bytes in the old generation.
    pub(crate) static BYTES_OLD: RefCell<usize> = const { RefCell::new(0) };
    /// Nestable: RT helpers that allocate multiple objects before they are reachable
    /// from roots must hold this to avoid soft-threshold GC UAF.
    pub(crate) static GC_INHIBIT: Cell<u32> = const { Cell::new(0) };
    /// Parallel map workers use a separate TLS heap; allocations there would leak /
    /// never be marked on the main heap. Forbid them (scalar Int/Bool/Float only).
    pub(crate) static PAR_WORKER: Cell<bool> = const { Cell::new(false) };
    /// Lumia-managed call stack for trap backtraces (nul-terminated name pointers).
    static CALL_STACK: RefCell<Vec<*const u8>> = const { RefCell::new(Vec::new()) };
}

/// Soft threshold on young-generation live payload (triggers minor STW).
pub(crate) static YOUNG_LIMIT: Mutex<usize> = Mutex::new(64 * 1024);
/// Soft threshold on old-generation live payload (triggers full STW).
/// Also used as the historical `HEAP_LIMIT` full-heap fuse.
pub(crate) static HEAP_LIMIT: Mutex<usize> = Mutex::new(256 * 1024);

/// Refcount sentinel: immortal / permanently shared (empty-list singleton).
pub(crate) const RC_SHARED: u32 = u32::MAX;

/// Retain a heap List for COW uniqueness (`TYPE_LIST` / `TYPE_LIST_F64` only).
#[inline]
pub(crate) fn list_rc_retain(payload: *mut u8) {
    if payload.is_null() || !is_heap_payload(payload) {
        return;
    }
    unsafe {
        let h = header_from_payload(payload);
        let tid = (*h).type_id;
        if tid_base(tid) != TYPE_LIST {
            return;
        }
        let rc = (*h)._pad;
        if rc != RC_SHARED {
            (*h)._pad = rc.saturating_add(1);
        }
    }
}

/// Release a heap List refcount (does not free; GC reclaims).
#[inline]
pub(crate) fn list_rc_release(payload: *mut u8) {
    if payload.is_null() || !is_heap_payload(payload) {
        return;
    }
    unsafe {
        let h = header_from_payload(payload);
        let tid = (*h).type_id;
        if tid_base(tid) != TYPE_LIST {
            return;
        }
        let rc = (*h)._pad;
        if rc != RC_SHARED && rc > 0 {
            (*h)._pad = rc - 1;
        }
    }
}

#[inline]
pub(crate) fn list_rc_is_unique(payload: *mut u8) -> bool {
    if payload.is_null() || !is_heap_payload(payload) {
        return false;
    }
    unsafe {
        let h = header_from_payload(payload);
        let tid = (*h).type_id;
        tid_base(tid) == TYPE_LIST && (*h)._pad == 1
    }
}

pub(crate) fn header_layout(payload: usize) -> Layout {
    let header = std::mem::size_of::<ObjectHeader>();
    let Some(size) = header.checked_add(payload) else {
        trap_abort("lumia: allocation size overflow");
    };
    Layout::from_size_align(size, 8).unwrap_or_else(|_| {
        trap_abort("lumia: invalid allocation layout");
    })
}

pub(crate) fn payload_ptr(header: *mut ObjectHeader) -> *mut u8 {
    unsafe { (header as *mut u8).add(std::mem::size_of::<ObjectHeader>()) }
}

pub(crate) fn header_from_payload(payload: *mut u8) -> *mut ObjectHeader {
    unsafe { payload.sub(std::mem::size_of::<ObjectHeader>()) as *mut ObjectHeader }
}

/// MmBackend trait — swap mark-sweep for semispace etc. behind same ABI.
pub trait MmBackend {
    fn alloc(&mut self, nbytes: usize, type_id: u32) -> *mut u8;
    fn collect(&mut self);
    fn write_barrier(&mut self, obj: *mut u8, _field: u32, new_ptr: *mut u8) {
        remember_old_to_young(obj, new_ptr as i64);
    }
}

/// Non-moving generational mark-sweep (young nursery + old tenure).
pub struct MarkSweep;

pub(crate) fn is_heap_payload(payload: *mut u8) -> bool {
    if payload.is_null() {
        return false;
    }
    let h = header_from_payload(payload);
    HEAP_SET.with(|set| set.borrow().contains(&h))
}

pub(crate) fn is_old_header(h: *mut ObjectHeader) -> bool {
    HEAP_OLD_SET.with(|set| set.borrow().contains(&h))
}

pub(crate) fn is_young_payload(payload: *mut u8) -> bool {
    if payload.is_null() || !is_heap_payload(payload) {
        return false;
    }
    !is_old_header(header_from_payload(payload))
}

pub(crate) fn is_old_payload(payload: *mut u8) -> bool {
    if payload.is_null() || !is_heap_payload(payload) {
        return false;
    }
    is_old_header(header_from_payload(payload))
}

/// Record a possible old→young edge (remembered set).
pub(crate) fn remember_old_to_young(obj_payload: *mut u8, new_bits: i64) {
    if !is_old_payload(obj_payload) {
        return;
    }
    let new_p = new_bits as *mut u8;
    if !is_young_payload(new_p) {
        return;
    }
    let h = header_from_payload(obj_payload);
    REMEMBERED.with(|r| {
        r.borrow_mut().insert(h);
    });
}

#[cfg(test)]
pub(crate) fn set_gc_limits_for_test(young: usize, old: usize) {
    *YOUNG_LIMIT.lock().unwrap_or_else(|e| e.into_inner()) = young;
    *HEAP_LIMIT.lock().unwrap_or_else(|e| e.into_inner()) = old;
}

#[cfg(test)]
pub(crate) fn gc_live_bytes_for_test() -> (usize, usize) {
    (
        BYTES_YOUNG.with(|y| *y.borrow()),
        BYTES_OLD.with(|o| *o.borrow()),
    )
}

#[cfg(test)]
pub(crate) fn gc_heap_lens_for_test() -> (usize, usize) {
    (
        HEAP_YOUNG.with(|h| h.borrow().len()),
        HEAP_OLD.with(|h| h.borrow().len()),
    )
}

#[cfg(test)]
pub(crate) fn gc_remembered_len_for_test() -> usize {
    REMEMBERED.with(|r| r.borrow().len())
}

pub(crate) struct GcInhibitGuard;
impl GcInhibitGuard {
    pub(crate) fn enter() -> Self {
        GC_INHIBIT.set(GC_INHIBIT.get().saturating_add(1));
        Self
    }
}
impl Drop for GcInhibitGuard {
    fn drop(&mut self) {
        GC_INHIBIT.set(GC_INHIBIT.get().saturating_sub(1));
    }
}

pub(crate) fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

pub(crate) fn float_key_eq(a: i64, b: i64) -> bool {
    f64::from_bits(a as u64) == f64::from_bits(b as u64)
}

pub(crate) fn float_key_hash(bits: i64) -> u64 {
    let f = f64::from_bits(bits as u64);
    if f.is_nan() {
        // All NaNs share a bucket; float_key_eq never reports equal.
        return splitmix64(0x7ff8_0000_0000_0001);
    }
    let canon = if f == 0.0 {
        0.0f64.to_bits()
    } else {
        f.to_bits()
    };
    splitmix64(canon)
}
