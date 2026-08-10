//! List operations and ranges.

use std::cell::Cell;
use std::ptr;

use crate::common::{
    header_from_payload, is_heap_payload, list_elem_is_float, list_rc_is_unique, list_rc_release,
    list_rc_retain, tid_base, trap_abort, GcInhibitGuard, PAR_WORKER, PERM_OBJECTS, RC_SHARED,
    TYPE_LIST, TYPE_LIST_F64, TYPE_LIST_IOTA,
};
use crate::gc::{list_payload_bytes, lumia_alloc};
use crate::show_eq::lumia_ord_cmp;
use crate::string_io::{lumia_alloc_string, with_str_bytes};

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
pub(crate) fn is_list_tid(tid: u32) -> bool {
    lumia_abi::is_list_tid(tid)
}

#[inline]
pub(crate) fn list_tid(list: *mut u8) -> u32 {
    if list.is_null() {
        TYPE_LIST
    } else {
        unsafe { (*header_from_payload(list)).type_id }
    }
}

/// Preserve Float-elem tagging when allocating a derived HeapList.
#[inline]
pub(crate) fn heap_list_tid(list: *mut u8) -> u32 {
    if list_elem_is_float(list_tid(list)) {
        TYPE_LIST_F64
    } else {
        TYPE_LIST
    }
}

pub(crate) fn list_float_elems(list: *mut u8) -> bool {
    list_elem_is_float(list_tid(list))
}

/// Ensure a list uses IEEE elem eq/hash (`TYPE_LIST_F64`).
/// Empty ordinary lists become a fresh empty F64 list (no in-place retag).
pub(crate) fn ensure_list_f64(list: *mut u8) -> *mut u8 {
    if list.is_null() {
        let dest = lumia_alloc(8, TYPE_LIST_F64);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let h = header_from_payload(list);
        let tid = (*h).type_id;
        if list_elem_is_float(tid) {
            return list;
        }
        if tid_base(tid) == TYPE_LIST {
            if *(list as *const i64) != 0 {
                trap_abort("lumia: ensure_list_f64 on non-empty Int-elem list");
            }
            let dest = lumia_alloc(8, TYPE_LIST_F64);
            *(dest as *mut i64) = 0;
            return dest;
        }
        if tid_base(tid) == TYPE_LIST_IOTA {
            trap_abort("lumia: ensure_list_f64 on Iota");
        }
        trap_abort(&format!("lumia: ensure_list_f64 on type_id={tid}"))
    }
}

#[no_mangle]
pub extern "C" fn lumia_ensure_list_f64(list: *mut u8) -> *mut u8 {
    ensure_list_f64(list)
}

/// HeapList: `[len][elem…]`; Iota: `[start][end_exclusive]`.
pub(crate) fn list_len_of(list: *mut u8) -> i64 {
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

pub(crate) fn list_get_of(list: *mut u8, index: i64) -> i64 {
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
pub(crate) fn force_heap_list(list: *mut u8) -> *mut u8 {
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

/// Promote stack `LitList` / Iota to a heap list so the pointer may escape.
/// Immortal empty singleton and existing heap payloads are returned unchanged.
#[no_mangle]
pub extern "C" fn lumia_list_promote(list: *mut u8) -> *mut u8 {
    if list.is_null() {
        return list;
    }
    let list = force_heap_list(list);
    if is_heap_payload(list) {
        return list;
    }
    // Empty immortal singleton is registered as a permanent object.
    if list == lumia_list_empty() {
        return list;
    }
    let tid = list_tid(list);
    if tid_base(tid) != TYPE_LIST {
        return list;
    }
    let _guard = GcInhibitGuard::enter();
    let n = list_len_of(list);
    if n < 0 {
        trap_abort("lumia: list promote length overflow");
    }
    let dest = lumia_alloc(list_payload_bytes(n), tid);
    unsafe {
        let dst = dest as *mut i64;
        let src = list as *const i64;
        *dst = n;
        for i in 0..n as usize {
            *dst.add(1 + i) = *src.add(1 + i);
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

/// Capacity (element slots) from the allocated payload size (`[len][elem…]`).
#[inline]
fn list_capacity_elems(list: *mut u8) -> i64 {
    if list.is_null() {
        return 0;
    }
    unsafe {
        let nbytes = (*header_from_payload(list)).size as i64;
        (nbytes / 8) - 1
    }
}

fn list_grow_cap(needed: i64) -> i64 {
    // Geometric growth: amortize repeated unique appends.
    let mut cap = 4i64;
    while cap < needed {
        cap = cap
            .checked_mul(2)
            .unwrap_or_else(|| trap_abort("lumia: list capacity overflow"));
    }
    cap
}

/// Return a HeapList with `elem` appended (COW: unique + spare capacity → in-place).
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
        let tid = heap_list_tid(list);

        // Unique owner with spare capacity → write in place (DESIGN §5.3 / §7.1.1 COWList).
        if !list.is_null() && list_rc_is_unique(list) && list_capacity_elems(list) >= n1 {
            let dst = list as *mut i64;
            *dst = n1;
            *dst.add(n1 as usize) = elem;
            return list;
        }

        let cap = if !list.is_null() && list_rc_is_unique(list) {
            list_grow_cap(n1.max(list_capacity_elems(list).saturating_mul(2)))
        } else {
            list_grow_cap(n1)
        };
        let nbytes = list_payload_bytes(cap);
        let dest = lumia_alloc(nbytes, tid);
        if dest.is_null() {
            trap_abort("lumia: list append OOM");
        }
        let dst = dest as *mut i64;
        *dst = n1;
        if !list.is_null() {
            let src = list as *const i64;
            ptr::copy_nonoverlapping(src.add(1), dst.add(1), n as usize);
        }
        *dst.add(n1 as usize) = elem;
        dest
    }
}

/// Retain a List value when aliasing (`val a = xs`). No-op for non-lists.
#[no_mangle]
pub extern "C" fn lumia_list_retain(list: *mut u8) {
    list_rc_retain(list);
}

/// Release a List alias (does not free; GC reclaims unreachable objects).
#[no_mangle]
pub extern "C" fn lumia_list_release(list: *mut u8) {
    list_rc_release(list);
}

/// Parallel map over List[scalar] with a C ABI `fn(i64) -> i64`.
/// `result_tid` is `TYPE_LIST` or `TYPE_LIST_F64` (codegen: result element sort).
/// Type checker requires concrete Int/Bool/Float elems; workers must not heap-allocate.
/// Falls back to sequential for small lists; inhibits GC while workers run.
#[no_mangle]
pub extern "C" fn lumia_list_par_map(
    list: *mut u8,
    f: Option<extern "C" fn(i64) -> i64>,
    result_tid: u32,
) -> *mut u8 {
    let Some(f) = f else {
        trap_abort("lumia: list_par_map null function");
    };
    let result_tid = if list_elem_is_float(result_tid) {
        TYPE_LIST_F64
    } else {
        TYPE_LIST
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
            return if list_elem_is_float(result_tid) {
                ensure_list_f64(lumia_list_empty())
            } else {
                lumia_list_empty()
            };
        }
        let src = list as *const i64;
        // Sequential for tiny lists.
        if n < 64 {
            let dest = lumia_alloc(list_payload_bytes(n), result_tid);
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
        let chunk = (n as usize).div_ceil(workers);
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
            .map(|h| match h.join() {
                Ok(v) => v,
                Err(_) => trap_abort("lumia: par_map worker panicked"),
            })
            .collect();
        let dest = lumia_alloc(list_payload_bytes(n), result_tid);
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
        let chunk = (n as usize).div_ceil(workers);
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
            let part = match h.join() {
                Ok(v) => v,
                Err(_) => trap_abort("lumia: par_fold worker panicked"),
            };
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
        // Immutable lists: concat with empty is identity (share the other),
        // but stack LitList must be promoted before the pointer escapes.
        if na == 0 {
            return if nb == 0 {
                lumia_list_empty()
            } else {
                lumia_list_promote(b)
            };
        }
        if nb == 0 {
            return lumia_list_promote(a);
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
            // Immortal shared empty list — never COW in-place.
            (*header_from_payload(dest))._pad = RC_SHARED;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::header_from_payload;
    use crate::TYPE_LIST_IOTA;

    #[test]
    fn range_empty_and_inverted() {
        let empty = lumia_range(5, 5);
        assert_eq!(list_len_of(empty), 0);
        let inv = lumia_range(10, 3);
        assert_eq!(list_len_of(inv), 0);
        let incl = lumia_range_inclusive(2, 4);
        assert_eq!(list_len_of(incl), 3);
        assert_eq!(list_get_of(incl, 2), 4);
    }

    #[test]
    fn iota_take_preserves_iota_tag() {
        let r = lumia_range(10, 20);
        let t = lumia_list_take(r, 3);
        unsafe {
            assert_eq!((*header_from_payload(t)).type_id, TYPE_LIST_IOTA);
        }
        assert_eq!(list_len_of(t), 3);
        assert_eq!(list_get_of(t, 0), 10);
        assert_eq!(list_get_of(t, 2), 12);
        // Negative / oversized take clamps.
        assert_eq!(list_len_of(lumia_list_take(r, -1)), 0);
        assert_eq!(list_len_of(lumia_list_take(r, 100)), 10);
    }

    #[test]
    fn reverse_and_sort_heap_list() {
        let mut xs = lumia_list_empty();
        for v in [3, 1, 2] {
            xs = lumia_list_append(xs, v);
        }
        let rev = lumia_list_reverse(xs);
        assert_eq!(list_get_of(rev, 0), 2);
        assert_eq!(list_get_of(rev, 2), 3);
        let sorted = lumia_list_sort(xs);
        assert_eq!(list_get_of(sorted, 0), 1);
        assert_eq!(list_get_of(sorted, 1), 2);
        assert_eq!(list_get_of(sorted, 2), 3);
    }
}
