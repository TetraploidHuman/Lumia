//! Shared runtime primitives: headers, TLS, traps, float-key helpers.

use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::ffi::CStr;

use crate::heap::with_heap;

pub use lumia_abi::{
    list_elem_is_float, tid_base, tid_f_key, tid_f_val, MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS,
    MEMO_IDX_TABLE_BYTES, MEMO_PROCESS_BYTE_CAP, MEMO_TF_MAX_ARGS, MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS,
    TYPE_ADT, TYPE_BYTES, TYPE_CHAR, TYPE_CHANNEL, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_F64,
    TYPE_LIST_IOTA, TYPE_MAP, TYPE_MAP_ASSOC, TYPE_MAP_ASSOC_F64, TYPE_MAP_ASSOC_F64V,
    TYPE_MAP_ASSOC_VF64, TYPE_MAP_F64, TYPE_MAP_F64V, TYPE_MAP_VF64, TYPE_SET, TYPE_SET_ASSOC,
    TYPE_SET_F64, TYPE_STRING, TYPE_TASK,
};

/// Object header placed before payload (24 bytes).
///
/// Stack Lit* layouts must use **3** `i64` header words so `header_from_payload` matches:
/// word0 = `type_id|size`, word1 = `marked|rc`, word2 = `_pad`.
///
/// - `rc`: COW refcount for `TYPE_LIST` / `TYPE_LIST_F64` / `TYPE_ADT`
///   (`RC_SHARED` = immortal; alloc starts at 1). Other type_ids leave `rc` at 0.
/// - `_pad`: **type_id-dependent**
///   - `TYPE_ADT`: packed field masks — low 32 = Float, high 32 = Bool
///     (bit `i` in each half ⇒ field `i`)
///   - All other type_ids: initialized to 0 (lists no longer store RC here)
#[repr(C)]
pub struct ObjectHeader {
    pub type_id: u32,
    pub size: u32,
    pub marked: u32,
    /// COW refcount for List / ADT (see struct contract).
    pub rc: u32,
    /// ADT packed float/bool field masks; otherwise 0.
    pub _pad: u64,
}

const _: () = assert!(std::mem::size_of::<ObjectHeader>() == lumia_abi::OBJECT_HEADER_BYTES);

/// Max frames retained for trap backtraces (DESIGN §2 / error table).
const CALL_STACK_CAP: usize = 256;

/// Fatal runtime error. Linked into user programs as abort (no FFI unwind).
/// Prints the Lumia call stack (pushed by codegen) then aborts.
///
/// **Dual mode (explicit):**
/// - Production (`not(test)`): `process::abort` after printing (no Drop unwind).
/// - Unit tests (`cfg(test)`): `panic!` so `#[should_panic]` can observe the message.
///   Production abort behavior is covered by linked user binaries / e2e traps, not
///   by flipping an env var inside the RT test harness (that would abort the suite).
pub(crate) fn trap_abort(msg: &str) -> ! {
    if let Some(h) = crate::globals::before_trap_hook() {
        h();
    }
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
    /// Parallel map workers must not touch the process heap (scalar Int/Bool/Float only).
    pub(crate) static PAR_WORKER: Cell<bool> = const { Cell::new(false) };
    /// Lumia-managed call stack for trap backtraces (nul-terminated name pointers).
    pub(crate) static CALL_STACK: RefCell<Vec<*const u8>> = const { RefCell::new(Vec::new()) };
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
    // Trust typed List/ADT pointers (same as `list_len_of`); skip process-heap
    // membership. Stack Lit* keep rc==0 / wrong tid and no-op via cow_tid_ok.
    if payload.is_null() {
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
    if payload.is_null() {
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
    // Trust typed List/ADT pointers (same as retain/release).
    if payload.is_null() {
        return false;
    }
    unsafe {
        let h = header_from_payload(payload);
        cow_tid_ok((*h).type_id, adt_ok) && (*h).rc == 1
    }
}

/// Bit `i` of an ADT **float** field mask (low 32 bits of header `_pad`).
///
/// `_pad` packing: bits `0..31` = Float mask, bits `32..63` = Bool mask.
#[inline]
pub(crate) fn adt_float_mask(pad: u64) -> u64 {
    pad & 0xFFFF_FFFF
}

/// Bit `i` of an ADT **bool** field mask (high 32 bits of header `_pad`).
#[inline]
pub(crate) fn adt_bool_mask(pad: u64) -> u64 {
    pad >> 32
}

/// Pack float (lo) + bool (hi) field masks into ADT `_pad`.
#[inline]
pub(crate) fn adt_pack_field_masks(float_m: u64, bool_m: u64) -> u64 {
    (float_m & 0xFFFF_FFFF) | ((bool_m & 0xFFFF_FFFF) << 32)
}

/// Bit `i` of a float field mask (also accepts packed `_pad` — uses low half).
#[inline]
pub(crate) fn adt_float_slot(mask: u64, field_index: usize) -> bool {
    let m = adt_float_mask(mask);
    field_index < 32 && (m & (1u64 << field_index)) != 0
}

/// Bit `i` of an **unpacked** bool field mask (bits `0..31`).
#[inline]
pub(crate) fn adt_bool_slot(bool_mask: u64, field_index: usize) -> bool {
    field_index < 32 && (bool_mask & (1u64 << field_index)) != 0
}

/// Drop one alias retain when `rc > 1` (no-op if unique or immortal).
/// Used by `p = p with {…}` before uniqueness check: `with` desugars to
/// `Let tmp = Name(p)` which retains, while the mut slot does not.
#[inline]
pub(crate) fn cow_rc_drop_alias(payload: *mut u8, adt_ok: bool) {
    if payload.is_null() {
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
    if is_heap_payload_bits(bits) {
        value_rc_retain(bits as *mut u8);
    }
}

#[inline]
pub(crate) fn value_rc_release_bits(bits: i64) {
    if is_heap_payload_bits(bits) {
        value_rc_release(bits as *mut u8);
    }
}

/// After shallow-copying an ADT, bump RC on nested List/ADT fields (shared).
///
/// Prefer heap-membership over `_pad` float bits (mistag-safe), matching ADT mark.
pub(crate) unsafe fn adt_retain_nested_fields(obj: *mut u8) {
    if obj.is_null() {
        return;
    }
    let h = header_from_payload(obj);
    if tid_base((*h).type_id) != TYPE_ADT {
        return;
    }
    let words = ((*h).size as usize) / 8;
    let nfields = words.saturating_sub(1);
    let base = obj as *const i64;
    for i in 0..nfields {
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

/// Generational mark-sweep (young list + old tenure; process-global `Heap`).
/// ZST facade: all mutable state lives under `with_heap`. Pluggable ARC/`--mm`
/// backends remain a future option (see Todo); do not reintroduce a TLS
/// "backend" cell that only forwards to the process heap.
pub struct MarkSweep;

/// Heap generation of a live payload (absent ⇒ not a managed heap object).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeapGen {
    Young,
    Old,
}

/// Single membership probe against the process heap (at most two set lookups).
pub(crate) fn heap_gen(payload: *mut u8) -> Option<HeapGen> {
    if payload.is_null() {
        return None;
    }
    let h = header_from_payload(payload);
    with_heap(|heap| {
        if !heap.contains_header(h) {
            return None;
        }
        if heap.is_old_header(h) {
            Some(HeapGen::Old)
        } else {
            Some(HeapGen::Young)
        }
    })
}

pub(crate) fn is_heap_payload(payload: *mut u8) -> bool {
    heap_gen(payload).is_some()
}

/// Cheap filter before [`is_heap_payload`]: managed payloads are non-null and
/// 8-byte aligned (header + payload from `alloc`). FunRef low-bit tags and most
/// small Int/Bool immediates fail this without taking the heap Mutex.
#[inline]
pub(crate) fn may_be_heap_payload_bits(bits: i64) -> bool {
    let u = bits as usize;
    u != 0 && u.is_multiple_of(8)
}

/// [`may_be_heap_payload_bits`] then [`is_heap_payload`] — prefer for i64 field words.
#[inline]
pub(crate) fn is_heap_payload_bits(bits: i64) -> bool {
    may_be_heap_payload_bits(bits) && is_heap_payload(bits as *mut u8)
}

pub(crate) fn is_young_payload(payload: *mut u8) -> bool {
    matches!(heap_gen(payload), Some(HeapGen::Young))
}

pub(crate) struct GcInhibitGuard;
impl GcInhibitGuard {
    pub(crate) fn enter() -> Self {
        crate::heap::with_heap(|h| {
            h.gc_inhibit = h.gc_inhibit.saturating_add(1);
        });
        Self
    }
}
impl Drop for GcInhibitGuard {
    fn drop(&mut self) {
        crate::heap::with_heap(|h| {
            h.gc_inhibit = h.gc_inhibit.saturating_sub(1);
        });
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
