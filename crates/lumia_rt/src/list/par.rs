//! Parallel list map / fold (C ABI workers; GC inhibited).

use super::core::{list_len_of, lumia_list_empty};
use super::tid::list_tid;
use crate::common::{list_elem_is_float, trap_abort, GcInhibitGuard, PAR_WORKER, TYPE_LIST_IOTA};
use crate::gc::{list_payload_bytes, lumia_alloc};
use crate::task::task_runtime_active;
use lumia_abi::list_type_id;

/// Parallel workers must not run under an active Task/Channel scheduler
/// (DESIGN: no mix). Fall back to sequential instead of aborting — spawn
/// bodies may still lower `ListParMap` before demotion.
fn force_sequential_par() -> bool {
    task_runtime_active()
}

/// Parallel map over List[scalar] with a C ABI `fn(i64) -> i64`.
/// `result_tid` is a list type_id (codegen: result element sort).
/// Type checker requires concrete Int/Bool/Float elems; workers must not heap-allocate.
/// Falls back to sequential for small lists; inhibits GC while workers run.
///
/// Iota (`range`) is consumed virtually — no materialization. Workers write
/// into disjoint `&mut [i64]` slices via `thread::scope` (no data race).
#[no_mangle]
pub extern "C" fn lumia_list_par_map(
    list: *mut u8,
    f: Option<extern "C" fn(i64) -> i64>,
    result_tid: u32,
) -> *mut u8 {
    let Some(f) = f else {
        trap_abort("lumia: list_par_map null function");
    };
    let result_tid = list_type_id(list_elem_is_float(result_tid));
    let iota = !list.is_null() && list_tid(list) == TYPE_LIST_IOTA;
    let (n, iota_start, src_addr) = unsafe {
        if list.is_null() {
            (0i64, 0i64, 0usize)
        } else if iota {
            let base = list as *const i64;
            let start = *base;
            let n = list_len_of(list);
            (n, start, 0usize)
        } else {
            // Heap or stack LitList share `[len][elems…]`; only Iota needs a virtual path.
            let n = *(list as *const i64);
            (n, 0i64, list as usize)
        }
    };
    unsafe {
        if n <= 0 {
            return if list_elem_is_float(result_tid) {
                // Fresh empty F64 list — avoid ensure_list_f64(empty) double-path.
                let dest = lumia_alloc(8, list_type_id(true));
                *(dest as *mut i64) = 0;
                dest
            } else {
                lumia_list_empty()
            };
        }
        let dest = lumia_alloc(list_payload_bytes(n), result_tid);
        let dst = dest as *mut i64;
        *dst = n;
        let n_usize = n as usize;
        // Sequential for tiny lists, or under Task/Channel (no OS workers).
        if force_sequential_par() || n < 64 {
            if iota {
                for i in 0..n_usize {
                    let x = iota_start
                        .checked_add(i as i64)
                        .unwrap_or_else(|| trap_abort("lumia: iota index overflow"));
                    *dst.add(1 + i) = f(x);
                }
            } else {
                let src = src_addr as *const i64;
                for i in 0..n_usize {
                    *dst.add(1 + i) = f(*src.add(1 + i));
                }
            }
            return dest;
        }
        // Inhibit GC only while OS workers hold list pointers.
        let _gc = GcInhibitGuard::enter();
        let workers = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4)
            .min(n_usize)
            .max(1);
        let chunk = n_usize.div_ceil(workers);
        // SAFETY: freshly allocated dest elems; exclusive `&mut` slices via chunks_mut.
        let out = std::slice::from_raw_parts_mut(dst.add(1), n_usize);
        std::thread::scope(|scope| {
            for (w, chunk_out) in out.chunks_mut(chunk).enumerate() {
                let start = w * chunk;
                scope.spawn(move || {
                    PAR_WORKER.with(|c| c.set(true));
                    if iota {
                        for (j, slot) in chunk_out.iter_mut().enumerate() {
                            let i = start + j;
                            let x = iota_start
                                .checked_add(i as i64)
                                .unwrap_or_else(|| trap_abort("lumia: iota index overflow"));
                            *slot = f(x);
                        }
                    } else {
                        // SAFETY: src immutable for the duration; GC inhibited; no alloc.
                        let src = src_addr as *const i64;
                        for (j, slot) in chunk_out.iter_mut().enumerate() {
                            let i = start + j;
                            *slot = f(*src.add(1 + i));
                        }
                    }
                });
            }
        });
        dest
    }
}

/// Parallel left-fold over List[scalar] with C ABI `fn(acc, x) -> acc`.
/// Assumes `f` is associative so chunk results can be combined: `f(z, combine(chunks))`.
/// Falls back to sequential for small lists; inhibits GC while workers run.
///
/// Each worker reduces a private index range into a local accumulator (no shared
/// writes); the main thread folds partials. Source is read-only during the scope.
#[no_mangle]
pub extern "C" fn lumia_list_par_fold(
    list: *mut u8,
    init: i64,
    f: Option<extern "C" fn(i64, i64) -> i64>,
) -> i64 {
    let Some(f) = f else {
        trap_abort("lumia: list_par_fold null function");
    };
    let iota = !list.is_null() && list_tid(list) == TYPE_LIST_IOTA;
    let (n, iota_start, src_addr) = unsafe {
        if list.is_null() {
            (0i64, 0i64, 0usize)
        } else if iota {
            let base = list as *const i64;
            let start = *base;
            let n = list_len_of(list);
            (n, start, 0usize)
        } else {
            let n = list_len_of(list);
            (n, 0i64, list as usize)
        }
    };
    if n <= 0 {
        return init;
    }
    let n_usize = n as usize;
    if force_sequential_par() || n < 64 {
        let mut acc = init;
        unsafe {
            if iota {
                for i in 0..n_usize {
                    let x = iota_start
                        .checked_add(i as i64)
                        .unwrap_or_else(|| trap_abort("lumia: iota index overflow"));
                    acc = f(acc, x);
                }
            } else {
                let src = src_addr as *const i64;
                for i in 0..n_usize {
                    acc = f(acc, *src.add(1 + i));
                }
            }
        }
        return acc;
    }
    let _gc = GcInhibitGuard::enter();
    let workers = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4)
        .min(n_usize)
        .max(1);
    let chunk = n_usize.div_ceil(workers);
    let mut partials = vec![0i64; workers];
    std::thread::scope(|scope| {
        for (w, part) in partials.iter_mut().enumerate() {
            let start = w * chunk;
            let end = ((w + 1) * chunk).min(n_usize);
            if start >= end {
                *part = init; // unused; skipped in combine
                continue;
            }
            scope.spawn(move || {
                PAR_WORKER.with(|c| c.set(true));
                unsafe {
                    *part = if iota {
                        let x0 = iota_start
                            .checked_add(start as i64)
                            .unwrap_or_else(|| trap_abort("lumia: iota index overflow"));
                        let mut a = x0;
                        for i in (start + 1)..end {
                            let x = iota_start
                                .checked_add(i as i64)
                                .unwrap_or_else(|| trap_abort("lumia: iota index overflow"));
                            a = f(a, x);
                        }
                        a
                    } else {
                        let src = src_addr as *const i64;
                        let mut a = *src.add(1 + start);
                        for i in (start + 1)..end {
                            a = f(a, *src.add(1 + i));
                        }
                        a
                    };
                }
            });
        }
    });
    let mut acc = init;
    for (w, part) in partials.into_iter().enumerate() {
        let start = w * chunk;
        let end = ((w + 1) * chunk).min(n_usize);
        if start >= end {
            continue;
        }
        acc = f(acc, part);
    }
    acc
}
