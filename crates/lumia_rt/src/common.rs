//! Shared runtime primitives: headers, TLS, traps, float-key helpers.

use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use rustc_hash::FxHashSet;

pub use lumia_abi::{
    list_elem_is_float, tid_base, tid_f_key, tid_f_val, MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS,
    MEMO_IDX_TABLE_BYTES, MEMO_PROCESS_BYTE_CAP, MEMO_TF_MAX_ARGS, MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS,
    TYPE_ADT, TYPE_BYTES, TYPE_CHAR, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_F64, TYPE_LIST_IOTA,
    TYPE_MAP, TYPE_MAP_ASSOC, TYPE_MAP_ASSOC_F64, TYPE_MAP_ASSOC_F64V, TYPE_MAP_ASSOC_VF64,
    TYPE_MAP_F64, TYPE_MAP_F64V, TYPE_MAP_VF64, TYPE_SET, TYPE_SET_ASSOC, TYPE_SET_F64,
    TYPE_STRING,
};

/// Object header placed before payload (24 bytes).
///
/// Stack Lit* layouts must use **3** `i64` header words so `header_from_payload` matches:
/// word0 = `type_id|size`, word1 = `marked|rc`, word2 = `_pad`.
///
/// - `rc`: COW refcount for `TYPE_LIST` / `TYPE_LIST_F64` / `TYPE_ADT`
///   (`RC_SHARED` = immortal; alloc starts at 1). Other type_ids leave `rc` at 0.
/// - `_pad`: **type_id-dependent**
///   - `TYPE_ADT`: 64-bit per-field Float layout mask (bit `i` ⇒ field `i` is Float)
///   - All other type_ids: initialized to 0 (lists no longer store RC here)
#[repr(C)]
pub struct ObjectHeader {
    pub type_id: u32,
    pub size: u32,
    pub marked: u32,
    /// COW refcount for List / ADT (see struct contract).
    pub rc: u32,
    /// ADT float-field mask; otherwise 0.
    pub _pad: u64,
}

const _: () = assert!(std::mem::size_of::<ObjectHeader>() == 24);

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
    pub(crate) static HEAP_OLD_SET: RefCell<FxHashSet<*mut ObjectHeader>> =
        RefCell::new(FxHashSet::default());
    /// Old objects that may hold young pointers (remembered set / card table).
    pub(crate) static REMEMBERED: RefCell<FxHashSet<*mut ObjectHeader>> =
        RefCell::new(FxHashSet::default());
    /// O(1) membership for `is_heap_payload` (Int bits vs real headers).
    pub(crate) static HEAP_SET: RefCell<FxHashSet<*mut ObjectHeader>> =
        RefCell::new(FxHashSet::default());
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
    /// Soft threshold on young-generation live payload (triggers minor STW).
    /// TLS so parallel crate tests with different limits do not race.
    pub(crate) static YOUNG_LIMIT: Cell<usize> = const { Cell::new(64 * 1024) };
    /// Soft threshold on old-generation live payload (triggers full STW).
    pub(crate) static HEAP_LIMIT: Cell<usize> = const { Cell::new(256 * 1024) };
}

/// Refcount sentinel: immortal / permanently shared (empty-list singleton).
pub(crate) const RC_SHARED: u32 = u32::MAX;

/// Retain a heap List/ADT for COW uniqueness.
#[inline]
pub(crate) fn list_rc_retain(payload: *mut u8) {
    cow_rc_retain(payload, /*adt_ok=*/ false);
}

/// Release a heap List refcount (does not free; GC reclaims).
#[inline]
pub(crate) fn list_rc_release(payload: *mut u8) {
    cow_rc_release(payload, /*adt_ok=*/ false);
}

#[inline]
pub(crate) fn list_rc_is_unique(payload: *mut u8) -> bool {
    cow_rc_is_unique(payload, /*adt_ok=*/ false)
}

#[inline]
fn cow_tid_ok(tid: u32, adt_ok: bool) -> bool {
    let b = tid_base(tid);
    b == TYPE_LIST || (adt_ok && b == TYPE_ADT)
}

#[inline]
pub(crate) fn cow_rc_retain(payload: *mut u8, adt_ok: bool) {
    if payload.is_null() || !is_heap_payload(payload) {
        return;
    }
    unsafe {
        let h = header_from_payload(payload);
        if !cow_tid_ok((*h).type_id, adt_ok) {
            return;
        }
        let rc = (*h).rc;
        if rc != RC_SHARED {
            (*h).rc = rc.saturating_add(1);
        }
    }
}

#[inline]
pub(crate) fn cow_rc_release(payload: *mut u8, adt_ok: bool) {
    if payload.is_null() || !is_heap_payload(payload) {
        return;
    }
    unsafe {
        let h = header_from_payload(payload);
        if !cow_tid_ok((*h).type_id, adt_ok) {
            return;
        }
        let rc = (*h).rc;
        if rc != RC_SHARED && rc > 0 {
            (*h).rc = rc - 1;
        }
    }
}

#[inline]
pub(crate) fn cow_rc_is_unique(payload: *mut u8, adt_ok: bool) -> bool {
    if payload.is_null() || !is_heap_payload(payload) {
        return false;
    }
    unsafe {
        let h = header_from_payload(payload);
        cow_tid_ok((*h).type_id, adt_ok) && (*h).rc == 1
    }
}

/// Drop one alias retain when `rc > 1` (no-op if unique or immortal).
/// Used by `p = p with {…}` before uniqueness check: `with` desugars to
/// `Let tmp = Name(p)` which retains, while the mut slot does not.
#[inline]
pub(crate) fn cow_rc_drop_alias(payload: *mut u8, adt_ok: bool) {
    if payload.is_null() || !is_heap_payload(payload) {
        return;
    }
    unsafe {
        let h = header_from_payload(payload);
        if !cow_tid_ok((*h).type_id, adt_ok) {
            return;
        }
        let rc = (*h).rc;
        if rc != RC_SHARED && rc > 1 {
            (*h).rc = rc - 1;
        }
    }
}

/// Retain List **or** ADT (codegen aliases).
#[inline]
pub(crate) fn value_rc_retain(payload: *mut u8) {
    cow_rc_retain(payload, /*adt_ok=*/ true);
}

#[inline]
pub(crate) fn value_rc_release(payload: *mut u8) {
    cow_rc_release(payload, /*adt_ok=*/ true);
}

/// Retain a field word if it points at a heap List/ADT (skip floats / immediates).
#[inline]
pub(crate) fn value_rc_retain_bits(bits: i64) {
    let p = bits as *mut u8;
    if is_heap_payload(p) {
        value_rc_retain(p);
    }
}

#[inline]
pub(crate) fn value_rc_release_bits(bits: i64) {
    let p = bits as *mut u8;
    if is_heap_payload(p) {
        value_rc_release(p);
    }
}

/// After shallow-copying an ADT, bump RC on nested List/ADT fields (shared).
///
/// `float_mask` bit `i` ⇒ field `i` is unboxed Float (not a pointer).
pub(crate) unsafe fn adt_retain_nested_fields(obj: *mut u8) {
    if obj.is_null() {
        return;
    }
    let h = header_from_payload(obj);
    if tid_base((*h).type_id) != TYPE_ADT {
        return;
    }
    let mask = (*h)._pad;
    let words = ((*h).size as usize) / 8;
    let nfields = words.saturating_sub(1);
    let base = obj as *const i64;
    for i in 0..nfields {
        if mask & (1u64 << i) != 0 {
            continue;
        }
        value_rc_retain_bits(*base.add(1 + i));
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

/// Heap generation of a live payload (absent ⇒ not a managed heap object).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeapGen {
    Young,
    Old,
}

/// Single membership probe: `HEAP_SET` then `HEAP_OLD_SET` (at most two lookups).
pub(crate) fn heap_gen(payload: *mut u8) -> Option<HeapGen> {
    if payload.is_null() {
        return None;
    }
    let h = header_from_payload(payload);
    if !HEAP_SET.with(|set| set.borrow().contains(&h)) {
        return None;
    }
    if HEAP_OLD_SET.with(|set| set.borrow().contains(&h)) {
        Some(HeapGen::Old)
    } else {
        Some(HeapGen::Young)
    }
}

pub(crate) fn is_heap_payload(payload: *mut u8) -> bool {
    heap_gen(payload).is_some()
}

pub(crate) fn is_old_header(h: *mut ObjectHeader) -> bool {
    HEAP_OLD_SET.with(|set| set.borrow().contains(&h))
}

pub(crate) fn is_young_payload(payload: *mut u8) -> bool {
    matches!(heap_gen(payload), Some(HeapGen::Young))
}

#[inline]
pub(crate) fn is_old_payload(payload: *mut u8) -> bool {
    matches!(heap_gen(payload), Some(HeapGen::Old))
}

/// Record a possible old→young edge (remembered set).
pub(crate) fn remember_old_to_young(obj_payload: *mut u8, new_bits: i64) {
    // Two gen probes max (obj + new); avoids the old 4× HashSet path.
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
    YOUNG_LIMIT.with(|c| c.set(young));
    HEAP_LIMIT.with(|c| c.set(old));
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
