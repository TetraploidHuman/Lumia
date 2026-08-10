//! Parallel list map / fold (C ABI workers; GC inhibited).

use super::core::{force_heap_list, lumia_list_empty};
use super::tid::ensure_list_f64;
use crate::common::{
    list_elem_is_float, trap_abort, GcInhibitGuard, PAR_WORKER, TYPE_LIST, TYPE_LIST_F64,
};
use crate::gc::{list_payload_bytes, lumia_alloc};

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
