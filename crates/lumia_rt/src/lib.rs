//! Lumia runtime: pluggable GC ABI + first MmBackend (STW mark-sweep).
//!
//! C ABI contract used by codegen:
//! - `lumia_alloc(nbytes, type_id) -> *mut u8`
//! - `lumia_root_push(*mut *mut u8)` / `lumia_root_pop()`
//! - `lumia_write_barrier(obj, field_index, new_ptr)` — no-op under STW mark-sweep
//!   (roots are exact; concurrent/incremental collectors would use a real barrier)
//!   (precise shadow-stack roots; barrier is part of the stable MmBackend ABI and
//!   becomes meaningful for concurrent / generational collectors).
//! - `lumia_gc_collect()`
//! - `lumia_println_int(i64)` / `lumia_println_str(*const u8, len)`

use std::alloc::{alloc, dealloc, Layout};
use std::cell::{Cell, RefCell};
use std::io::{self, Read, Write};
use std::ptr;
use std::sync::Mutex;

/// Object header placed before payload.
#[repr(C)]
pub struct ObjectHeader {
    pub type_id: u32,
    pub size: u32,
    pub marked: u32,
    pub _pad: u32,
}

/// Type ids (descriptor table later).
pub const TYPE_BYTES: u32 = 1;
pub const TYPE_STRING: u32 = 2;
pub const TYPE_LIST: u32 = 3;
pub const TYPE_MAP: u32 = 4;
pub const TYPE_SET: u32 = 5;
pub const TYPE_ADT: u32 = 6;
pub const TYPE_CHAR: u32 = 7;
/// Heap closure: `[fn_ptr:i64][cap0:i64]…`
pub const TYPE_CLOSURE: u32 = 8;
/// Virtual Int range list: payload `[start:i64][end_exclusive:i64]` (DESIGN §3.5 Iota).
pub const TYPE_LIST_IOTA: u32 = 9;
/// Map/Set whose keys/elements are unboxed Float bits; eq/hash use IEEE (DESIGN §2.1).
pub const TYPE_MAP_F64: u32 = 10;
pub const TYPE_SET_F64: u32 = 11;
/// Map/Set without Hash — linear forever (DESIGN AssocList).
pub const TYPE_MAP_ASSOC: u32 = 12;
pub const TYPE_SET_ASSOC: u32 = 13;
/// List of unboxed Float bits; structural `==` / hash use IEEE (DESIGN §2.1).
pub const TYPE_LIST_F64: u32 = 14;
/// Map with Float values (Int/ADT keys); value `==` uses IEEE.
pub const TYPE_MAP_VF64: u32 = 15;
/// Map with Float keys and Float values.
pub const TYPE_MAP_F64V: u32 = 16;

/// Fatal runtime error. Linked into user programs as abort (no FFI unwind).
/// Under `cfg(test)` panics so `#[should_panic]` unit tests can observe the message.
fn trap_abort(msg: &str) -> ! {
    #[cfg(test)]
    {
        panic!("{msg}");
    }
    #[cfg(not(test))]
    {
        eprintln!("{msg}");
        std::process::abort();
    }
}

thread_local! {
    static HEAP: RefCell<Vec<*mut ObjectHeader>> = const { RefCell::new(Vec::new()) };
    static ROOTS: RefCell<Vec<*mut *mut u8>> = const { RefCell::new(Vec::new()) };
    /// Immortal payloads (empty-list singleton, …) — always marked.
    static PERM_OBJECTS: RefCell<Vec<*mut u8>> = const { RefCell::new(Vec::new()) };
    static BYTES_ALLOCATED: RefCell<usize> = const { RefCell::new(0) };
    /// Nestable: RT helpers that allocate multiple objects before they are reachable
    /// from roots must hold this to avoid soft-threshold GC UAF.
    static GC_INHIBIT: Cell<u32> = const { Cell::new(0) };
    /// Parallel map workers use a separate TLS heap; allocations there would leak /
    /// never be marked on the main heap. Forbid them (scalar Int/Bool/Float only).
    static PAR_WORKER: Cell<bool> = const { Cell::new(false) };
}

static HEAP_LIMIT: Mutex<usize> = Mutex::new(256 * 1024);

fn header_layout(payload: usize) -> Layout {
    let size = std::mem::size_of::<ObjectHeader>() + payload;
    Layout::from_size_align(size, 8).unwrap()
}

fn payload_ptr(header: *mut ObjectHeader) -> *mut u8 {
    unsafe { (header as *mut u8).add(std::mem::size_of::<ObjectHeader>()) }
}

fn header_from_payload(payload: *mut u8) -> *mut ObjectHeader {
    unsafe { payload.sub(std::mem::size_of::<ObjectHeader>()) as *mut ObjectHeader }
}

/// MmBackend trait — swap mark-sweep for semispace etc. behind same ABI.
pub trait MmBackend {
    fn alloc(&mut self, nbytes: usize, type_id: u32) -> *mut u8;
    fn collect(&mut self);
    fn write_barrier(&mut self, _obj: *mut u8, _field: u32, _new: *mut u8) {}
}

pub struct MarkSweep;

impl MarkSweep {
    fn mark_from_roots() {
        ROOTS.with(|r| {
            for root in r.borrow().iter() {
                unsafe {
                    let p = **root;
                    // Slot may hold Int / FunRef bits; only mark real heap payloads.
                    if is_heap_payload(p) {
                        mark(header_from_payload(p));
                    }
                }
            }
        });
        PERM_OBJECTS.with(|p| {
            for obj in p.borrow().iter() {
                if is_heap_payload(*obj) {
                    mark(header_from_payload(*obj));
                }
            }
        });
        // Transparent memo tables may hold heap arg/result bits if a Fun's ABI
        // types were misclassified as scalar; keep them alive across GC.
        Self::mark_memo_tables();
    }

    fn mark_i64_if_heap(bits: i64) {
        let p = bits as *mut u8;
        if is_heap_payload(p) {
            mark(header_from_payload(p));
        }
    }

    fn mark_memo_tables() {
        MEMO_L2.with(|t| {
            for table in t.borrow().iter() {
                for slot in &table.slots {
                    if !slot.valid {
                        continue;
                    }
                    for a in slot.args.iter().take(slot.nargs as usize) {
                        Self::mark_i64_if_heap(*a);
                    }
                    Self::mark_i64_if_heap(slot.result);
                }
            }
        });
        MEMO_IDX.with(|t| {
            for table in t.borrow().iter().flatten() {
                for (i, &v) in table.valid.iter().enumerate() {
                    if v != 0 {
                        Self::mark_i64_if_heap(table.values[i]);
                    }
                }
            }
        });
    }

    fn sweep() {
        HEAP.with(|h| {
            let mut heap = h.borrow_mut();
            let mut i = 0;
            while i < heap.len() {
                let obj = heap[i];
                unsafe {
                    if (*obj).marked == 0 {
                        let layout = header_layout((*obj).size as usize);
                        dealloc(obj as *mut u8, layout);
                        heap.swap_remove(i);
                        continue;
                    }
                    (*obj).marked = 0;
                }
                i += 1;
            }
        });
        BYTES_ALLOCATED.with(|b| *b.borrow_mut() = 0);
    }
}

fn mark(obj: *mut ObjectHeader) {
    unsafe {
        if obj.is_null() || (*obj).marked != 0 {
            return;
        }
        (*obj).marked = 1;
        let payload = payload_ptr(obj);
        match (*obj).type_id {
            TYPE_LIST => {
                let n = *(payload as *const i64);
                let base = payload as *const i64;
                for i in 0..n as usize {
                    mark_value(*base.add(1 + i));
                }
            }
            TYPE_LIST_F64 => {
                // Unboxed Float elems — never heap pointers.
            }
            TYPE_LIST_IOTA => {
                // Scalar bounds only — no child pointers.
            }
            TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => {
                set_mark_payload(payload, (*obj).size as usize);
            }
            tid if is_map_tid(tid) => {
                map_mark_payload(payload, (*obj).size as usize);
            }
            TYPE_ADT | TYPE_CLOSURE => {
                let words = ((*obj).size as usize) / 8;
                let base = payload as *const i64;
                // ADT: [tag][fields…]; Closure: [fn_ptr][caps…] — skip word0 as non-heap.
                for i in 1..words {
                    mark_value(*base.add(i));
                }
            }
            _ => {}
        }
    }
}

fn mark_value(x: i64) {
    let p = x as *mut u8;
    if is_heap_payload(p) {
        mark(header_from_payload(p));
    }
}

impl MmBackend for MarkSweep {
    fn alloc(&mut self, nbytes: usize, type_id: u32) -> *mut u8 {
        if PAR_WORKER.get() {
            trap_abort("lumia: heap allocation inside parallel map worker \
                 (use scalar Int/Bool/Float callbacks only)");
        }
        let inhibit = GC_INHIBIT.get();
        if inhibit == 0 {
            let limit = *HEAP_LIMIT.lock().unwrap_or_else(|e| e.into_inner());
            BYTES_ALLOCATED.with(|b| {
                if *b.borrow() >= limit {
                    Self::mark_from_roots();
                    Self::sweep();
                }
            });
        }
        let layout = header_layout(nbytes);
        unsafe {
            let mem = alloc(layout);
            if mem.is_null() {
                trap_abort("lumia: out of memory");
            }
            finish_alloc(mem, nbytes, type_id)
        }
    }

    fn collect(&mut self) {
        Self::mark_from_roots();
        Self::sweep();
    }
}

/// Payload bytes for a HeapList of `len` elements (`[len][e…]`), overflow-safe.
fn list_payload_bytes(len: i64) -> u64 {
    if len < 0 {
        trap_abort("lumia: negative list length");
    }
    (len as u64)
        .checked_add(1)
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as u64)
        .unwrap_or_else(|| trap_abort(&format!("lumia: list too large (len={len})")))
}

struct GcInhibitGuard;
impl GcInhibitGuard {
    fn enter() -> Self {
        GC_INHIBIT.set(GC_INHIBIT.get().saturating_add(1));
        Self
    }
}
impl Drop for GcInhibitGuard {
    fn drop(&mut self) {
        GC_INHIBIT.set(GC_INHIBIT.get().saturating_sub(1));
    }
}

unsafe fn finish_alloc(mem: *mut u8, nbytes: usize, type_id: u32) -> *mut u8 {
    if nbytes > u32::MAX as usize {
        trap_abort("lumia: allocation too large (exceeds u32 size field)");
    }
    let header = mem as *mut ObjectHeader;
    (*header).type_id = type_id;
    (*header).size = nbytes as u32;
    (*header).marked = 0;
    (*header)._pad = 0;
    HEAP.with(|h| h.borrow_mut().push(header));
    BYTES_ALLOCATED.with(|b| *b.borrow_mut() += nbytes);
    payload_ptr(header)
}

thread_local! {
    static BACKEND: RefCell<MarkSweep> = const { RefCell::new(MarkSweep) };
}

#[no_mangle]
pub extern "C" fn lumia_alloc(nbytes: u64, type_id: u32) -> *mut u8 {
    BACKEND.with(|b| b.borrow_mut().alloc(nbytes as usize, type_id))
}

#[no_mangle]
pub extern "C" fn lumia_root_push(slot: *mut *mut u8) {
    ROOTS.with(|r| r.borrow_mut().push(slot));
}

#[no_mangle]
pub extern "C" fn lumia_root_pop() {
    ROOTS.with(|r| {
        let _ = r.borrow_mut().pop();
    });
}

#[no_mangle]
pub extern "C" fn lumia_write_barrier(obj: *mut u8, field: u32, new_ptr: *mut u8) {
    // STW mark-sweep + precise shadow-stack roots: mutations are stopped during
    // collection, so a write barrier is unnecessary. Concurrent/incremental GCs
    // must replace this with a real barrier; the ABI stays stable.
    BACKEND.with(|b| b.borrow_mut().write_barrier(obj, field, new_ptr));
}

#[no_mangle]
pub extern "C" fn lumia_gc_collect() {
    BACKEND.with(|b| b.borrow_mut().collect());
}

#[no_mangle]
pub extern "C" fn lumia_println_int(n: i64) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{n}");
}

/// Soft cap so a hostile/huge stdin cannot force unbounded host allocation.
const MAX_STDIN_BYTES: usize = 64 * 1024 * 1024;

/// Read all of stdin into a heap String (UTF-8 bytes).
#[no_mangle]
pub extern "C" fn lumia_read_stdin() -> *mut u8 {
    let mut buf = Vec::new();
    let mut stdin = io::stdin().lock();
    let mut chunk = [0u8; 8192];
    loop {
        let n = match stdin.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        if buf.len().saturating_add(n) > MAX_STDIN_BYTES {
            trap_abort(&format!(
                "lumia: stdin exceeds {MAX_STDIN_BYTES} bytes (soft cap; use smaller input)"
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    lumia_alloc_string(buf.as_ptr(), buf.len() as u64)
}

#[no_mangle]
pub extern "C" fn lumia_str_starts_with(s: *mut u8, prefix: *mut u8) -> i64 {
    with_str_bytes(s, |bytes| {
        with_str_bytes(prefix, |p| {
            if bytes.starts_with(p) {
                1
            } else {
                0
            }
        })
    })
}

#[no_mangle]
pub extern "C" fn lumia_str_ends_with(s: *mut u8, suffix: *mut u8) -> i64 {
    with_str_bytes(s, |bytes| {
        with_str_bytes(suffix, |p| {
            if bytes.ends_with(p) {
                1
            } else {
                0
            }
        })
    })
}

/// Substring search (`haystack.contains(needle)`).
#[no_mangle]
pub extern "C" fn lumia_str_contains(s: *mut u8, needle: *mut u8) -> i64 {
    with_str_bytes(s, |bytes| {
        with_str_bytes(needle, |n| {
            if n.is_empty() || bytes.windows(n.len()).any(|w| w == n) {
                1
            } else {
                0
            }
        })
    })
}

#[no_mangle]
pub extern "C" fn lumia_println_str(ptr: *const u8, len: u64) {
    let mut out = io::stdout().lock();
    if ptr.is_null() {
        let _ = writeln!(out);
        return;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let _ = out.write_all(slice);
    let _ = writeln!(out);
}

#[no_mangle]
pub extern "C" fn lumia_println_bool(b: i8) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{}", if b != 0 { "true" } else { "false" });
}

/// Print a NUL-terminated C string (from LLVM global string ptrs).
#[no_mangle]
pub extern "C" fn lumia_println_cstr(ptr: *const u8) {
    let mut out = io::stdout().lock();
    if ptr.is_null() {
        let _ = writeln!(out);
        return;
    }
    unsafe {
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let _ = out.write_all(slice);
        let _ = writeln!(out);
    }
}

/// Allocate a GC-managed byte buffer (for strings etc.).
#[no_mangle]
pub extern "C" fn lumia_alloc_string(ptr: *const u8, len: u64) -> *mut u8 {
    let dest = lumia_alloc(len, TYPE_STRING);
    if !dest.is_null() && len > 0 {
        unsafe {
            ptr::copy_nonoverlapping(ptr, dest, len as usize);
        }
    }
    dest
}

/// NUL-terminated C string copy of a Lumia String (for `foreign` String arguments).
#[no_mangle]
pub extern "C" fn lumia_string_cstr(s: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if s.is_null() {
        let dest = lumia_alloc(1, TYPE_BYTES);
        unsafe {
            *dest = 0;
        }
        return dest;
    }
    unsafe {
        let n = (*header_from_payload(s)).size as usize;
        let bytes = std::slice::from_raw_parts(s, n);
        if bytes.contains(&0) {
            trap_abort("lumia: String with interior NUL cannot convert to C string");
        }
        let nbytes = (n as u64)
            .checked_add(1)
            .filter(|&b| b <= u32::MAX as u64)
            .unwrap_or_else(|| trap_abort("lumia: cstr buffer too large"));
        let dest = lumia_alloc(nbytes, TYPE_BYTES);
        ptr::copy_nonoverlapping(s, dest, n);
        *dest.add(n) = 0;
        dest
    }
}

/// Build a Lumia String from a NUL-terminated C string (foreign String returns).
#[no_mangle]
pub extern "C" fn lumia_cstr_to_string(cstr: *const u8) -> *mut u8 {
    if cstr.is_null() {
        return lumia_alloc_string(std::ptr::null(), 0);
    }
    unsafe {
        let mut n = 0usize;
        while *cstr.add(n) != 0 {
            n += 1;
            if n > 1 << 28 {
                trap_abort("lumia: cstr too long");
            }
        }
        lumia_alloc_string(cstr, n as u64)
    }
}

fn is_heap_payload(payload: *mut u8) -> bool {
    if payload.is_null() {
        return false;
    }
    let h = header_from_payload(payload);
    HEAP.with(|heap| heap.borrow().iter().any(|&p| p == h))
}

/// Print `x` as a heap String if it is one; ADTs via structural Show; otherwise Int.
#[no_mangle]
pub extern "C" fn lumia_println_auto(x: i64) {
    let p = x as *mut u8;
    if is_heap_payload(p) {
        unsafe {
            let h = header_from_payload(p);
            if (*h).type_id == TYPE_STRING {
                let len = (*h).size as u64;
                lumia_println_str(p, len);
                return;
            }
            if (*h).type_id == TYPE_CHAR {
                let cp = *(p as *const i64) as u32;
                let mut out = io::stdout().lock();
                if let Some(ch) = char::from_u32(cp) {
                    let _ = writeln!(out, "{ch}");
                } else {
                    let _ = writeln!(out, "\u{FFFD}");
                }
                return;
            }
            if (*h).type_id == TYPE_ADT {
                let s = lumia_show(x);
                let len = (*header_from_payload(s)).size as u64;
                lumia_println_str(s, len);
                return;
            }
        }
    }
    lumia_println_int(x);
}

#[no_mangle]
pub extern "C" fn lumia_println_float(n: f64) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{n}");
}

fn map_is_assoc(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    unsafe { (*header_from_payload(map)).type_id == TYPE_MAP_ASSOC }
}

fn set_is_assoc(set: *mut u8) -> bool {
    if set.is_null() {
        return false;
    }
    unsafe { (*header_from_payload(set)).type_id == TYPE_SET_ASSOC }
}

#[inline]
fn is_map_tid(tid: u32) -> bool {
    matches!(
        tid,
        TYPE_MAP | TYPE_MAP_F64 | TYPE_MAP_ASSOC | TYPE_MAP_VF64 | TYPE_MAP_F64V
    )
}

fn map_float_keys(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    matches!(
        unsafe { (*header_from_payload(map)).type_id },
        TYPE_MAP_F64 | TYPE_MAP_F64V
    )
}

fn map_float_vals(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    matches!(
        unsafe { (*header_from_payload(map)).type_id },
        TYPE_MAP_VF64 | TYPE_MAP_F64V
    )
}

fn map_tid_with_flags(float_keys: bool, float_vals: bool) -> u32 {
    match (float_keys, float_vals) {
        (true, true) => TYPE_MAP_F64V,
        (true, false) => TYPE_MAP_F64,
        (false, true) => TYPE_MAP_VF64,
        (false, false) => TYPE_MAP,
    }
}

fn set_float_elems(set: *mut u8) -> bool {
    if set.is_null() {
        return false;
    }
    unsafe { (*header_from_payload(set)).type_id == TYPE_SET_F64 }
}

/// IEEE key equality for unboxed Float bits (±0 equal; NaN ≠ NaN).
fn float_key_eq(a: i64, b: i64) -> bool {
    f64::from_bits(a as u64) == f64::from_bits(b as u64)
}

fn float_key_hash(bits: i64) -> u64 {
    let f = f64::from_bits(bits as u64);
    if f.is_nan() {
        // All NaNs share a bucket; float_key_eq never reports equal.
        return splitmix64(0x7ff8_0000_0000_0001);
    }
    let canon = if f == 0.0 { 0.0f64.to_bits() } else { f.to_bits() };
    splitmix64(canon)
}

fn key_eq(a: i64, b: i64, float_keys: bool) -> bool {
    if float_keys {
        float_key_eq(a, b)
    } else {
        lumia_eq(a, b) != 0
    }
}

fn key_hash(key: i64, float_keys: bool) -> u64 {
    if float_keys {
        float_key_hash(key)
    } else {
        lumia_hash(key)
    }
}

/// Ensure a map uses Float-key IEEE eq/hash.
/// Empty maps may be retagged (fresh alloc); non-empty wrong key sort traps.
fn ensure_map_f64(map: *mut u8) -> *mut u8 {
    if map.is_null() {
        let dest = lumia_alloc(8, TYPE_MAP_F64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(map);
        match (*h).type_id {
            TYPE_MAP_F64 | TYPE_MAP_F64V => map,
            TYPE_MAP | TYPE_MAP_VF64 => {
                if map_count(map) != 0 {
                    trap_abort("lumia: ensure_map_f64 on non-empty Int-key map");
                }
                let tid = map_tid_with_flags(true, (*h).type_id == TYPE_MAP_VF64);
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                dest
            }
            other => trap_abort(&format!("lumia: ensure_map_f64 on type_id={other}")),
        }
    }
}

/// Ensure a map uses IEEE equality for Float values.
fn ensure_map_vf64(map: *mut u8) -> *mut u8 {
    if map.is_null() {
        let dest = lumia_alloc(8, TYPE_MAP_VF64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(map);
        match (*h).type_id {
            TYPE_MAP_VF64 | TYPE_MAP_F64V => map,
            TYPE_MAP | TYPE_MAP_F64 => {
                if map_count(map) != 0 {
                    trap_abort("lumia: ensure_map_vf64 on non-empty non-Float-value map");
                }
                let tid = map_tid_with_flags((*h).type_id == TYPE_MAP_F64, true);
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                dest
            }
            other => trap_abort(&format!("lumia: ensure_map_vf64 on type_id={other}")),
        }
    }
}

fn ensure_set_f64(set: *mut u8) -> *mut u8 {
    if set.is_null() {
        let dest = lumia_alloc(8, TYPE_SET_F64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(set);
        match (*h).type_id {
            TYPE_SET_F64 => set,
            TYPE_SET => {
                if *(set as *const i64) != 0 {
                    trap_abort("lumia: ensure_set_f64 on non-empty Int-elem set");
                }
                let dest = lumia_alloc(8, TYPE_SET_F64);
                *(dest as *mut i64) = 0;
                dest
            }
            other => trap_abort(&format!("lumia: ensure_set_f64 on type_id={other}")),
        }
    }
}

fn map_type_id(map: *mut u8) -> u32 {
    if map.is_null() {
        TYPE_MAP
    } else {
        unsafe { (*header_from_payload(map)).type_id }
    }
}

fn set_type_id(set: *mut u8) -> u32 {
    if set.is_null() {
        TYPE_SET
    } else {
        unsafe { (*header_from_payload(set)).type_id }
    }
}

#[no_mangle]
pub extern "C" fn lumia_ensure_map_f64(map: *mut u8) -> *mut u8 {
    ensure_map_f64(map)
}

#[no_mangle]
pub extern "C" fn lumia_ensure_map_vf64(map: *mut u8) -> *mut u8 {
    ensure_map_vf64(map)
}

#[no_mangle]
pub extern "C" fn lumia_ensure_set_f64(set: *mut u8) -> *mut u8 {
    ensure_set_f64(set)
}

/// Structural equality for scalars and heap objects (DESIGN: recursive `==`).
#[no_mangle]
pub extern "C" fn lumia_eq(a: i64, b: i64) -> i64 {
    // Same pointer/bits is usually equal, but Float-tagged containers hold
    // IEEE elems/keys: NaN ≠ NaN, so reflexivity fails and we must compare.
    if a == b {
        let p = a as *mut u8;
        if is_heap_payload(p) {
            let tid = unsafe { (*header_from_payload(p)).type_id };
            if !matches!(
                tid,
                TYPE_LIST_F64 | TYPE_SET_F64 | TYPE_MAP_F64 | TYPE_MAP_VF64 | TYPE_MAP_F64V
            ) {
                return 1;
            }
            // Fall through to content compare (same object still ok for ±0).
        } else {
            return 1;
        }
    }
    let pa = a as *mut u8;
    let pb = b as *mut u8;
    if !is_heap_payload(pa) || !is_heap_payload(pb) {
        return 0;
    }
    unsafe {
        let ha = header_from_payload(pa);
        let hb = header_from_payload(pb);
        let ta = (*ha).type_id;
        let tb = (*hb).type_id;
        // HeapList ↔ Iota ↔ ListF64: same user type `List`, compare by content.
        if is_list_tid(ta) && is_list_tid(tb) {
            let na = list_len_of(pa);
            let nb = list_len_of(pb);
            if na != nb {
                return 0;
            }
            // Either side tagged Float elems ⇒ IEEE (covers ±0 / NaN).
            let float_elems = ta == TYPE_LIST_F64 || tb == TYPE_LIST_F64;
            for i in 0..na {
                let ea = list_get_of(pa, i);
                let eb = list_get_of(pb, i);
                let ok = if float_elems {
                    float_key_eq(ea, eb)
                } else {
                    lumia_eq(ea, eb) != 0
                };
                if !ok {
                    return 0;
                }
            }
            return 1;
        }
        if ta != tb {
            return 0;
        }
        match ta {
            TYPE_STRING => {
                let na = (*ha).size as usize;
                let nb = (*hb).size as usize;
                if na != nb {
                    return 0;
                }
                let sa = std::slice::from_raw_parts(pa, na);
                let sb = std::slice::from_raw_parts(pb, nb);
                if sa == sb {
                    1
                } else {
                    0
                }
            }
            TYPE_CHAR => {
                let ca = *(pa as *const i64);
                let cb = *(pb as *const i64);
                if ca == cb {
                    1
                } else {
                    0
                }
            }
            TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => set_eq(pa, pb),
            tid if is_map_tid(tid) => map_eq(pa, pb),
            TYPE_ADT => {
                let words_a = ((*ha).size as usize) / 8;
                let words_b = ((*hb).size as usize) / 8;
                if words_a != words_b || words_a == 0 {
                    return 0;
                }
                let ba = pa as *const i64;
                let bb = pb as *const i64;
                for i in 0..words_a {
                    if lumia_eq(*ba.add(i), *bb.add(i)) == 0 {
                        return 0;
                    }
                }
                1
            }
            _ => 0,
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_match_fail() {
    trap_abort("lumia: non-exhaustive match");
}

/// Abort if `cond` is false (0). `msg` is a UTF-8 message (e.g. `path:line: assert failed`).
#[no_mangle]
pub extern "C" fn lumia_assert(cond: i64, msg: *const u8, msg_len: i64) {
    if cond == 0 {
        let text = if msg.is_null() || msg_len <= 0 {
            "lumia: assert failed".to_string()
        } else {
            let slice = unsafe { std::slice::from_raw_parts(msg, msg_len as usize) };
            match std::str::from_utf8(slice) {
                Ok(s) => format!("lumia: {s}"),
                Err(_) => "lumia: assert failed".to_string(),
            }
        };
        eprintln!("{text}");
        std::process::abort();
    }
}

#[no_mangle]
pub extern "C" fn lumia_alloc_char(codepoint: i64) -> *mut u8 {
    let dest = lumia_alloc(8, TYPE_CHAR);
    if dest.is_null() {
        trap_abort("lumia: char OOM");
    }
    unsafe {
        *(dest as *mut i64) = codepoint;
    }
    dest
}

/// Format a value as a heap String (for interpolation).
/// Strings are returned as-is; Chars become one-character strings;
/// ADTs are `#tag(field, …)` via recursive Show; otherwise decimal Int.
#[no_mangle]
pub extern "C" fn lumia_show(x: i64) -> *mut u8 {
    let p = x as *mut u8;
    if is_heap_payload(p) {
        unsafe {
            let h = header_from_payload(p);
            if (*h).type_id == TYPE_STRING {
                return p;
            }
            if (*h).type_id == TYPE_CHAR {
                let cp = *(p as *const i64) as u32;
                let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                return lumia_alloc_string(s.as_ptr(), s.len() as u64);
            }
            if (*h).type_id == TYPE_ADT {
                return show_adt(p);
            }
        }
    }
    let s = x.to_string();
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}

unsafe fn show_adt(payload: *mut u8) -> *mut u8 {
    let words = ((*header_from_payload(payload)).size as usize) / 8;
    let base = payload as *const i64;
    let mut s = String::from("#");
    if words == 0 {
        s.push_str("()");
        return lumia_alloc_string(s.as_ptr(), s.len() as u64);
    }
    let tag = *base;
    s.push_str(&tag.to_string());
    s.push('(');
    for i in 1..words {
        if i > 1 {
            s.push_str(", ");
        }
        let field = lumia_show(*base.add(i));
        with_str_bytes(field, |b| {
            s.push_str(std::str::from_utf8(b).unwrap_or("?"));
        });
    }
    s.push(')');
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}

#[no_mangle]
pub extern "C" fn lumia_show_float(n: f64) -> *mut u8 {
    let s = n.to_string();
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}

#[no_mangle]
pub extern "C" fn lumia_show_bool(b: i8) -> *mut u8 {
    let s = if b != 0 { "true" } else { "false" };
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}

#[no_mangle]
pub extern "C" fn lumia_str_len(s: *mut u8) -> i64 {
    if s.is_null() {
        return 0;
    }
    unsafe { (*header_from_payload(s)).size as i64 }
}

/// Byte-length / element-count for List, Map, Set, or String.
#[no_mangle]
pub extern "C" fn lumia_len(obj: *mut u8) -> i64 {
    if obj.is_null() {
        return 0;
    }
    unsafe {
        let h = header_from_payload(obj);
        match (*h).type_id {
            TYPE_STRING => (*h).size as i64,
            TYPE_LIST | TYPE_LIST_F64 | TYPE_LIST_IOTA => list_len_of(obj),
            TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => *(obj as *const i64),
            tid if is_map_tid(tid) => map_count(obj),
            _ => trap_abort(&format!("lumia: len on unsupported type {}", (*h).type_id)),
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_str_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    // Keep `a`/`b` alive across the destination allocation.
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let na = if a.is_null() {
            0u64
        } else {
            (*header_from_payload(a)).size as u64
        };
        let nb = if b.is_null() {
            0u64
        } else {
            (*header_from_payload(b)).size as u64
        };
        let total = na
            .checked_add(nb)
            .filter(|&t| t <= u32::MAX as u64)
            .unwrap_or_else(|| trap_abort("lumia: string too large to concat"));
        let dest = lumia_alloc(total, TYPE_STRING);
        if dest.is_null() {
            trap_abort("lumia: str concat OOM");
        }
        if na > 0 {
            ptr::copy_nonoverlapping(a, dest, na as usize);
        }
        if nb > 0 {
            ptr::copy_nonoverlapping(b, dest.add(na as usize), nb as usize);
        }
        dest
    }
}

/// Concatenate two Lists or two Strings (same type_id).
#[no_mangle]
pub extern "C" fn lumia_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    let ta = if a.is_null() {
        TYPE_LIST
    } else {
        unsafe { (*header_from_payload(a)).type_id }
    };
    let tb = if b.is_null() {
        ta
    } else {
        unsafe { (*header_from_payload(b)).type_id }
    };
    if ta == TYPE_STRING || tb == TYPE_STRING {
        if ta != TYPE_STRING || tb != TYPE_STRING {
            trap_abort("lumia: concat type mismatch");
        }
        return lumia_str_concat(a, b);
    }
    lumia_list_concat(a, b)
}

fn with_str_bytes<R>(s: *mut u8, f: impl FnOnce(&[u8]) -> R) -> R {
    if s.is_null() {
        return f(&[]);
    }
    unsafe {
        let n = (*header_from_payload(s)).size as usize;
        f(std::slice::from_raw_parts(s, n))
    }
}

fn char_codepoint(ch: i64) -> u32 {
    let p = ch as *mut u8;
    if !p.is_null() && is_heap_payload(p) {
        unsafe {
            if (*header_from_payload(p)).type_id == TYPE_CHAR {
                return *(p as *const i64) as u32;
            }
        }
    }
    ch as u32
}

/// Trim ASCII whitespace from both ends.
#[no_mangle]
pub extern "C" fn lumia_str_trim(s: *mut u8) -> *mut u8 {
    // Copy before alloc — slice aliases heap bytes that GC may free.
    with_str_bytes(s, |bytes| {
        let start = bytes
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(bytes.len());
        let end = bytes
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map(|i| i + 1)
            .unwrap_or(start);
        let owned = bytes[start..end].to_vec();
        lumia_alloc_string(owned.as_ptr(), owned.len() as u64)
    })
}

/// Substring `[start, end)` in byte offsets (clamped).
#[no_mangle]
pub extern "C" fn lumia_str_substring(s: *mut u8, start: i64, end: i64) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let n = bytes.len() as i64;
        let a = start.clamp(0, n) as usize;
        let b = end.clamp(0, n) as usize;
        let b = b.max(a);
        let owned = bytes[a..b].to_vec();
        lumia_alloc_string(owned.as_ptr(), owned.len() as u64)
    })
}

#[no_mangle]
pub extern "C" fn lumia_str_to_lower(s: *mut u8) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let lower: Vec<u8> = bytes.iter().map(|b| b.to_ascii_lowercase()).collect();
        lumia_alloc_string(lower.as_ptr(), lower.len() as u64)
    })
}

#[no_mangle]
pub extern "C" fn lumia_str_to_upper(s: *mut u8) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let upper: Vec<u8> = bytes.iter().map(|b| b.to_ascii_uppercase()).collect();
        lumia_alloc_string(upper.as_ptr(), upper.len() as u64)
    })
}

/// Split `s` on separator Char (or raw codepoint). Returns List[String].
#[no_mangle]
pub extern "C" fn lumia_str_split(s: *mut u8, sep_ch: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let cp = char_codepoint(sep_ch);
    let mut sep_buf = [0u8; 4];
    let sep = match char::from_u32(cp) {
        Some(c) => c.encode_utf8(&mut sep_buf).as_bytes().to_vec(),
        None => vec![cp as u8],
    };
    with_str_bytes(s, |bytes| {
        let mut parts: Vec<*mut u8> = Vec::new();
        if sep.is_empty() {
            parts.push(lumia_alloc_string(bytes.as_ptr(), bytes.len() as u64));
        } else {
            let mut start = 0usize;
            let mut i = 0usize;
            while i + sep.len() <= bytes.len() {
                if &bytes[i..i + sep.len()] == sep.as_slice() {
                    let slice = &bytes[start..i];
                    parts.push(lumia_alloc_string(slice.as_ptr(), slice.len() as u64));
                    i += sep.len();
                    start = i;
                } else {
                    i += 1;
                }
            }
            let slice = &bytes[start..];
            parts.push(lumia_alloc_string(slice.as_ptr(), slice.len() as u64));
        }
        let n = parts.len() as i64;
        let dest = lumia_alloc(list_payload_bytes(n), TYPE_LIST);
        unsafe {
            let dst = dest as *mut i64;
            *dst = n;
            for (i, p) in parts.into_iter().enumerate() {
                *dst.add(1 + i) = p as i64;
            }
        }
        dest
    })
}

/// Prefix of length `n` (clamped).
#[no_mangle]
pub extern "C" fn lumia_list_take(list: *mut u8, n: i64) -> *mut u8 {
    // Iota take can stay virtual: [start, start+take).
    if list_tid(list) == TYPE_LIST_IOTA {
        let len = list_len_of(list);
        let take = if n < 0 {
            0
        } else if n > len {
            len
        } else {
            n
        };
        unsafe {
            let base = list as *const i64;
            let start = *base;
            let end = start
                .checked_add(take)
                .unwrap_or_else(|| trap_abort("lumia: iota take overflow"));
            return lumia_range(start, end);
        }
    }
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let len = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        let take = if n < 0 {
            0
        } else if n > len {
            len
        } else {
            n
        };
        let dest = lumia_alloc(list_payload_bytes(take), heap_list_tid(list));
        if dest.is_null() {
            trap_abort("lumia: list take OOM");
        }
        let dst = dest as *mut i64;
        *dst = take;
        if !list.is_null() && take > 0 {
            let src = list as *const i64;
            for i in 0..take as usize {
                *dst.add(1 + i) = *src.add(1 + i);
            }
        }
        dest
    }
}

/// Reverse element order into a new list.
#[no_mangle]
pub extern "C" fn lumia_list_reverse(list: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    unsafe {
        let len = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        let dest = lumia_alloc(list_payload_bytes(len), heap_list_tid(list));
        if dest.is_null() {
            trap_abort("lumia: list reverse OOM");
        }
        let dst = dest as *mut i64;
        *dst = len;
        if !list.is_null() && len > 0 {
            let src = list as *const i64;
            let n = len as usize;
            for i in 0..n {
                *dst.add(1 + i) = *src.add(n - i);
            }
        }
        dest
    }
}

/// Sort `List[Int]` ascending (stable via slice::sort).
#[no_mangle]
pub extern "C" fn lumia_list_sort(list: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    unsafe {
        let len = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        let n = len as usize;
        let dest = lumia_alloc(list_payload_bytes(len), TYPE_LIST);
        if dest.is_null() {
            trap_abort("lumia: list sort OOM");
        }
        let dst = dest as *mut i64;
        *dst = len;
        if !list.is_null() && n > 0 {
            let src = list as *const i64;
            for i in 0..n {
                *dst.add(1 + i) = *src.add(1 + i);
            }
            let slice = std::slice::from_raw_parts_mut(dst.add(1), n);
            slice.sort();
        }
        dest
    }
}

/// Stable permute of `values` by parallel Ord keys (Int / String / Char).
#[no_mangle]
pub extern "C" fn lumia_list_sort_by_keys(values: *mut u8, keys: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let values = force_heap_list(values);
    let keys = force_heap_list(keys);
    unsafe {
        let n = if values.is_null() {
            0i64
        } else {
            *(values as *const i64)
        };
        let nk = if keys.is_null() {
            0i64
        } else {
            *(keys as *const i64)
        };
        if n != nk {
            trap_abort("lumia: sortBy keys/values length mismatch");
        }
        let dest = lumia_alloc(list_payload_bytes(n), heap_list_tid(values));
        if dest.is_null() {
            trap_abort("lumia: list sortBy OOM");
        }
        let dst = dest as *mut i64;
        *dst = n;
        if n == 0 {
            return dest;
        }
        let mut order: Vec<(i64, usize)> = Vec::with_capacity(n as usize);
        let kbase = keys as *const i64;
        for i in 0..n as usize {
            order.push((*kbase.add(1 + i), i));
        }
        order.sort_by(|a, b| lumia_ord_cmp(a.0, b.0).then(a.1.cmp(&b.1)));
        let vbase = values as *const i64;
        for (w, &(_, i)) in order.iter().enumerate() {
            *dst.add(1 + w) = *vbase.add(1 + i);
        }
        dest
    }
}

/// Total order for Ord scalars: Int/Bool (signed bits), String (bytes), Char (codepoint).
/// Used by `sortBy` and by `<`/`<=`/`>`/`>=` codegen (must not pointer-compare heap values).
fn lumia_ord_cmp(a: i64, b: i64) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a == b {
        return Ordering::Equal;
    }
    let pa = a as *mut u8;
    let pb = b as *mut u8;
    let ha = is_heap_payload(pa);
    let hb = is_heap_payload(pb);
    if !ha && !hb {
        return a.cmp(&b);
    }
    if ha && hb {
        unsafe {
            let ta = (*header_from_payload(pa)).type_id;
            let tb = (*header_from_payload(pb)).type_id;
            if ta != tb {
                trap_abort("lumia: Ord operands have mixed heap types");
            }
            match ta {
                TYPE_STRING => {
                    let na = (*header_from_payload(pa)).size as usize;
                    let nb = (*header_from_payload(pb)).size as usize;
                    let sa = std::slice::from_raw_parts(pa, na);
                    let sb = std::slice::from_raw_parts(pb, nb);
                    sa.cmp(sb)
                }
                TYPE_CHAR => {
                    let ca = *(pa as *const i64);
                    let cb = *(pb as *const i64);
                    ca.cmp(&cb)
                }
                TYPE_ADT => {
                    // Lexicographic: tag then fields (products use tag 0).
                    let words_a = ((*header_from_payload(pa)).size as usize) / 8;
                    let words_b = ((*header_from_payload(pb)).size as usize) / 8;
                    if words_a != words_b {
                        return words_a.cmp(&words_b);
                    }
                    let ba = pa as *const i64;
                    let bb = pb as *const i64;
                    for i in 0..words_a {
                        match lumia_ord_cmp(*ba.add(i), *bb.add(i)) {
                            Ordering::Equal => continue,
                            other => return other,
                        }
                    }
                    Ordering::Equal
                }
                _ => trap_abort(&format!(
                    "lumia: type_id={ta} is not Ord (use Int/Float/Bool/String/Char or Ord ADT)"
                )),
            }
        }
    } else {
        trap_abort("lumia: cannot compare scalar with heap value under Ord");
    }
}

/// C ABI for `<`/`<=`/`>`/`>=`: returns -1 / 0 / 1.
#[no_mangle]
pub extern "C" fn lumia_cmp(a: i64, b: i64) -> i64 {
    match lumia_ord_cmp(a, b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// Join `List[String]` with a separator string.
#[no_mangle]
pub extern "C" fn lumia_list_join(list: *mut u8, sep: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    let sep_bytes = with_str_bytes(sep, |b| b.to_vec());
    let parts: Vec<Vec<u8>> = unsafe {
        let len = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        let mut out = Vec::with_capacity(len as usize);
        if !list.is_null() {
            let base = list as *const i64;
            for i in 0..len as usize {
                let p = *base.add(1 + i) as *mut u8;
                out.push(with_str_bytes(p, |b| b.to_vec()));
            }
        }
        out
    };
    let mut buf: Vec<u8> = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            buf.extend_from_slice(&sep_bytes);
        }
        buf.extend_from_slice(p);
    }
    lumia_alloc_string(buf.as_ptr(), buf.len() as u64)
}

#[inline]
fn is_list_tid(tid: u32) -> bool {
    tid == TYPE_LIST || tid == TYPE_LIST_F64 || tid == TYPE_LIST_IOTA
}

#[inline]
fn list_tid(list: *mut u8) -> u32 {
    if list.is_null() {
        TYPE_LIST
    } else {
        unsafe { (*header_from_payload(list)).type_id }
    }
}

/// Preserve Float-elem tagging when allocating a derived HeapList.
#[inline]
fn heap_list_tid(list: *mut u8) -> u32 {
    if list_tid(list) == TYPE_LIST_F64 {
        TYPE_LIST_F64
    } else {
        TYPE_LIST
    }
}

fn list_float_elems(list: *mut u8) -> bool {
    list_tid(list) == TYPE_LIST_F64
}

/// Ensure a list uses IEEE elem eq/hash (`TYPE_LIST_F64`).
/// Empty ordinary lists become a fresh empty F64 list (no in-place retag).
fn ensure_list_f64(list: *mut u8) -> *mut u8 {
    if list.is_null() {
        let dest = lumia_alloc(8, TYPE_LIST_F64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(list);
        match (*h).type_id {
            TYPE_LIST_F64 => list,
            TYPE_LIST => {
                if *(list as *const i64) != 0 {
                    trap_abort("lumia: ensure_list_f64 on non-empty Int-elem list");
                }
                let dest = lumia_alloc(8, TYPE_LIST_F64);
                *(dest as *mut i64) = 0;
                dest
            }
            TYPE_LIST_IOTA => trap_abort("lumia: ensure_list_f64 on Iota"),
            other => trap_abort(&format!("lumia: ensure_list_f64 on type_id={other}")),
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_ensure_list_f64(list: *mut u8) -> *mut u8 {
    ensure_list_f64(list)
}

/// HeapList: `[len][elem…]`; Iota: `[start][end_exclusive]`.
fn list_len_of(list: *mut u8) -> i64 {
    if list.is_null() {
        return 0;
    }
    unsafe {
        match (*header_from_payload(list)).type_id {
            TYPE_LIST_IOTA => {
                let base = list as *const i64;
                let start = *base;
                let end = *base.add(1);
                if end > start {
                    end.checked_sub(start)
                        .unwrap_or_else(|| trap_abort("lumia: iota length overflow"))
                } else {
                    0
                }
            }
            _ => *(list as *const i64),
        }
    }
}

fn list_get_of(list: *mut u8, index: i64) -> i64 {
    if list.is_null() || index < 0 {
        trap_abort("lumia: list get out of bounds");
    }
    unsafe {
        match (*header_from_payload(list)).type_id {
            TYPE_LIST_IOTA => {
                let base = list as *const i64;
                let start = *base;
                let end = *base.add(1);
                let len = if end > start { end - start } else { 0 };
                if index >= len {
                    trap_abort("lumia: list get out of bounds");
                }
                start
                    .checked_add(index)
                    .unwrap_or_else(|| trap_abort("lumia: iota index overflow"))
            }
            _ => {
                let len = *(list as *const i64);
                if index >= len {
                    trap_abort("lumia: list get out of bounds");
                }
                let base = list as *const i64;
                *base.add(1 + index as usize)
            }
        }
    }
}

/// Materialize Iota → HeapList (identity for HeapList / null).
fn force_heap_list(list: *mut u8) -> *mut u8 {
    if list.is_null() {
        return list;
    }
    if list_tid(list) != TYPE_LIST_IOTA {
        return list;
    }
    let _guard = GcInhibitGuard::enter();
    let n = list_len_of(list);
    if n < 0 {
        trap_abort("lumia: iota length overflow");
    }
    let dest = lumia_alloc(list_payload_bytes(n), TYPE_LIST);
    unsafe {
        let dst = dest as *mut i64;
        *dst = n;
        let base = list as *const i64;
        let start = *base;
        for i in 0..n as usize {
            let v = start
                .checked_add(i as i64)
                .unwrap_or_else(|| trap_abort("lumia: iota element overflow"));
            *dst.add(1 + i) = v;
        }
    }
    dest
}

/// List payload layout: HeapList `[len:i64][elem0:i64]…`; Iota `[start][end)`.
#[no_mangle]
pub extern "C" fn lumia_list_len(list: *mut u8) -> i64 {
    list_len_of(list)
}

#[no_mangle]
pub extern "C" fn lumia_list_get(list: *mut u8, index: i64) -> i64 {
    list_get_of(list, index)
}

/// Return a new HeapList with `elem` appended.
#[no_mangle]
pub extern "C" fn lumia_list_append(list: *mut u8, elem: i64) -> *mut u8 {
    // Keep materialized Iota alive across the following alloc/copy.
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    unsafe {
        let n = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        let n1 = n
            .checked_add(1)
            .unwrap_or_else(|| trap_abort("lumia: list append length overflow"));
        let nbytes = list_payload_bytes(n1);
        let dest = lumia_alloc(nbytes, heap_list_tid(list));
        if dest.is_null() {
            trap_abort("lumia: list append OOM");
        }
        let dst = dest as *mut i64;
        *dst = n + 1;
        if !list.is_null() {
            let src = list as *const i64;
            for i in 0..n as usize {
                *dst.add(1 + i) = *src.add(1 + i);
            }
        }
        *dst.add(1 + n as usize) = elem;
        dest
    }
}

/// Parallel map over List[scalar] with a C ABI `fn(i64) -> i64`.
/// Type checker requires concrete Int/Bool/Float elems; workers must not heap-allocate.
/// Falls back to sequential for small lists; inhibits GC while workers run.
#[no_mangle]
pub extern "C" fn lumia_list_par_map(
    list: *mut u8,
    f: Option<extern "C" fn(i64) -> i64>,
) -> *mut u8 {
    let Some(f) = f else {
        trap_abort("lumia: list_par_map null function");
    };
    // Cover force + sequential alloc; parallel path takes its own inhibit.
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    unsafe {
        let n = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        if n <= 0 {
            return lumia_list_empty();
        }
        let src = list as *const i64;
        // Sequential for tiny lists.
        if n < 64 {
            let dest = lumia_alloc(list_payload_bytes(n), TYPE_LIST);
            let dst = dest as *mut i64;
            *dst = n;
            for i in 0..n as usize {
                *dst.add(1 + i) = f(*src.add(1 + i));
            }
            return dest;
        }
        let _gc = GcInhibitGuard::enter();
        let workers = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4)
            .min(n as usize)
            .max(1);
        let chunk = (n as usize + workers - 1) / workers;
        let mut handles = Vec::new();
        for w in 0..workers {
            let start = w * chunk;
            let end = ((w + 1) * chunk).min(n as usize);
            // SAFETY: list is immutable during map; GC inhibited on main;
            // workers must not allocate (PAR_WORKER).
            let base = src as usize;
            handles.push(std::thread::spawn(move || {
                PAR_WORKER.with(|c| c.set(true));
                let src = base as *const i64;
                let mut out = Vec::with_capacity(end.saturating_sub(start));
                for i in start..end {
                    out.push(f(*src.add(1 + i)));
                }
                out
            }));
        }
        let parts: Vec<Vec<i64>> = handles
            .into_iter()
            .map(|h| h.join().expect("par_map worker"))
            .collect();
        let dest = lumia_alloc(list_payload_bytes(n), TYPE_LIST);
        let dst = dest as *mut i64;
        *dst = n;
        let mut i = 0usize;
        for part in parts {
            for v in part {
                *dst.add(1 + i) = v;
                i += 1;
            }
        }
        dest
    }
}

/// Parallel left-fold over List[scalar] with C ABI `fn(acc, x) -> acc`.
/// Assumes `f` is associative so chunk results can be combined: `f(z, combine(chunks))`.
/// Falls back to sequential for small lists; inhibits GC while workers run.
#[no_mangle]
pub extern "C" fn lumia_list_par_fold(
    list: *mut u8,
    init: i64,
    f: Option<extern "C" fn(i64, i64) -> i64>,
) -> i64 {
    let Some(f) = f else {
        trap_abort("lumia: list_par_fold null function");
    };
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    unsafe {
        let n = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        if n <= 0 {
            return init;
        }
        let src = list as *const i64;
        if n < 64 {
            let mut acc = init;
            for i in 0..n as usize {
                acc = f(acc, *src.add(1 + i));
            }
            return acc;
        }
        let _gc = GcInhibitGuard::enter();
        let workers = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4)
            .min(n as usize)
            .max(1);
        let chunk = (n as usize + workers - 1) / workers;
        let mut handles = Vec::new();
        for w in 0..workers {
            let start = w * chunk;
            let end = ((w + 1) * chunk).min(n as usize);
            if start >= end {
                continue;
            }
            let base = src as usize;
            handles.push(std::thread::spawn(move || {
                PAR_WORKER.with(|c| c.set(true));
                let src = base as *const i64;
                let mut acc = *src.add(1 + start);
                for i in (start + 1)..end {
                    acc = f(acc, *src.add(1 + i));
                }
                acc
            }));
        }
        let mut acc = init;
        for h in handles {
            let part = h.join().expect("par_fold worker");
            acc = f(acc, part);
        }
        acc
    }
}

/// Immutable update: new List with index `i` set to `elem` (bounds trap).
#[no_mangle]
pub extern "C" fn lumia_list_set(list: *mut u8, index: i64, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let list = force_heap_list(list);
    unsafe {
        if list.is_null() || index < 0 {
            trap_abort("lumia: list set out of bounds");
        }
        let n = *(list as *const i64);
        if index >= n {
            trap_abort("lumia: list set out of bounds");
        }
        let nbytes = list_payload_bytes(n);
        let dest = lumia_alloc(nbytes, heap_list_tid(list));
        if dest.is_null() {
            trap_abort("lumia: list set OOM");
        }
        let src = list as *const i64;
        let dst = dest as *mut i64;
        *dst = n;
        for j in 0..n as usize {
            *dst.add(1 + j) = *src.add(1 + j);
        }
        *dst.add(1 + index as usize) = elem;
        dest
    }
}

/// Return a new HeapList that is `a` followed by `b`.
#[no_mangle]
pub extern "C" fn lumia_list_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let a = force_heap_list(a);
    let b = force_heap_list(b);
    unsafe {
        let na = if a.is_null() {
            0i64
        } else {
            *(a as *const i64)
        };
        let nb = if b.is_null() {
            0i64
        } else {
            *(b as *const i64)
        };
        // Immutable lists: concat with empty is identity (share the other).
        if na == 0 {
            return if nb == 0 { lumia_list_empty() } else { b };
        }
        if nb == 0 {
            return a;
        }
        let n = na
            .checked_add(nb)
            .unwrap_or_else(|| trap_abort("lumia: list concat length overflow"));
        let nbytes = list_payload_bytes(n);
        let tid = if list_float_elems(a) || list_float_elems(b) {
            TYPE_LIST_F64
        } else {
            TYPE_LIST
        };
        let dest = lumia_alloc(nbytes, tid);
        if dest.is_null() {
            trap_abort("lumia: list concat OOM");
        }
        let dst = dest as *mut i64;
        *dst = n;
        let src = a as *const i64;
        for i in 0..na as usize {
            *dst.add(1 + i) = *src.add(1 + i);
        }
        let src = b as *const i64;
        for i in 0..nb as usize {
            *dst.add(1 + na as usize + i) = *src.add(1 + i);
        }
        dest
    }
}

/// Shared empty `List` (`LitList` / `listOf()`). Immortal — survives GC.
#[no_mangle]
pub extern "C" fn lumia_list_empty() -> *mut u8 {
    thread_local! {
        static EMPTY: Cell<*mut u8> = const { Cell::new(ptr::null_mut()) };
    }
    EMPTY.with(|c| {
        let cur = c.get();
        if !cur.is_null() {
            return cur;
        }
        let dest = lumia_alloc(8, TYPE_LIST);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        PERM_OBJECTS.with(|p| p.borrow_mut().push(dest));
        c.set(dest);
        dest
    })
}

/// Return a new list with elements from `start` to end (Iota stays virtual).
#[no_mangle]
pub extern "C" fn lumia_list_slice(list: *mut u8, start: i64) -> *mut u8 {
    if list.is_null() {
        return lumia_list_empty();
    }
    if list_tid(list) == TYPE_LIST_IOTA {
        unsafe {
            let base = list as *const i64;
            let s0 = *base;
            let end = *base.add(1);
            let start = if start < 0 { 0 } else { start };
            let abs = s0
                .checked_add(start)
                .unwrap_or_else(|| trap_abort("lumia: iota slice overflow"));
            if abs >= end {
                return lumia_range(s0, s0);
            }
            return lumia_range(abs, end);
        }
    }
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let len = *(list as *const i64);
        let start = if start < 0 { 0 } else { start };
        let n = if start >= len { 0i64 } else { len - start };
        let dest = lumia_alloc(list_payload_bytes(n), heap_list_tid(list));
        if dest.is_null() {
            trap_abort("lumia: slice OOM");
        }
        *(dest as *mut i64) = n;
        let src = list as *const i64;
        let dst = dest as *mut i64;
        for i in 0..n as usize {
            *dst.add(1 + i) = *src.add(1 + start as usize + i);
        }
        dest
    }
}

/// Build `[start, end)` as Iota (`TYPE_LIST_IOTA`) — O(1), no element materialization.
#[no_mangle]
pub extern "C" fn lumia_range(start: i64, end: i64) -> *mut u8 {
    let end = if end > start { end } else { start };
    let dest = lumia_alloc(16, TYPE_LIST_IOTA);
    unsafe {
        let base = dest as *mut i64;
        *base = start;
        *base.add(1) = end;
    }
    dest
}

/// Build `[start, end]` inclusive.
#[no_mangle]
pub extern "C" fn lumia_range_inclusive(start: i64, end: i64) -> *mut u8 {
    if end < start {
        return lumia_range(start, start);
    }
    match end.checked_add(1) {
        Some(excl) => lumia_range(start, excl),
        None => trap_abort("lumia: rangeInclusive overflow"),
    }
}

#[no_mangle]
pub extern "C" fn lumia_trap_div0() {
    trap_abort("lumia: division by zero");
}

#[no_mangle]
pub extern "C" fn lumia_trap_overflow() {
    trap_abort("lumia: integer overflow");
}

/// Map: small maps stay linear `[n][k0][v0]…`; larger use HashOrdered
/// `[n][cap][order×cap][key,val,state × cap]` (DESIGN default path).
/// Hash writes may produce Overlay: `[-1][parent][dn][k0][v0]…` (delta ≤ 8).
const MAP_SMALL_MAX: i64 = 8;
const MAP_OVERLAY_MARK: i64 = -1;
const MAP_OVERLAY_MAX: i64 = 8;
const MAP_ST_EMPTY: i64 = 0;
const MAP_ST_FULL: i64 = 1;
const MAP_ST_TOMB: i64 = 2;

fn map_linear_nbytes(n: i64) -> usize {
    if n < 0 {
        trap_abort("lumia: negative map length");
    }
    (n as u64)
        .checked_mul(2)
        .and_then(|pairs| pairs.checked_add(1))
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as u64)
        .map(|b| b as usize)
        .unwrap_or_else(|| trap_abort(&format!("lumia: map too large (n={n})")))
}

fn map_hash_nbytes(cap: usize) -> usize {
    // [count][cap] + order[cap] + (key,val,state)[cap]
    cap.checked_mul(4)
        .and_then(|w| w.checked_add(2))
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as usize)
        .unwrap_or_else(|| trap_abort(&format!("lumia: map hash table too large (cap={cap})")))
}

fn map_overlay_nbytes(dn: i64) -> usize {
    if dn < 0 {
        trap_abort("lumia: negative overlay delta");
    }
    (dn as u64)
        .checked_mul(2)
        .and_then(|kv| kv.checked_add(3))
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as u64)
        .map(|b| b as usize)
        .unwrap_or_else(|| trap_abort(&format!("lumia: map overlay too large (dn={dn})")))
}

fn map_is_overlay(map: *mut u8) -> bool {
    if map.is_null() {
        return false;
    }
    unsafe { *(map as *const i64) == MAP_OVERLAY_MARK }
}

fn map_is_hash(map: *mut u8) -> bool {
    if map.is_null() || map_is_overlay(map) {
        return false;
    }
    unsafe {
        let n = *(map as *const i64);
        if n < 0 {
            return false;
        }
        (*header_from_payload(map)).size as usize != map_linear_nbytes(n)
    }
}

unsafe fn map_overlay_parent(map: *mut u8) -> *mut u8 {
    *(map as *const i64).add(1) as *mut u8
}

unsafe fn map_overlay_dn(map: *mut u8) -> i64 {
    *(map as *const i64).add(2)
}

/// Logical entry count (insertion-unique keys).
fn map_count(map: *mut u8) -> i64 {
    if map.is_null() {
        return 0;
    }
    unsafe {
        if map_is_overlay(map) {
            let parent = map_overlay_parent(map);
            let dn = map_overlay_dn(map) as usize;
            let base = map as *const i64;
            let mut n = map_count(parent);
            for i in 0..dn {
                let k = *base.add(3 + i * 2);
                // Count as new if not in parent and not earlier in delta.
                let mut seen = false;
                for j in 0..i {
                    if lumia_eq(*base.add(3 + j * 2), k) != 0 {
                        seen = true;
                        break;
                    }
                }
                if seen {
                    continue;
                }
                if map_find(parent, k).is_none() {
                    n += 1;
                }
            }
            n
        } else {
            *(map as *const i64)
        }
    }
}

/// Lookup value through overlay chain then base map.
unsafe fn map_lookup_val(map: *mut u8, key: i64) -> Option<i64> {
    if map.is_null() {
        return None;
    }
    if map_is_overlay(map) {
        let dn = map_overlay_dn(map) as usize;
        let base = map as *const i64;
        for i in (0..dn).rev() {
            if lumia_eq(*base.add(3 + i * 2), key) != 0 {
                return Some(*base.add(4 + i * 2));
            }
        }
        return map_lookup_val(map_overlay_parent(map), key);
    }
    match map_find(map, key) {
        Some(i) if map_is_hash(map) => {
            let base = map as *const i64;
            let cap = *base.add(1) as usize;
            let cell = base.add(2 + cap + i * 3);
            Some(*cell.add(1))
        }
        Some(i) => {
            let base = map as *const i64;
            Some(*base.add(2 + i * 2))
        }
        None => None,
    }
}

/// Flatten overlay (and nested overlays) into a HashOrdered or linear map.
unsafe fn map_materialize(map: *mut u8) -> *mut u8 {
    // Multi-alloc helper: keep intermediates alive across soft-threshold GC.
    let _gc = GcInhibitGuard::enter();
    if map.is_null() || !map_is_overlay(map) {
        return map;
    }
    let parent = map_materialize(map_overlay_parent(map));
    let dn = map_overlay_dn(map) as usize;
    let base = map as *const i64;
    let mut dest = if map_is_hash(parent) || map_count(parent) + dn as i64 > MAP_SMALL_MAX {
        // Start from hash clone of parent
        if map_is_hash(parent) {
            let pbase = parent as *const i64;
            let n = *pbase;
            let cap = *pbase.add(1) as usize;
            let out = map_alloc_hash_tid(cap, 0, map_type_id(parent));
            let mut w = 0usize;
            for i in 0..n as usize {
                let s = *pbase.add(2 + i) as usize;
                let cell = pbase.add(2 + cap + s * 3);
                map_hash_put_new(out, *cell, *cell.add(1), w);
                w += 1;
            }
            *(out as *mut i64) = n;
            out
        } else {
            map_from_linear_to_hash(parent, None)
        }
    } else {
        // Stay linear: copy parent then apply deltas via set path below
        let n = map_count(parent);
        let nbytes = map_linear_nbytes(n) as u64;
        let out = lumia_alloc(nbytes, map_type_id(parent));
        ptr::copy_nonoverlapping(parent, out, nbytes as usize);
        out
    };
    for i in 0..dn {
        let k = *base.add(3 + i * 2);
        let v = *base.add(4 + i * 2);
        dest = map_clone_hash_upsert_or_linear(dest, k, v);
    }
    dest
}

unsafe fn map_clone_hash_upsert_or_linear(map: *mut u8, key: i64, val: i64) -> *mut u8 {
    if map_is_hash(map) {
        map_clone_hash_upsert(map, key, val)
    } else {
        // linear upsert (same as lumia_map_set linear branch)
        let (n, base) = if map.is_null() {
            (0i64, ptr::null())
        } else {
            (*(map as *const i64), map as *const i64)
        };
        if let Some(i) = map_find(map, key) {
            let nbytes = map_linear_nbytes(n) as u64;
            let dest = lumia_alloc(nbytes, map_type_id(map));
            let dst = dest as *mut i64;
            *dst = n;
            for j in 0..(n as usize * 2) {
                *dst.add(1 + j) = *base.add(1 + j);
            }
            *dst.add(2 + i * 2) = val;
            return dest;
        }
        let n2 = n + 1;
        if n2 > MAP_SMALL_MAX && !map_is_assoc(map) {
            return map_from_linear_to_hash(map, Some((key, val)));
        }
        let nbytes = map_linear_nbytes(n2) as u64;
        let dest = lumia_alloc(nbytes, map_type_id(map));
        let dst = dest as *mut i64;
        *dst = n2;
        for j in 0..(n as usize * 2) {
            *dst.add(1 + j) = *base.add(1 + j);
        }
        *dst.add(1 + n as usize * 2) = key;
        *dst.add(2 + n as usize * 2) = val;
        dest
    }
}

unsafe fn map_alloc_overlay(parent: *mut u8, pairs: &[(i64, i64)]) -> *mut u8 {
    let dn = pairs.len() as i64;
    let nbytes = map_overlay_nbytes(dn) as u64;
    let dest = lumia_alloc(nbytes, map_type_id(parent));
    let dst = dest as *mut i64;
    *dst = MAP_OVERLAY_MARK;
    *dst.add(1) = parent as i64;
    *dst.add(2) = dn;
    for (i, (k, v)) in pairs.iter().enumerate() {
        *dst.add(3 + i * 2) = *k;
        *dst.add(4 + i * 2) = *v;
    }
    dest
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Stable content hash for Map/Set keys — must agree with `lumia_eq` (DESIGN §3.5.1).
pub fn lumia_hash(key: i64) -> u64 {
    hash_value(key, 0)
}

fn hash_value(key: i64, depth: u32) -> u64 {
    if depth > 64 {
        return splitmix64(key as u64);
    }
    let p = key as *mut u8;
    if !is_heap_payload(p) {
        return splitmix64(key as u64);
    }
    unsafe {
        let h = header_from_payload(p);
        match (*h).type_id {
            TYPE_STRING => {
                let n = (*h).size as usize;
                let bytes = std::slice::from_raw_parts(p, n);
                let mut acc = 0xcbf29ce484222325u64;
                for &b in bytes {
                    acc ^= b as u64;
                    acc = acc.wrapping_mul(0x100000001b3);
                }
                acc
            }
            TYPE_CHAR => splitmix64(*(p as *const i64) as u64),
            TYPE_LIST | TYPE_LIST_F64 | TYPE_LIST_IOTA => {
                let n = list_len_of(p);
                let float_elems = (*h).type_id == TYPE_LIST_F64;
                let mut acc = splitmix64(0x4c495354u64 ^ (n as u64));
                for i in 0..n {
                    let e = list_get_of(p, i);
                    let he = if float_elems {
                        float_key_hash(e)
                    } else {
                        hash_value(e, depth + 1)
                    };
                    acc = acc.rotate_left(7).wrapping_add(he);
                }
                acc
            }
            TYPE_ADT => {
                let words = ((*h).size as usize) / 8;
                let base = p as *const i64;
                let mut acc = splitmix64(0x414454u64 ^ (words as u64));
                for i in 0..words {
                    acc = acc
                        .rotate_left(11)
                        .wrapping_add(hash_value(*base.add(i), depth + 1));
                }
                acc
            }
            tid if is_map_tid(tid) => {
                // Unordered mix so content-equal maps collide regardless of insert order.
                let float_keys = map_float_keys(p);
                let float_vals = map_float_vals(p);
                let n = map_count(p);
                let mut acc = splitmix64(0x4d4150u64 ^ (n as u64));
                for i in 0..n as usize {
                    let (k, v) = map_pair_at(p, i);
                    let hk = if float_keys {
                        float_key_hash(k)
                    } else {
                        hash_value(k, depth + 1)
                    };
                    let hv = if float_vals {
                        float_key_hash(v)
                    } else {
                        hash_value(v, depth + 1)
                    };
                    acc ^= hk.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(hv);
                }
                acc
            }
            TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => {
                let float_elems = (*h).type_id == TYPE_SET_F64;
                let n = *(p as *const i64);
                let mut acc = splitmix64(0x534554u64 ^ (n as u64));
                for i in 0..n as usize {
                    let e = set_elem_at(p, i);
                    acc ^= if float_elems {
                        float_key_hash(e)
                    } else {
                        hash_value(e, depth + 1)
                    };
                }
                acc
            }
            TYPE_CLOSURE | TYPE_BYTES => splitmix64(key as u64),
            _ => splitmix64(key as u64),
        }
    }
}

fn map_mark_payload(payload: *mut u8, size: usize) {
    unsafe {
        let base = payload as *const i64;
        let n0 = *base;
        if n0 == MAP_OVERLAY_MARK {
            let parent = map_overlay_parent(payload);
            if is_heap_payload(parent) {
                mark(header_from_payload(parent));
            }
            let dn = map_overlay_dn(payload) as usize;
            for i in 0..dn {
                mark_value(*base.add(3 + i * 2));
                mark_value(*base.add(4 + i * 2));
            }
            return;
        }
        let n = n0;
        if size == map_linear_nbytes(n) {
            for i in 0..n as usize {
                mark_value(*base.add(1 + i * 2));
                mark_value(*base.add(2 + i * 2));
            }
            return;
        }
        // HashOrdered
        let cap = *base.add(1) as usize;
        let order = base.add(2);
        for i in 0..n as usize {
            let slot = *order.add(i) as usize;
            let cell = base.add(2 + cap + slot * 3);
            mark_value(*cell);
            mark_value(*cell.add(1));
        }
    }
}

fn map_eq(a: *mut u8, b: *mut u8) -> i64 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let a = if map_is_overlay(a) {
            map_materialize(a)
        } else {
            a
        };
        let b = if map_is_overlay(b) {
            map_materialize(b)
        } else {
            b
        };
        let na = if a.is_null() { 0 } else { *(a as *const i64) };
        let nb = if b.is_null() { 0 } else { *(b as *const i64) };
        if na != nb {
            return 0;
        }
        let float_keys = map_float_keys(a) || map_float_keys(b);
        let float_vals = map_float_vals(a) || map_float_vals(b);
        for i in 0..na as usize {
            let (ka, va) = map_pair_at(a, i);
            let mut found = false;
            for j in 0..nb as usize {
                let (kb, vb) = map_pair_at(b, j);
                let vals_ok = if float_vals {
                    float_key_eq(va, vb)
                } else {
                    lumia_eq(va, vb) != 0
                };
                if key_eq(ka, kb, float_keys) && vals_ok {
                    found = true;
                    break;
                }
            }
            if !found {
                return 0;
            }
        }
        1
    }
}

/// i-th pair in insertion order.
unsafe fn map_pair_at(map: *mut u8, i: usize) -> (i64, i64) {
    let _gc = GcInhibitGuard::enter();
    let map = if map_is_overlay(map) {
        map_materialize(map)
    } else {
        map
    };
    let base = map as *const i64;
    if map_is_hash(map) {
        let cap = *base.add(1) as usize;
        let slot = *base.add(2 + i) as usize;
        let cell = base.add(2 + cap + slot * 3);
        (*cell, *cell.add(1))
    } else {
        (*base.add(1 + i * 2), *base.add(2 + i * 2))
    }
}

unsafe fn map_hash_find_slot(map: *mut u8, key: i64) -> Option<usize> {
    let float_keys = map_float_keys(map);
    let base = map as *const i64;
    let cap = *base.add(1) as usize;
    if cap == 0 {
        return None;
    }
    let mut idx = (key_hash(key, float_keys) as usize) % cap;
    for _ in 0..cap {
        let cell = base.add(2 + cap + idx * 3);
        let st = *cell.add(2);
        if st == MAP_ST_EMPTY {
            return None;
        }
        if st == MAP_ST_FULL && key_eq(*cell, key, float_keys) {
            return Some(idx);
        }
        idx = (idx + 1) % cap;
    }
    None
}

/// Map payload helpers — linear or HashOrdered (see above).
unsafe fn map_find(map: *mut u8, key: i64) -> Option<usize> {
    if map.is_null() || map_is_overlay(map) {
        return None;
    }
    if map_is_hash(map) {
        return map_hash_find_slot(map, key);
    }
    let float_keys = map_float_keys(map);
    let n = *(map as *const i64);
    let base = map as *const i64;
    let mut found = None;
    for i in 0..n as usize {
        if key_eq(*base.add(1 + i * 2), key, float_keys) {
            found = Some(i);
        }
    }
    found
}

#[no_mangle]
pub extern "C" fn lumia_map_contains(map: *mut u8, key: i64) -> i64 {
    unsafe {
        if map_lookup_val(map, key).is_some() {
            1
        } else {
            0
        }
    }
}

/// Missing key → None ADT; hit → Some(value). Tags come from the program's `Option` decl.
#[no_mangle]
pub extern "C" fn lumia_map_get(
    map: *mut u8,
    key: i64,
    some_tag: i64,
    none_tag: i64,
) -> *mut u8 {
    unsafe {
        match map_lookup_val(map, key) {
            Some(val) => alloc_adt(some_tag, &[val]),
            None => alloc_adt(none_tag, &[]),
        }
    }
}

fn alloc_adt(tag: i64, fields: &[i64]) -> *mut u8 {
    let nbytes = list_payload_bytes(fields.len() as i64);
    let dest = lumia_alloc(nbytes, TYPE_ADT);
    if dest.is_null() {
        trap_abort("lumia: adt OOM");
    }
    unsafe {
        let dst = dest as *mut i64;
        *dst = tag;
        for (i, f) in fields.iter().enumerate() {
            *dst.add(1 + i) = *f;
        }
    }
    dest
}

unsafe fn map_alloc_hash_tid(cap: usize, count: i64, tid: u32) -> *mut u8 {
    let nbytes = map_hash_nbytes(cap) as u64;
    let dest = lumia_alloc(nbytes, tid);
    if dest.is_null() {
        trap_abort("lumia: map hash OOM");
    }
    let dst = dest as *mut i64;
    *dst = count;
    *dst.add(1) = cap as i64;
    for i in 0..cap {
        *dst.add(2 + i) = -1;
        let cell = dst.add(2 + cap + i * 3);
        *cell = 0;
        *cell.add(1) = 0;
        *cell.add(2) = MAP_ST_EMPTY;
    }
    dest
}

unsafe fn map_hash_put_new(dest: *mut u8, key: i64, val: i64, order_i: usize) {
    let float_keys = map_float_keys(dest);
    let base = dest as *mut i64;
    let cap = *base.add(1) as usize;
    let mut idx = (key_hash(key, float_keys) as usize) % cap;
    for _ in 0..cap {
        let cell = base.add(2 + cap + idx * 3);
        let st = *cell.add(2);
        if st == MAP_ST_EMPTY || st == MAP_ST_TOMB {
            *cell = key;
            *cell.add(1) = val;
            *cell.add(2) = MAP_ST_FULL;
            *base.add(2 + order_i) = idx as i64;
            return;
        }
        idx = (idx + 1) % cap;
    }
    trap_abort("lumia: map hash full");
}

/// Insert or replace during hash-table build. Returns true if a new key was added.
unsafe fn map_hash_upsert_build(dest: *mut u8, key: i64, val: i64) -> bool {
    if let Some(slot) = map_hash_find_slot(dest, key) {
        let base = dest as *mut i64;
        let cap = *base.add(1) as usize;
        let cell = base.add(2 + cap + slot * 3);
        *cell.add(1) = val; // last wins
        return false;
    }
    let base = dest as *mut i64;
    let n = *base as usize;
    map_hash_put_new(dest, key, val, n);
    *base = (n as i64) + 1;
    true
}

unsafe fn map_from_linear_to_hash(src: *mut u8, extra_key: Option<(i64, i64)>) -> *mut u8 {
    let n = if src.is_null() {
        0i64
    } else {
        *(src as *const i64)
    };
    let n2 = n + if extra_key.is_some() { 1 } else { 0 };
    let mut cap = 16usize;
    while (cap as i64) < n2 * 2 {
        cap *= 2;
    }
    let dest = map_alloc_hash_tid(cap, 0, map_type_id(src)); // count filled by upserts
    let base = src as *const i64;
    for i in 0..n as usize {
        let k = *base.add(1 + i * 2);
        let v = *base.add(2 + i * 2);
        map_hash_upsert_build(dest, k, v);
    }
    if let Some((k, v)) = extra_key {
        map_hash_upsert_build(dest, k, v);
    }
    dest
}

unsafe fn map_clone_hash_upsert(src: *mut u8, key: i64, val: i64) -> *mut u8 {
    let base = src as *const i64;
    let n = *base;
    let cap = *base.add(1) as usize;
    let replace = map_hash_find_slot(src, key);
    let n2 = if replace.is_some() { n } else { n + 1 };
    let need_grow = replace.is_none() && (n2 as usize * 2 > cap);
    let new_cap = if need_grow { cap * 2 } else { cap };
    let dest = map_alloc_hash_tid(new_cap, n2, map_type_id(src));
    let mut w = 0usize;
    for i in 0..n as usize {
        let slot = *base.add(2 + i) as usize;
        let cell = base.add(2 + cap + slot * 3);
        let k = *cell;
        let v = if replace == Some(slot) {
            val
        } else {
            *cell.add(1)
        };
        map_hash_put_new(dest, k, v, w);
        w += 1;
    }
    if replace.is_none() {
        map_hash_put_new(dest, key, val, w);
    }
    dest
}

/// Immutable upsert: new Map with `key → val` (overwrite keeps insertion slot).
#[no_mangle]
pub extern "C" fn lumia_map_set(map: *mut u8, key: i64, val: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        if map_is_overlay(map) {
            let parent = map_overlay_parent(map);
            let dn = map_overlay_dn(map);
            let base = map as *const i64;
            // Replace existing delta key in-place in a new overlay copy.
            let float_keys = map_float_keys(parent) || map_float_keys(map);
            for i in (0..dn as usize).rev() {
                if key_eq(*base.add(3 + i * 2), key, float_keys) {
                    let mut pairs = Vec::with_capacity(dn as usize);
                    for j in 0..dn as usize {
                        let k = *base.add(3 + j * 2);
                        let v = if j == i {
                            val
                        } else {
                            *base.add(4 + j * 2)
                        };
                        pairs.push((k, v));
                    }
                    return map_alloc_overlay(parent, &pairs);
                }
            }
            if dn < MAP_OVERLAY_MAX {
                let mut pairs = Vec::with_capacity(dn as usize + 1);
                for j in 0..dn as usize {
                    pairs.push((*base.add(3 + j * 2), *base.add(4 + j * 2)));
                }
                pairs.push((key, val));
                return map_alloc_overlay(parent, &pairs);
            }
            // Delta full → flatten then upsert.
            let flat = map_materialize(map);
            return lumia_map_set(flat, key, val);
        }
        if map.is_null() || !map_is_hash(map) {
            let (n, base) = if map.is_null() {
                (0i64, ptr::null())
            } else {
                (*(map as *const i64), map as *const i64)
            };
            if let Some(i) = map_find(map, key) {
                let nbytes = map_linear_nbytes(n) as u64;
                let dest = lumia_alloc(nbytes, map_type_id(map));
                let dst = dest as *mut i64;
                *dst = n;
                for j in 0..(n as usize * 2) {
                    *dst.add(1 + j) = *base.add(1 + j);
                }
                *dst.add(2 + i * 2) = val;
                return dest;
            }
            let n2 = n + 1;
            if n2 > MAP_SMALL_MAX && !map_is_assoc(map) {
                return map_from_linear_to_hash(map, Some((key, val)));
            }
            let nbytes = map_linear_nbytes(n2) as u64;
            let dest = lumia_alloc(nbytes, map_type_id(map));
            let dst = dest as *mut i64;
            *dst = n2;
            for j in 0..(n as usize * 2) {
                *dst.add(1 + j) = *base.add(1 + j);
            }
            *dst.add(1 + n as usize * 2) = key;
            *dst.add(2 + n as usize * 2) = val;
            return dest;
        }
        // HashOrdered → Overlay (avoid full table clone on each set).
        map_alloc_overlay(map, &[(key, val)])
    }
}

/// Dispatch `set` for List (index update) or Map (key upsert).
#[no_mangle]
pub extern "C" fn lumia_set(obj: *mut u8, key_or_index: i64, val: i64) -> *mut u8 {
    if obj.is_null() {
        return lumia_map_set(obj, key_or_index, val);
    }
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        TYPE_LIST | TYPE_LIST_F64 | TYPE_LIST_IOTA => lumia_list_set(obj, key_or_index, val),
        tid if is_map_tid(tid) => lumia_map_set(obj, key_or_index, val),
        _ => trap_abort(&format!("lumia: set on unsupported type_id={tid}")),
    }
}

/// Drop key if present; returns new Map (insertion order of remaining keys).
#[no_mangle]
pub extern "C" fn lumia_map_remove(map: *mut u8, key: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let map = if map_is_overlay(map) {
            map_materialize(map)
        } else {
            map
        };
        let tid = map_type_id(map);
        if map.is_null() {
            let dest = lumia_alloc(8, TYPE_MAP);
            *(dest as *mut i64) = 0;
            return dest;
        }
        if map_is_hash(map) {
            let base = map as *const i64;
            let n = *base;
            let cap = *base.add(1) as usize;
            let Some(slot) = map_hash_find_slot(map, key) else {
                let nbytes = map_hash_nbytes(cap) as u64;
                let dest = lumia_alloc(nbytes, tid);
                ptr::copy_nonoverlapping(map, dest, nbytes as usize);
                return dest;
            };
            let n2 = n - 1;
            if n2 <= MAP_SMALL_MAX {
                // Demote to linear
                let nbytes = map_linear_nbytes(n2) as u64;
                let dest = lumia_alloc(nbytes, tid);
                let dst = dest as *mut i64;
                *dst = n2;
                let mut w = 0usize;
                for i in 0..n as usize {
                    let s = *base.add(2 + i) as usize;
                    if s == slot {
                        continue;
                    }
                    let cell = base.add(2 + cap + s * 3);
                    *dst.add(1 + w * 2) = *cell;
                    *dst.add(2 + w * 2) = *cell.add(1);
                    w += 1;
                }
                return dest;
            }
            let dest = map_alloc_hash_tid(cap, n2, tid);
            let mut w = 0usize;
            for i in 0..n as usize {
                let s = *base.add(2 + i) as usize;
                if s == slot {
                    continue;
                }
                let cell = base.add(2 + cap + s * 3);
                map_hash_put_new(dest, *cell, *cell.add(1), w);
                w += 1;
            }
            return dest;
        }

        let n = *(map as *const i64);
        let base = map as *const i64;
        let Some(idx) = map_find(map, key) else {
            let nbytes = map_linear_nbytes(n) as u64;
            let dest = lumia_alloc(nbytes, tid);
            ptr::copy_nonoverlapping(map, dest, nbytes as usize);
            return dest;
        };
        let n2 = n - 1;
        let nbytes = map_linear_nbytes(n2) as u64;
        let dest = lumia_alloc(nbytes, tid);
        let dst = dest as *mut i64;
        *dst = n2;
        let mut w = 0usize;
        for j in 0..n as usize {
            if j == idx {
                continue;
            }
            *dst.add(1 + w * 2) = *base.add(1 + j * 2);
            *dst.add(2 + w * 2) = *base.add(2 + j * 2);
            w += 1;
        }
        dest
    }
}

/// If `map` is a linear table larger than [`MAP_SMALL_MAX`], promote to HashOrdered.
#[no_mangle]
pub extern "C" fn lumia_map_finish(map: *mut u8) -> *mut u8 {
    // Literal build may call finish before the linear table is rooted; inhibit
    // while promoting so alloc inside `map_from_linear_to_hash` cannot collect it.
    let _gc = GcInhibitGuard::enter();
    if map.is_null() {
        return map;
    }
    unsafe {
        if map_is_overlay(map) || map_is_hash(map) || map_is_assoc(map) {
            return map;
        }
        let n = *(map as *const i64);
        if n > MAP_SMALL_MAX {
            map_from_linear_to_hash(map, None)
        } else {
            map
        }
    }
}

/// Keys in insertion order as HeapList.
#[no_mangle]
pub extern "C" fn lumia_map_keys(map: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let map = if map_is_overlay(map) {
            map_materialize(map)
        } else {
            map
        };
        let n = if map.is_null() {
            0i64
        } else {
            *(map as *const i64)
        };
        let nbytes = list_payload_bytes(n);
        let dest = lumia_alloc(nbytes, TYPE_LIST);
        let dst = dest as *mut i64;
        *dst = n;
        if !map.is_null() {
            for i in 0..n as usize {
                let (k, _) = map_pair_at(map, i);
                *dst.add(1 + i) = k;
            }
        }
        dest
    }
}

/// Normalize a collection for indexed `for` / `toList`: List/Iota as heap list,
/// Set as element list, Map as key list (DESIGN: `for (k,v) in m` for pairs).
#[no_mangle]
pub extern "C" fn lumia_elems(obj: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if obj.is_null() {
        let dest = lumia_alloc(8, TYPE_LIST);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        TYPE_LIST | TYPE_LIST_F64 => obj,
        TYPE_LIST_IOTA => force_heap_list(obj),
        TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => unsafe {
            let n = *(obj as *const i64);
            let nbytes = list_payload_bytes(n);
            let dest = lumia_alloc(nbytes, TYPE_LIST);
            let dst = dest as *mut i64;
            *dst = n;
            for i in 0..n as usize {
                *dst.add(1 + i) = set_elem_at(obj, i);
            }
            dest
        },
        tid if is_map_tid(tid) => lumia_map_keys(obj),
        other => trap_abort(&format!("lumia: elems unsupported type_id={other}")),
    }
}

/// Values in insertion order as HeapList.
#[no_mangle]
pub extern "C" fn lumia_map_values(map: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let map = if map_is_overlay(map) {
            map_materialize(map)
        } else {
            map
        };
        let n = if map.is_null() {
            0i64
        } else {
            *(map as *const i64)
        };
        let nbytes = list_payload_bytes(n);
        let dest = lumia_alloc(nbytes, TYPE_LIST);
        let dst = dest as *mut i64;
        *dst = n;
        if !map.is_null() {
            for i in 0..n as usize {
                let (_, v) = map_pair_at(map, i);
                *dst.add(1 + i) = v;
            }
        }
        dest
    }
}

/// Insertion-ordered list of `(k, v)` pairs (each pair is ADT tag0 + 2 fields).
/// Also accepts an existing `List` of pairs (identity) so `for (k,v) in pairs` works.
#[no_mangle]
pub extern "C" fn lumia_map_items(map: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if !map.is_null() {
        let tid = unsafe { (*header_from_payload(map)).type_id };
        if tid == TYPE_LIST || tid == TYPE_LIST_F64 {
            return map;
        }
        if tid == TYPE_LIST_IOTA {
            return force_heap_list(map);
        }
    }
    unsafe {
        let map = if map_is_overlay(map) {
            map_materialize(map)
        } else {
            map
        };
        let n = if map.is_null() {
            0i64
        } else {
            *(map as *const i64)
        };
        let nbytes = list_payload_bytes(n);
        let dest = lumia_alloc(nbytes, TYPE_LIST);
        let dst = dest as *mut i64;
        *dst = n;
        if !map.is_null() {
            for i in 0..n as usize {
                let (k, v) = map_pair_at(map, i);
                let pair = alloc_adt(0, &[k, v]);
                *dst.add(1 + i) = pair as i64;
            }
        }
        dest
    }
}

/// Set: small stays linear `[n][e0]…`; larger HashOrdered
/// `[n][cap][order×cap][elem,state × cap]`.
const SET_SMALL_MAX: i64 = 8;
const SET_ST_EMPTY: i64 = 0;
const SET_ST_FULL: i64 = 1;
const SET_ST_TOMB: i64 = 2;

fn set_linear_nbytes(n: i64) -> usize {
    list_payload_bytes(n) as usize
}

fn set_hash_nbytes(cap: usize) -> usize {
    // [n][cap] + order[cap] + (elem,state)[cap]
    cap.checked_mul(3)
        .and_then(|w| w.checked_add(2))
        .and_then(|words| words.checked_mul(8))
        .filter(|&b| b <= u32::MAX as usize)
        .unwrap_or_else(|| trap_abort(&format!("lumia: set hash table too large (cap={cap})")))
}

fn set_is_hash(set: *mut u8) -> bool {
    if set.is_null() {
        return false;
    }
    unsafe {
        let n = *(set as *const i64);
        if n < 0 {
            return false;
        }
        (*header_from_payload(set)).size as usize != set_linear_nbytes(n)
    }
}

fn set_mark_payload(payload: *mut u8, size: usize) {
    unsafe {
        let base = payload as *const i64;
        let n = *base;
        if size == set_linear_nbytes(n) {
            for i in 0..n as usize {
                mark_value(*base.add(1 + i));
            }
            return;
        }
        let cap = *base.add(1) as usize;
        let order = base.add(2);
        for i in 0..n as usize {
            let slot = *order.add(i) as usize;
            let cell = base.add(2 + cap + slot * 2);
            mark_value(*cell);
        }
    }
}

fn set_eq(a: *mut u8, b: *mut u8) -> i64 {
    unsafe {
        let na = if a.is_null() { 0 } else { *(a as *const i64) };
        let nb = if b.is_null() { 0 } else { *(b as *const i64) };
        if na != nb {
            return 0;
        }
        let float_elems = set_float_elems(a) || set_float_elems(b);
        for i in 0..na as usize {
            let ea = set_elem_at(a, i);
            let mut found = false;
            for j in 0..nb as usize {
                if key_eq(ea, set_elem_at(b, j), float_elems) {
                    found = true;
                    break;
                }
            }
            if !found {
                return 0;
            }
        }
        1
    }
}

unsafe fn set_elem_at(set: *mut u8, i: usize) -> i64 {
    let base = set as *const i64;
    if set_is_hash(set) {
        let cap = *base.add(1) as usize;
        let slot = *base.add(2 + i) as usize;
        *base.add(2 + cap + slot * 2)
    } else {
        *base.add(1 + i)
    }
}

unsafe fn set_hash_find_slot(set: *mut u8, elem: i64) -> Option<usize> {
    let float_elems = set_float_elems(set);
    let base = set as *const i64;
    let cap = *base.add(1) as usize;
    if cap == 0 {
        return None;
    }
    let mut idx = (key_hash(elem, float_elems) as usize) % cap;
    for _ in 0..cap {
        let cell = base.add(2 + cap + idx * 2);
        let st = *cell.add(1);
        if st == SET_ST_EMPTY {
            return None;
        }
        if st == SET_ST_FULL && key_eq(*cell, elem, float_elems) {
            return Some(idx);
        }
        idx = (idx + 1) % cap;
    }
    None
}

/// If `set` is a linear table larger than [`SET_SMALL_MAX`], promote to HashOrdered.
#[no_mangle]
pub extern "C" fn lumia_set_finish(set: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if set.is_null() {
        return set;
    }
    unsafe {
        if set_is_hash(set) || set_is_assoc(set) {
            return set;
        }
        let n = *(set as *const i64);
        if n > SET_SMALL_MAX {
            set_from_linear_to_hash(set, None)
        } else {
            set
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_set_contains(set: *mut u8, elem: i64) -> i64 {
    if set.is_null() {
        return 0;
    }
    unsafe {
        if set_is_hash(set) {
            return if set_hash_find_slot(set, elem).is_some() {
                1
            } else {
                0
            };
        }
        let float_elems = set_float_elems(set);
        let n = *(set as *const i64);
        let base = set as *const i64;
        for i in 0..n as usize {
            if key_eq(*base.add(1 + i), elem, float_elems) {
                return 1;
            }
        }
        0
    }
}

unsafe fn set_alloc_hash_tid(cap: usize, count: i64, tid: u32) -> *mut u8 {
    let dest = lumia_alloc(set_hash_nbytes(cap) as u64, tid);
    let dst = dest as *mut i64;
    *dst = count;
    *dst.add(1) = cap as i64;
    for i in 0..cap {
        *dst.add(2 + i) = -1;
        let cell = dst.add(2 + cap + i * 2);
        *cell = 0;
        *cell.add(1) = SET_ST_EMPTY;
    }
    dest
}

unsafe fn set_hash_put_new(dest: *mut u8, elem: i64, order_i: usize) {
    let float_elems = set_float_elems(dest);
    let base = dest as *mut i64;
    let cap = *base.add(1) as usize;
    let mut idx = (key_hash(elem, float_elems) as usize) % cap;
    for _ in 0..cap {
        let cell = base.add(2 + cap + idx * 2);
        let st = *cell.add(1);
        if st == SET_ST_EMPTY || st == SET_ST_TOMB {
            *cell = elem;
            *cell.add(1) = SET_ST_FULL;
            *base.add(2 + order_i) = idx as i64;
            return;
        }
        idx = (idx + 1) % cap;
    }
    trap_abort("lumia: set hash full");
}

/// Insert during hash build; skip if already present. Returns true if newly added.
unsafe fn set_hash_insert_build(dest: *mut u8, elem: i64) -> bool {
    if set_hash_find_slot(dest, elem).is_some() {
        return false;
    }
    let base = dest as *mut i64;
    let n = *base as usize;
    set_hash_put_new(dest, elem, n);
    *base = (n as i64) + 1;
    true
}

unsafe fn set_from_linear_to_hash(src: *mut u8, extra: Option<i64>) -> *mut u8 {
    let n = if src.is_null() {
        0i64
    } else {
        *(src as *const i64)
    };
    let n2 = n + if extra.is_some() { 1 } else { 0 };
    let mut cap = 16usize;
    while (cap as i64) < n2 * 2 {
        cap *= 2;
    }
    let dest = set_alloc_hash_tid(cap, 0, set_type_id(src));
    let base = src as *const i64;
    for i in 0..n as usize {
        set_hash_insert_build(dest, *base.add(1 + i));
    }
    if let Some(e) = extra {
        set_hash_insert_build(dest, e);
    }
    dest
}

/// Immutable insert: new Set with `elem` (no-op copy if already present).
#[no_mangle]
pub extern "C" fn lumia_set_insert(set: *mut u8, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let tid = set_type_id(set);
        if lumia_set_contains(set, elem) != 0 {
            if set.is_null() {
                let dest = lumia_alloc(8, tid);
                *(dest as *mut i64) = 0;
                return dest;
            }
            let nbytes = (*header_from_payload(set)).size as u64;
            let dest = lumia_alloc(nbytes, tid);
            ptr::copy_nonoverlapping(set, dest, nbytes as usize);
            return dest;
        }
        if set.is_null() || !set_is_hash(set) {
            let n = if set.is_null() {
                0i64
            } else {
                *(set as *const i64)
            };
            let n2 = n + 1;
            if n2 > SET_SMALL_MAX && !set_is_assoc(set) {
                return set_from_linear_to_hash(set, Some(elem));
            }
            let nbytes = set_linear_nbytes(n2) as u64;
            let dest = lumia_alloc(nbytes, tid);
            let dst = dest as *mut i64;
            *dst = n2;
            if !set.is_null() {
                let src = set as *const i64;
                for i in 0..n as usize {
                    *dst.add(1 + i) = *src.add(1 + i);
                }
            }
            *dst.add(1 + n as usize) = elem;
            return dest;
        }
        // Hash insert
        let base = set as *const i64;
        let n = *base;
        let cap = *base.add(1) as usize;
        let n2 = n + 1;
        let need_grow = (n2 as usize * 2) > cap;
        let new_cap = if need_grow { cap * 2 } else { cap };
        let dest = set_alloc_hash_tid(new_cap, n2, tid);
        for i in 0..n as usize {
            set_hash_put_new(dest, set_elem_at(set, i), i);
        }
        set_hash_put_new(dest, elem, n as usize);
        dest
    }
}

/// Drop element if present; returns new Set (insertion order of remaining elems).
#[no_mangle]
pub extern "C" fn lumia_set_remove(set: *mut u8, elem: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let tid = set_type_id(set);
        if set.is_null() {
            let dest = lumia_alloc(8, TYPE_SET);
            *(dest as *mut i64) = 0;
            return dest;
        }
        if set_is_hash(set) {
            let base = set as *const i64;
            let n = *base;
            let cap = *base.add(1) as usize;
            let Some(slot) = set_hash_find_slot(set, elem) else {
                let nbytes = set_hash_nbytes(cap) as u64;
                let dest = lumia_alloc(nbytes, tid);
                ptr::copy_nonoverlapping(set, dest, nbytes as usize);
                return dest;
            };
            let n2 = n - 1;
            if n2 <= SET_SMALL_MAX {
                let dest = lumia_alloc(set_linear_nbytes(n2) as u64, tid);
                let dst = dest as *mut i64;
                *dst = n2;
                let mut w = 0usize;
                for i in 0..n as usize {
                    let s = *base.add(2 + i) as usize;
                    if s == slot {
                        continue;
                    }
                    *dst.add(1 + w) = *base.add(2 + cap + s * 2);
                    w += 1;
                }
                return dest;
            }
            let dest = set_alloc_hash_tid(cap, n2, tid);
            let mut w = 0usize;
            for i in 0..n as usize {
                let s = *base.add(2 + i) as usize;
                if s == slot {
                    continue;
                }
                set_hash_put_new(dest, *base.add(2 + cap + s * 2), w);
                w += 1;
            }
            return dest;
        }

        let n = *(set as *const i64);
        let base = set as *const i64;
        let float_elems = set_float_elems(set);
        let mut idx = None;
        for i in 0..n as usize {
            if key_eq(*base.add(1 + i), elem, float_elems) {
                idx = Some(i);
                break;
            }
        }
        let Some(idx) = idx else {
            let nbytes = set_linear_nbytes(n) as u64;
            let dest = lumia_alloc(nbytes, tid);
            ptr::copy_nonoverlapping(set, dest, nbytes as usize);
            return dest;
        };
        let n2 = n - 1;
        let dest = lumia_alloc(set_linear_nbytes(n2) as u64, tid);
        let dst = dest as *mut i64;
        *dst = n2;
        let mut w = 0usize;
        for j in 0..n as usize {
            if j == idx {
                continue;
            }
            *dst.add(1 + w) = *base.add(1 + j);
            w += 1;
        }
        dest
    }
}

/// Dispatch remove for Map or Set.
#[no_mangle]
pub extern "C" fn lumia_remove(obj: *mut u8, key_or_elem: i64) -> *mut u8 {
    if obj.is_null() {
        // Ambiguous empty — prefer Map (same historical default as typed `remove`).
        return lumia_map_remove(obj, key_or_elem);
    }
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        tid if is_map_tid(tid) => lumia_map_remove(obj, key_or_elem),
        TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => lumia_set_remove(obj, key_or_elem),
        _ => trap_abort(&format!("lumia: remove on unsupported type_id={tid}")),
    }
}

/// Dispatch get: List/Set by index → i64 elem; Map by key → Option ADT ptr as i64.
#[no_mangle]
pub extern "C" fn lumia_get(
    obj: *mut u8,
    key_or_index: i64,
    some_tag: i64,
    none_tag: i64,
) -> i64 {
    if obj.is_null() {
        trap_abort("lumia: get on null");
    }
    let h = header_from_payload(obj);
    unsafe {
        match (*h).type_id {
            TYPE_LIST | TYPE_LIST_F64 | TYPE_LIST_IOTA => lumia_list_get(obj, key_or_index),
            TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => {
                let n = *(obj as *const i64);
                if key_or_index < 0 || key_or_index >= n {
                    trap_abort("lumia: set get OOB");
                }
                set_elem_at(obj, key_or_index as usize)
            }
            tid if is_map_tid(tid) => {
                let opt = lumia_map_get(obj, key_or_index, some_tag, none_tag);
                opt as i64
            }
            other => trap_abort(&format!("lumia: get unsupported type_id {other}")),
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_contains(obj: *mut u8, key: i64) -> i64 {
    if obj.is_null() {
        return 0;
    }
    let h = header_from_payload(obj);
    unsafe {
        match (*h).type_id {
            tid if is_map_tid(tid) => lumia_map_contains(obj, key),
            TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => lumia_set_contains(obj, key),
            TYPE_STRING => lumia_str_contains(obj, key as *mut u8),
            other => trap_abort(&format!("lumia: contains unsupported type_id {other}")),
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_adt_tag(obj: *mut u8) -> i64 {
    if obj.is_null() {
        trap_abort("lumia: adt_tag on null");
    }
    unsafe { *(obj as *const i64) }
}

#[no_mangle]
pub extern "C" fn lumia_adt_field(obj: *mut u8, index: i64) -> i64 {
    if obj.is_null() || index < 0 {
        trap_abort("lumia: adt_field OOB");
    }
    unsafe {
        let h = header_from_payload(obj);
        let words = ((*h).size as usize) / 8;
        // Layout: [tag][field0]… → field count = words - 1
        if words == 0 || (index as usize) + 1 >= words {
            trap_abort("lumia: adt_field OOB");
        }
        let base = obj as *const i64;
        *base.add(1 + index as usize)
    }
}

/// Transparent Memo `T_f` — fixed small associative tables (DESIGN §7.5.1-B).
/// Internal symbol prefix `memo_l2_*` kept for ABI stability.
pub const MEMO_L2_MAX_FUNS: usize = 64;
pub const MEMO_L2_SLOTS: usize = 4;
pub const MEMO_L2_MAX_ARGS: usize = 4;

/// Process-level hard bound on transparent Memo bytes (slots + dense), versioned (§7.5.0).
pub const MEMO_PROCESS_BYTE_CAP: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
struct MemoL2Slot {
    valid: bool,
    nargs: u8,
    args: [i64; MEMO_L2_MAX_ARGS],
    result: i64,
}

impl MemoL2Slot {
    const EMPTY: Self = Self {
        valid: false,
        nargs: 0,
        args: [0; MEMO_L2_MAX_ARGS],
        result: 0,
    };

    fn matches(&self, nargs: u8, args: &[i64; MEMO_L2_MAX_ARGS]) -> bool {
        self.valid
            && self.nargs == nargs
            && self.args[..nargs as usize] == args[..nargs as usize]
    }
}

struct MemoL2Table {
    slots: [MemoL2Slot; MEMO_L2_SLOTS],
    next_victim: usize,
    hits: u64,
    misses: u64,
}

impl MemoL2Table {
    const EMPTY: Self = Self {
        slots: [MemoL2Slot::EMPTY; MEMO_L2_SLOTS],
        next_victim: 0,
        hits: 0,
        misses: 0,
    };
}

thread_local! {
    static MEMO_L2: RefCell<[MemoL2Table; MEMO_L2_MAX_FUNS]> =
        const { RefCell::new([MemoL2Table::EMPTY; MEMO_L2_MAX_FUNS]) };
}

fn pack_args(a0: i64, a1: i64, a2: i64, a3: i64) -> [i64; MEMO_L2_MAX_ARGS] {
    [a0, a1, a2, a3]
}

/// Lookup: returns 1 and writes `*out_result` on hit; else 0.
#[no_mangle]
pub extern "C" fn lumia_memo_l2_lookup(
    fun_id: i64,
    nargs: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    a3: i64,
    out_result: *mut i64,
) -> i64 {
    if fun_id < 0 || fun_id as usize >= MEMO_L2_MAX_FUNS || out_result.is_null() {
        return 0;
    }
    let nargs = nargs.clamp(0, MEMO_L2_MAX_ARGS as i64) as u8;
    let args = pack_args(a0, a1, a2, a3);
    MEMO_L2.with(|t| {
        let mut tables = t.borrow_mut();
        let table = &mut tables[fun_id as usize];
        for slot in &table.slots {
            if slot.matches(nargs, &args) {
                table.hits += 1;
                unsafe {
                    *out_result = slot.result;
                }
                return 1;
            }
        }
        table.misses += 1;
        0
    })
}

/// Store result into a slot (round-robin eviction).
#[no_mangle]
pub extern "C" fn lumia_memo_l2_store(
    fun_id: i64,
    nargs: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    a3: i64,
    result: i64,
) {
    if fun_id < 0 || fun_id as usize >= MEMO_L2_MAX_FUNS {
        return;
    }
    let nargs = nargs.clamp(0, MEMO_L2_MAX_ARGS as i64) as u8;
    let args = pack_args(a0, a1, a2, a3);
    MEMO_L2.with(|t| {
        let mut tables = t.borrow_mut();
        let table = &mut tables[fun_id as usize];
        for slot in &mut table.slots {
            if slot.matches(nargs, &args) {
                slot.result = result;
                return;
            }
        }
        let i = table.next_victim % MEMO_L2_SLOTS;
        table.next_victim = i + 1;
        let mut stored = [0i64; MEMO_L2_MAX_ARGS];
        stored[..nargs as usize].copy_from_slice(&args[..nargs as usize]);
        table.slots[i] = MemoL2Slot {
            valid: true,
            nargs,
            args: stored,
            result,
        };
    });
}

/// Test / `--show-memo-stats` helper: total hits across tables.
#[no_mangle]
pub extern "C" fn lumia_memo_l2_hits() -> i64 {
    MEMO_L2.with(|t| {
        t.borrow().iter().map(|x| x.hits as i64).sum()
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_l2_misses() -> i64 {
    MEMO_L2.with(|t| {
        t.borrow().iter().map(|x| x.misses as i64).sum()
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_l2_reset() {
    MEMO_L2.with(|t| {
        *t.borrow_mut() = [MemoL2Table::EMPTY; MEMO_L2_MAX_FUNS];
    });
}

/// Dense Int-key `T_f` for structural recursion (DESIGN §7.5.3) — prefer over hashing.
pub const MEMO_IDX_MAX_FUNS: usize = 16;
pub const MEMO_IDX_CAP: usize = 4096;
/// Bytes reserved per dense table when allocated (valid bitmap + values).
pub const MEMO_IDX_TABLE_BYTES: usize = MEMO_IDX_CAP * (1 + 8);

struct MemoIdxTable {
    valid: [u8; MEMO_IDX_CAP],
    values: [i64; MEMO_IDX_CAP],
    hits: u64,
    misses: u64,
}

impl MemoIdxTable {
    fn new() -> Box<Self> {
        Box::new(Self {
            valid: [0; MEMO_IDX_CAP],
            values: [0; MEMO_IDX_CAP],
            hits: 0,
            misses: 0,
        })
    }
}

thread_local! {
    // Lazy: allocate a dense table only on first use of that `fun_id` (§7.5 low occupancy).
    static MEMO_IDX: RefCell<[Option<Box<MemoIdxTable>>; MEMO_IDX_MAX_FUNS]> =
        const { RefCell::new([const { None }; MEMO_IDX_MAX_FUNS]) };
}

fn memo_idx_table(
    tables: &mut [Option<Box<MemoIdxTable>>; MEMO_IDX_MAX_FUNS],
    fun_id: usize,
) -> &mut MemoIdxTable {
    if tables[fun_id].is_none() {
        tables[fun_id] = Some(MemoIdxTable::new());
    }
    tables[fun_id].as_mut().unwrap()
}

/// Lookup by Int key in `0..MEMO_IDX_CAP`. Returns 1 + writes result on hit.
#[no_mangle]
pub extern "C" fn lumia_memo_idx_lookup(fun_id: i64, key: i64, out_result: *mut i64) -> i64 {
    if fun_id < 0
        || fun_id as usize >= MEMO_IDX_MAX_FUNS
        || out_result.is_null()
        || key < 0
        || key as usize >= MEMO_IDX_CAP
    {
        return 0;
    }
    let k = key as usize;
    MEMO_IDX.with(|t| {
        let mut tables = t.borrow_mut();
        let table = memo_idx_table(&mut tables, fun_id as usize);
        if table.valid[k] != 0 {
            table.hits += 1;
            unsafe {
                *out_result = table.values[k];
            }
            1
        } else {
            table.misses += 1;
            0
        }
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_store(fun_id: i64, key: i64, result: i64) {
    if fun_id < 0 || fun_id as usize >= MEMO_IDX_MAX_FUNS || key < 0 || key as usize >= MEMO_IDX_CAP
    {
        return;
    }
    let k = key as usize;
    MEMO_IDX.with(|t| {
        let mut tables = t.borrow_mut();
        let table = memo_idx_table(&mut tables, fun_id as usize);
        table.valid[k] = 1;
        table.values[k] = result;
    });
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_hits() -> i64 {
    MEMO_IDX.with(|t| {
        t.borrow()
            .iter()
            .filter_map(|x| x.as_ref())
            .map(|x| x.hits as i64)
            .sum()
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_misses() -> i64 {
    MEMO_IDX.with(|t| {
        t.borrow()
            .iter()
            .filter_map(|x| x.as_ref())
            .map(|x| x.misses as i64)
            .sum()
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_reset() {
    MEMO_IDX.with(|t| {
        *t.borrow_mut() = [const { None }; MEMO_IDX_MAX_FUNS];
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_collect_unrooted() {
        let p = lumia_alloc(16, TYPE_BYTES);
        assert!(!p.is_null());
        // Not rooted → collect should free
        lumia_gc_collect();
        // Heap should be empty or not contain live unmarked — allocate again
        let q = lumia_alloc(8, TYPE_BYTES);
        assert!(!q.is_null());
    }

    #[test]
    fn rooted_survives_collect() {
        let mut slot: *mut u8 = lumia_alloc(32, TYPE_STRING);
        lumia_root_push(&mut slot as *mut *mut u8);
        lumia_gc_collect();
        assert!(!slot.is_null());
        // header still valid
        let h = header_from_payload(slot);
        unsafe {
            assert_eq!((*h).type_id, TYPE_STRING);
            assert_eq!((*h).size, 32);
        }
        lumia_root_pop();
    }

    #[test]
    fn write_barrier_empty_under_stw() {
        let p = lumia_alloc(8, TYPE_BYTES);
        lumia_write_barrier(p, 0, ptr::null_mut());
    }

    #[test]
    fn println_int_smoke() {
        lumia_println_int(7);
    }

    #[test]
    fn rooted_survives_soft_threshold() {
        // Lower limit temporarily via many small allocs with a rooted object.
        let mut slot: *mut u8 = lumia_alloc(64, TYPE_STRING);
        lumia_root_push(&mut slot as *mut *mut u8);
        for _ in 0..5000 {
            let _ = lumia_alloc(64, TYPE_BYTES);
        }
        assert!(!slot.is_null());
        let h = header_from_payload(slot);
        unsafe {
            assert_eq!((*h).type_id, TYPE_STRING);
            assert_eq!((*h).size, 64);
        }
        lumia_root_pop();
    }

    #[test]
    fn map_promotes_to_hash_and_looks_up() {
        let mut m: *mut u8 = ptr::null_mut();
        for i in 0..20 {
            m = lumia_map_set(m, i, i * 10);
        }
        assert!(!m.is_null());
        assert!(map_is_hash(m) || map_is_overlay(m));
        assert_eq!(map_count(m), 20);
        for i in 0..20 {
            assert_eq!(lumia_map_contains(m, i), 1);
            let opt = lumia_map_get(m, i, 0, 1);
            // Some(v) tag 0 with field
            unsafe {
                let base = opt as *const i64;
                assert_eq!(*base, 0);
                assert_eq!(*base.add(1), i * 10);
            }
        }
        assert_eq!(lumia_map_contains(m, 99), 0);
        m = lumia_map_remove(m, 5);
        assert_eq!(lumia_map_contains(m, 5), 0);
        assert_eq!(map_count(m), 19);
        // Still insertion-ordered keys without 5
        let keys = lumia_map_keys(m);
        unsafe {
            assert_eq!(*(keys as *const i64), 19);
            assert_eq!(*((keys as *const i64).add(1)), 0);
        }
    }

    #[test]
    fn map_overlay_set_avoids_full_clone() {
        let mut m: *mut u8 = ptr::null_mut();
        for i in 0..9 {
            m = lumia_map_set(m, i, i);
        }
        assert!(map_is_hash(m), "expected hash after promoting past small max");
        m = lumia_map_set(m, 100, 42);
        assert!(map_is_overlay(m));
        assert_eq!(map_count(m), 10);
        assert_eq!(lumia_map_contains(m, 100), 1);
        assert_eq!(lumia_map_contains(m, 3), 1);
        // Another set extends delta (still overlay).
        m = lumia_map_set(m, 101, 7);
        assert!(map_is_overlay(m));
        unsafe {
            assert_eq!(map_overlay_dn(m), 2);
        }
        assert_eq!(map_count(m), 11);
        assert_eq!(lumia_map_contains(m, 101), 1);
    }

    #[test]
    fn set_promotes_to_hash_and_contains() {
        let mut s: *mut u8 = ptr::null_mut();
        for i in 0..20 {
            s = lumia_set_insert(s, i);
        }
        assert!(!s.is_null());
        assert!(set_is_hash(s));
        assert_eq!(unsafe { *(s as *const i64) }, 20);
        for i in 0..20 {
            assert_eq!(lumia_set_contains(s, i), 1);
            assert_eq!(unsafe { set_elem_at(s, i as usize) }, i);
        }
        assert_eq!(lumia_set_contains(s, 99), 0);
        s = lumia_set_remove(s, 5);
        assert_eq!(lumia_set_contains(s, 5), 0);
        assert_eq!(unsafe { *(s as *const i64) }, 19);
        assert_eq!(unsafe { set_elem_at(s, 0) }, 0);
        assert_eq!(unsafe { set_elem_at(s, 5) }, 6);
        // Shrink far enough to demote to linear
        for i in 0..12 {
            s = lumia_set_remove(s, i);
        }
        assert!(!set_is_hash(s));
        assert_eq!(unsafe { *(s as *const i64) }, 8);
    }

    #[test]
    fn memo_l2_hit_miss() {
        lumia_memo_l2_reset();
        let mut out = 0i64;
        assert_eq!(lumia_memo_l2_lookup(0, 1, 42, 0, 0, 0, &mut out), 0);
        lumia_memo_l2_store(0, 1, 42, 0, 0, 0, 99);
        assert_eq!(lumia_memo_l2_lookup(0, 1, 42, 0, 0, 0, &mut out), 1);
        assert_eq!(out, 99);
        assert_eq!(lumia_memo_l2_lookup(0, 1, 7, 0, 0, 0, &mut out), 0);
        // 4-arg key
        lumia_memo_l2_store(1, 4, 1, 2, 3, 4, 77);
        assert_eq!(lumia_memo_l2_lookup(1, 4, 1, 2, 3, 4, &mut out), 1);
        assert_eq!(out, 77);
        assert_eq!(lumia_memo_l2_lookup(1, 4, 1, 2, 3, 5, &mut out), 0);
        assert!(lumia_memo_l2_hits() >= 2);
        assert!(lumia_memo_l2_misses() >= 2);
        lumia_memo_l2_reset();
    }

    #[test]
    fn memo_idx_hit_miss() {
        lumia_memo_idx_reset();
        let mut out = 0i64;
        assert_eq!(lumia_memo_idx_lookup(0, 10, &mut out), 0);
        lumia_memo_idx_store(0, 10, 55);
        assert_eq!(lumia_memo_idx_lookup(0, 10, &mut out), 1);
        assert_eq!(out, 55);
        assert_eq!(lumia_memo_idx_lookup(0, 11, &mut out), 0);
        assert_eq!(lumia_memo_idx_lookup(0, -1, &mut out), 0);
        assert_eq!(lumia_memo_idx_lookup(0, MEMO_IDX_CAP as i64, &mut out), 0);
        assert!(lumia_memo_idx_hits() >= 1);
        assert!(lumia_memo_idx_misses() >= 1);
        lumia_memo_idx_reset();
    }

    #[test]
    fn range_is_iota_not_materialized() {
        let r = lumia_range(0, 1_000_000);
        assert!(!r.is_null());
        unsafe {
            assert_eq!((*header_from_payload(r)).type_id, TYPE_LIST_IOTA);
            assert_eq!((*header_from_payload(r)).size, 16);
        }
        assert_eq!(lumia_list_len(r), 1_000_000);
        assert_eq!(lumia_list_get(r, 0), 0);
        assert_eq!(lumia_list_get(r, 999_999), 999_999);
        // Content-equal to a small heap list of the same prefix.
        let h = lumia_range(10, 13);
        let forced = force_heap_list(h);
        unsafe {
            assert_eq!((*header_from_payload(forced)).type_id, TYPE_LIST);
        }
        assert_eq!(lumia_eq(h as i64, forced as i64), 1);
        assert_eq!(lumia_list_len(lumia_list_take(r, 3)), 3);
        assert_eq!(lumia_list_get(lumia_list_slice(r, 5), 0), 5);
    }

    #[test]
    fn empty_list_singleton_survives_gc() {
        let a = lumia_list_empty();
        let b = lumia_list_empty();
        assert_eq!(a, b);
        assert_eq!(lumia_list_len(a), 0);
        // Force a collection; permanent root must keep the singleton alive.
        lumia_gc_collect();
        assert_eq!(lumia_list_empty(), a);
        // Identity concat on a heap list (Iota would be forced first).
        let xs = force_heap_list(lumia_range(1, 4));
        let id = lumia_list_concat(lumia_list_empty(), xs);
        assert_eq!(id, xs);
        assert_eq!(lumia_list_len(id), 3);
        assert_eq!(lumia_list_concat(xs, lumia_list_empty()), xs);
    }

    #[test]
    #[should_panic(expected = "list too large")]
    fn force_huge_iota_traps_without_alloc() {
        // Length that cannot fit in ObjectHeader.size (u32) when stored as bytes.
        let n = (u32::MAX as i64 / 8) + 8;
        let r = lumia_range(0, n);
        let _ = force_heap_list(r);
    }

    #[test]
    #[should_panic(expected = "list too large")]
    fn list_payload_bytes_rejects_overflow() {
        let _ = list_payload_bytes(i64::MAX);
    }

    #[test]
    #[should_panic(expected = "parallel map worker")]
    fn par_worker_alloc_is_forbidden() {
        PAR_WORKER.with(|c| c.set(true));
        // Call Rust path (not `extern "C"`) so the panic can unwind for should_panic.
        let _ = MarkSweep.alloc(8, TYPE_LIST);
    }

    #[test]
    fn list_f64_eq_follows_ieee() {
        let pos0 = 0.0f64.to_bits() as i64;
        let neg0 = (-0.0f64).to_bits() as i64;
        let nan = f64::NAN.to_bits() as i64;
        let a = {
            let p = lumia_alloc(list_payload_bytes(1), TYPE_LIST_F64);
            unsafe {
                *(p as *mut i64) = 1;
                *((p as *mut i64).add(1)) = pos0;
            }
            p
        };
        let b = {
            let p = lumia_alloc(list_payload_bytes(1), TYPE_LIST_F64);
            unsafe {
                *(p as *mut i64) = 1;
                *((p as *mut i64).add(1)) = neg0;
            }
            p
        };
        let c = {
            let p = lumia_alloc(list_payload_bytes(1), TYPE_LIST_F64);
            unsafe {
                *(p as *mut i64) = 1;
                *((p as *mut i64).add(1)) = nan;
            }
            p
        };
        assert_eq!(lumia_eq(a as i64, b as i64), 1);
        // Same object still NaN≠NaN under IEEE content compare.
        assert_eq!(lumia_eq(c as i64, c as i64), 0);
        let c2 = {
            let p = lumia_alloc(list_payload_bytes(1), TYPE_LIST_F64);
            unsafe {
                *(p as *mut i64) = 1;
                *((p as *mut i64).add(1)) = nan;
            }
            p
        };
        assert_eq!(lumia_eq(c as i64, c2 as i64), 0);
    }

}

