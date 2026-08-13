//! Parallel list map / fold (C ABI workers; GC inhibited).

use super::core::{list_len_of, lumia_list_empty};
use super::tid::list_tid;
use crate::common::{list_elem_is_float, trap_abort, GcInhibitGuard, PAR_WORKER, TYPE_LIST_IOTA};
use crate::gc::{list_payload_bytes, lumia_alloc};
use lumia_abi::list_type_id;

/// Parallel map over List[scalar] with a C ABI `fn(i64) -> i64`.
/// `result_tid` is a list type_id (codegen: result element sort).
/// Type checker requires concrete Int/Bool/Float elems; workers must not heap-allocate.
/// Falls back to sequential for small lists; inhibits GC while workers run.
///
/// Iota (`range`) is consumed virtually — no materialization. Workers write
/// directly into the preallocated destination (no per-chunk `Vec` gather).
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
    let _gc = GcInhibitGuard::enter();
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
        // Sequential for tiny lists.
        if n < 64 {
            if iota {
                for i in 0..n as usize {
                    let x = iota_start
                        .checked_add(i as i64)
                        .unwrap_or_else(|| trap_abort("lumia: iota index overflow"));
                    *dst.add(1 + i) = f(x);
                }
            } else {
                let src = src_addr as *const i64;
                for i in 0..n as usize {
                    *dst.add(1 + i) = f(*src.add(1 + i));
                }
            }
            return dest;
        }
        let workers = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4)
            .min(n as usize)
            .max(1);
        let chunk = (n as usize).div_ceil(workers);
        let dest_base = dst as usize;
        let mut handles = Vec::with_capacity(workers);
        for w in 0..workers {
            let start = w * chunk;
            let end = ((w + 1) * chunk).min(n as usize);
            if start >= end {
                continue;
            }
            // SAFETY: dest/list immutable layout during map; GC inhibited; workers
            // must not allocate (PAR_WORKER). Each worker owns a disjoint dst slice.
            handles.push(std::thread::spawn(move || {
                PAR_WORKER.with(|c| c.set(true));
                let dst = dest_base as *mut i64;
                if iota {
                    for i in start..end {
                        let x = iota_start
                            .checked_add(i as i64)
                            .unwrap_or_else(|| trap_abort("lumia: iota index overflow"));
                        *dst.add(1 + i) = f(x);
                    }
                } else {
                    let src = src_addr as *const i64;
                    for i in start..end {
                        *dst.add(1 + i) = f(*src.add(1 + i));
                    }
                }
            }));
        }
        for h in handles {
            if h.join().is_err() {
                trap_abort("lumia: par_map worker panicked");
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
    unsafe {
        if n <= 0 {
            return init;
        }
        if n < 64 {
            let mut acc = init;
            if iota {
                for i in 0..n as usize {
                    let x = iota_start
                        .checked_add(i as i64)
                        .unwrap_or_else(|| trap_abort("lumia: iota index overflow"));
                    acc = f(acc, x);
                }
            } else {
                let src = src_addr as *const i64;
                for i in 0..n as usize {
                    acc = f(acc, *src.add(1 + i));
                }
            }
            return acc;
        }
        let workers = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4)
            .min(n as usize)
            .max(1);
        let chunk = (n as usize).div_ceil(workers);
        let mut handles = Vec::with_capacity(workers);
        for w in 0..workers {
            let start = w * chunk;
            let end = ((w + 1) * chunk).min(n as usize);
            if start >= end {
                continue;
            }
            handles.push(std::thread::spawn(move || {
                PAR_WORKER.with(|c| c.set(true));
                if iota {
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
                }
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
