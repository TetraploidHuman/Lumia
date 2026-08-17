//! Dense `List[Float]` kernels for tiny GEMV / rank-1 updates (CogniNucleus-scale).
//!
//! Matrices are **flat row-major** `TYPE_LIST_F64` buffers (not `List[List[Float]]`).
//! In-place ops COW-clone when the destination is not uniquely owned, so unique
//! scratch buffers stay zero-alloc across a step.
//!
//! # Safety (FFI)
//! List buffers are null or valid `TYPE_LIST_F64` / Float-elem layouts.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use crate::common::{list_rc_is_unique, trap_abort, GcInhibitGuard, TYPE_LIST_F64};
use crate::gc::{list_payload_bytes, lumia_alloc};
use crate::list::{f64_elems, f64_elems_mut, force_heap_list, list_float_elems, list_len_of, require_len};
use std::ptr;

/// Allocate a length-`n` `List[Float]` filled with `0.0`.
#[no_mangle]
pub extern "C" fn lumia_list_f64_zeros(n: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if n < 0 {
        trap_abort("lumia: list_f64_zeros negative length");
    }
    if n == 0 {
        return unsafe { crate::list::lumia_ensure_list_f64(crate::list::lumia_list_empty()) };
    }
    unsafe {
        let dest = lumia_alloc(list_payload_bytes(n), TYPE_LIST_F64);
        if dest.is_null() {
            trap_abort("lumia: list_f64_zeros OOM");
        }
        let dst = dest as *mut i64;
        *dst = n;
        let elems = dst.add(1) as *mut f64;
        crate::f64_simd::zero_f64(elems, n as usize);
        dest
    }
}

/// Fill every element of a float list with `v` (COW if shared). Returns the list.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_fill(xs: *mut u8, v: f64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let xs = ensure_unique_f64(xs);
    unsafe {
        let (p, n) = f64_elems_mut(xs);
        crate::f64_simd::fill_f64(p, n, v);
    }
    xs
}

/// `xs[i] *= alpha` (COW if shared). Returns `xs`.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_scale(xs: *mut u8, alpha: f64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let xs = ensure_unique_f64(xs);
    unsafe {
        let (p, n) = f64_elems_mut(xs);
        crate::f64_simd::scale_f64(p, n, alpha);
    }
    xs
}

/// Scalar `√x` (for hand-written norms / std that SR into list kernels).
#[no_mangle]
pub extern "C" fn lumia_f64_sqrt(x: f64) -> f64 {
    x.sqrt()
}

/// Scalar `eˣ` (for hand-written softmax that SR into `lumia_f64_softmax`).
#[no_mangle]
pub extern "C" fn lumia_f64_exp(x: f64) -> f64 {
    x.exp()
}

/// Scalar `sin(x)` (radians).
#[no_mangle]
pub extern "C" fn lumia_f64_sin(x: f64) -> f64 {
    x.sin()
}

/// Scalar `cos(x)` (radians).
#[no_mangle]
pub extern "C" fn lumia_f64_cos(x: f64) -> f64 {
    x.cos()
}

/// Scalar `atan2(y, x)`.
#[no_mangle]
pub extern "C" fn lumia_f64_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

/// Scalar `hypot(x, y)` = √(x²+y²).
#[no_mangle]
pub extern "C" fn lumia_f64_hypot(x: f64, y: f64) -> f64 {
    x.hypot(y)
}

/// `out[i] = a[i] * b[i]` (same length). Returns `out`.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_mul(out: *mut u8, a: *mut u8, b: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let a = force_f64(a);
    let b = force_f64(b);
    let out = ensure_unique_f64(out);
    let n = list_len_of(a);
    require_len(b, n, "mul b");
    require_len(out, n, "mul out");
    unsafe {
        let (op, _) = f64_elems_mut(out);
        let (ap, _) = f64_elems(a);
        let (bp, _) = f64_elems(b);
        crate::f64_simd::mul_f64(op, ap, bp, n as usize);
    }
    out
}

/// `out[i] = a[i] + b[i]` (same length). Returns `out`.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_add(out: *mut u8, a: *mut u8, b: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let a = force_f64(a);
    let b = force_f64(b);
    let out = ensure_unique_f64(out);
    let n = list_len_of(a);
    require_len(b, n, "add b");
    require_len(out, n, "add out");
    unsafe {
        let (op, _) = f64_elems_mut(out);
        let (ap, _) = f64_elems(a);
        let (bp, _) = f64_elems(b);
        crate::f64_simd::add_f64(op, ap, bp, n as usize);
    }
    out
}

/// Euclidean L2 norm.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_l2_norm(xs: *mut u8) -> f64 {
    let xs = force_f64(xs);
    unsafe {
        let (p, n) = f64_elems(xs);
        crate::f64_simd::dot_f64(p, p, n).sqrt()
    }
}

/// `∑ xᵢ²` (squared L2; used by soft-async skip checks).
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_sum_sq(xs: *mut u8) -> f64 {
    let xs = force_f64(xs);
    unsafe {
        let (p, n) = f64_elems(xs);
        crate::f64_simd::dot_f64(p, p, n)
    }
}

/// Arithmetic mean. Empty list → `0.0`.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_mean(xs: *mut u8) -> f64 {
    let xs = force_f64(xs);
    unsafe {
        let (p, n) = f64_elems(xs);
        if n == 0 {
            return 0.0;
        }
        let mut s = 0.0_f64;
        for i in 0..n {
            s += *p.add(i);
        }
        s / (n as f64)
    }
}

/// Population standard deviation (`torch.std(unbiased=False)`). Empty → `0.0`.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_std(xs: *mut u8) -> f64 {
    let xs = force_f64(xs);
    unsafe {
        let (p, n) = f64_elems(xs);
        if n == 0 {
            return 0.0;
        }
        let mut s = 0.0_f64;
        for i in 0..n {
            s += *p.add(i);
        }
        let mean = s / (n as f64);
        let mut var = 0.0_f64;
        for i in 0..n {
            let d = *p.add(i) - mean;
            var += d * d;
        }
        (var / (n as f64)).sqrt()
    }
}

/// In-place softmax (numerically stable). Returns `xs` (COW if shared).
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_softmax(xs: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let xs = ensure_unique_f64(xs);
    unsafe {
        let (p, n) = f64_elems_mut(xs);
        if n == 0 {
            return xs;
        }
        let mut m = *p;
        for i in 1..n {
            let v = *p.add(i);
            if v > m {
                m = v;
            }
        }
        let mut z = 0.0_f64;
        for i in 0..n {
            let e = (*p.add(i) - m).exp();
            *p.add(i) = e;
            z += e;
        }
        let inv = 1.0 / z;
        for i in 0..n {
            *p.add(i) *= inv;
        }
    }
    xs
}

/// In-place `x *= 1 / (‖x‖ + eps)`. Returns `x` (COW if shared).
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_l2_normalize(xs: *mut u8, eps: f64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let xs = ensure_unique_f64(xs);
    unsafe {
        let (p, n) = f64_elems_mut(xs);
        let s = crate::f64_simd::dot_f64(p, p, n);
        crate::f64_simd::scale_f64(p, n, 1.0 / (s.sqrt() + eps));
    }
    xs
}

/// Clamp every element into `[lo, hi]` (COW if shared).
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_clamp(xs: *mut u8, lo: f64, hi: f64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let xs = ensure_unique_f64(xs);
    unsafe {
        let (p, n) = f64_elems_mut(xs);
        crate::f64_simd::clamp_f64(p, n, lo, hi);
    }
    xs
}

/// `y = A @ x` with `A` row-major `m×n`. Writes `y` (len `m`). Returns `y`.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_gemv(m: i64, n: i64, a: *mut u8, x: *mut u8, y: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if m < 0 || n < 0 {
        trap_abort("lumia: gemv negative dims");
    }
    let a = force_f64(a);
    let x = force_f64(x);
    let y = ensure_unique_f64(y);
    require_len(a, m.saturating_mul(n), "gemv A");
    require_len(x, n, "gemv x");
    require_len(y, m, "gemv y");
    unsafe {
        let (ap, _) = f64_elems(a);
        let (xp, _) = f64_elems(x);
        let (yp, _) = f64_elems_mut(y);
        let m = m as usize;
        let n = n as usize;
        crate::f64_simd::matvec_f64(ap, xp, yp, m, n);
    }
    y
}

/// `y = Aᵀ @ x` with `A` row-major `m×n` (`x` len `m`, `y` len `n`). Returns `y`.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_gemv_t(m: i64, n: i64, a: *mut u8, x: *mut u8, y: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if m < 0 || n < 0 {
        trap_abort("lumia: gemv_t negative dims");
    }
    let a = force_f64(a);
    let x = force_f64(x);
    let y = ensure_unique_f64(y);
    require_len(a, m.saturating_mul(n), "gemv_t A");
    require_len(x, m, "gemv_t x");
    require_len(y, n, "gemv_t y");
    unsafe {
        let (ap, _) = f64_elems(a);
        let (xp, _) = f64_elems(x);
        let (yp, _) = f64_elems_mut(y);
        let m = m as usize;
        let n = n as usize;
        crate::f64_simd::zero_f64(yp, n);
        for i in 0..m {
            let xi = *xp.add(i);
            if xi != 0.0 {
                crate::f64_simd::axpy_scale_f64(yp, ap.add(i * n), xi, n);
            }
        }
    }
    y
}

/// `W += α · u ⊗ v` with `W` row-major `m×n`, `u` len `m`, `v` len `n`. Returns `W`.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_addmm(
    m: i64,
    n: i64,
    w: *mut u8,
    u: *mut u8,
    v: *mut u8,
    alpha: f64,
) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if m < 0 || n < 0 {
        trap_abort("lumia: addmm negative dims");
    }
    let u = force_f64(u);
    let v = force_f64(v);
    let w = ensure_unique_f64(w);
    require_len(w, m.saturating_mul(n), "addmm W");
    require_len(u, m, "addmm u");
    require_len(v, n, "addmm v");
    unsafe {
        let (wp, _) = f64_elems_mut(w);
        let (up, _) = f64_elems(u);
        let (vp, _) = f64_elems(v);
        let m = m as usize;
        let n = n as usize;
        for i in 0..m {
            let ui = *up.add(i) * alpha;
            if ui != 0.0 {
                crate::f64_simd::axpy_scale_f64(wp.add(i * n), vp, ui, n);
            }
        }
    }
    w
}

/// `y += α · x` (same length). Returns `y`.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_axpy(y: *mut u8, alpha: f64, x: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let x = force_f64(x);
    let y = ensure_unique_f64(y);
    let n = list_len_of(x);
    require_len(y, n, "axpy y");
    unsafe {
        let (yp, _) = f64_elems_mut(y);
        let (xp, _) = f64_elems(x);
        crate::f64_simd::axpy_scale_f64(yp, xp, alpha, n as usize);
    }
    y
}

/// `out = a - b` (same length). Returns `out`.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_sub(out: *mut u8, a: *mut u8, b: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let a = force_f64(a);
    let b = force_f64(b);
    let out = ensure_unique_f64(out);
    let n = list_len_of(a);
    require_len(b, n, "sub b");
    require_len(out, n, "sub out");
    unsafe {
        let (op, _) = f64_elems_mut(out);
        let (ap, _) = f64_elems(a);
        let (bp, _) = f64_elems(b);
        crate::f64_simd::sub_f64(op, ap, bp, n as usize);
    }
    out
}

/// `dst = src` (same length). Returns `dst`.
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_copy(dst: *mut u8, src: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let src = force_f64(src);
    let dst = ensure_unique_f64(dst);
    let n = list_len_of(src);
    require_len(dst, n, "copy dst");
    unsafe {
        let (dp, _) = f64_elems_mut(dst);
        let (sp, _) = f64_elems(src);
        ptr::copy_nonoverlapping(sp, dp, n as usize);
    }
    dst
}

/// Stable int fingerprint: `⌊Σ xᵢ · 1000⌋` (for e2e / oracle).
#[no_mangle]
pub unsafe extern "C" fn lumia_f64_checksum(xs: *mut u8) -> i64 {
    let xs = force_f64(xs);
    unsafe {
        let (p, n) = f64_elems(xs);
        let mut s = 0.0_f64;
        for i in 0..n {
            s += *p.add(i);
        }
        (s * 1000.0).floor() as i64
    }
}

fn force_f64(list: *mut u8) -> *mut u8 {
    let list = force_heap_list(list);
    if list.is_null() {
        trap_abort("lumia: dense f64 on null list");
    }
    if !list_float_elems(list) {
        trap_abort("lumia: dense f64 expects List[Float]");
    }
    list
}

fn ensure_unique_f64(list: *mut u8) -> *mut u8 {
    let list = force_f64(list);
    if list_rc_is_unique(list) {
        return list;
    }
    clone_f64_list(list)
}

fn clone_f64_list(list: *mut u8) -> *mut u8 {
    unsafe {
        let n = *(list as *const i64);
        let dest = lumia_alloc(list_payload_bytes(n), TYPE_LIST_F64);
        if dest.is_null() {
            trap_abort("lumia: dense f64 clone OOM");
        }
        ptr::copy_nonoverlapping(list as *const i64, dest as *mut i64, (n as usize) + 1);
        dest
    }
}

#[cfg(test)]
#[path = "dense_f64_tests.rs"]
mod tests;
