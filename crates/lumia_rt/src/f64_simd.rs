//! Portable f64 helpers with optional AVX2+FMA fast paths (CN / dense kernels).
//!
//! On x86_64 with AVX2+FMA, `dot` / `axpy_scale` / `hebbian_row` / elementwise
//! ops use 8-wide (2×YMM) FMA chunks plus a 4-wide tail. Association can differ
//! slightly from pure scalar.

#![allow(dead_code)]

#[inline(always)]
fn simd_f64() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        use std::sync::atomic::{AtomicU8, Ordering};
        static CACHED: AtomicU8 = AtomicU8::new(0); // 0 unknown, 1 no, 2 yes
        let v = CACHED.load(Ordering::Relaxed);
        if v != 0 {
            return v == 2;
        }
        let yes = is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma");
        CACHED.store(if yes { 2 } else { 1 }, Ordering::Relaxed);
        yes
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

#[inline(always)]
pub(crate) fn dot_f64(a: *const f64, b: *const f64, n: usize) -> f64 {
    #[cfg(target_arch = "x86_64")]
    {
        if simd_f64() {
            return unsafe { dot_avx2(a, b, n) };
        }
    }
    unsafe { dot_scalar(a, b, n) }
}

/// `y[j] += scale * x[j]` for `j in 0..n`.
#[inline(always)]
pub(crate) fn axpy_scale_f64(y: *mut f64, x: *const f64, scale: f64, n: usize) {
    if scale == 0.0 {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        if simd_f64() {
            unsafe { axpy_scale_avx2(y, x, scale, n) };
            return;
        }
    }
    unsafe { axpy_scale_scalar(y, x, scale, n) }
}

/// `y[j] = clamp(y[j] + scale * x[j], lo, hi)`.
#[inline(always)]
pub(crate) fn axpy_clamp_f64(y: *mut f64, x: *const f64, scale: f64, n: usize, lo: f64, hi: f64) {
    #[cfg(target_arch = "x86_64")]
    {
        if simd_f64() {
            unsafe { axpy_clamp_avx2(y, x, scale, n, lo, hi) };
            return;
        }
    }
    unsafe {
        for j in 0..n {
            *y.add(j) = (*y.add(j) + scale * *x.add(j)).clamp(lo, hi);
        }
    }
}

/// Zero `y[0..n]` (`0.0` is all-zero bits).
#[inline(always)]
pub(crate) fn zero_f64(y: *mut f64, n: usize) {
    unsafe {
        std::ptr::write_bytes(y as *mut u8, 0, n * std::mem::size_of::<f64>());
    }
}

/// Fill `y[0..n]` with `v`.
#[inline(always)]
pub(crate) fn fill_f64(y: *mut f64, n: usize, v: f64) {
    if v == 0.0 {
        zero_f64(y, n);
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        if simd_f64() {
            unsafe { fill_avx2(y, n, v) };
            return;
        }
    }
    unsafe {
        for j in 0..n {
            *y.add(j) = v;
        }
    }
}

/// `y[j] *= alpha`.
#[inline(always)]
pub(crate) fn scale_f64(y: *mut f64, n: usize, alpha: f64) {
    if alpha == 1.0 {
        return;
    }
    if alpha == 0.0 {
        zero_f64(y, n);
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        if simd_f64() {
            unsafe { scale_avx2(y, n, alpha) };
            return;
        }
    }
    unsafe {
        for j in 0..n {
            *y.add(j) *= alpha;
        }
    }
}

/// `out[j] = a[j] + b[j]`.
#[inline(always)]
pub(crate) fn add_f64(out: *mut f64, a: *const f64, b: *const f64, n: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        if simd_f64() {
            unsafe { binop_avx2(out, a, b, n, BinOp::Add) };
            return;
        }
    }
    unsafe {
        for j in 0..n {
            *out.add(j) = *a.add(j) + *b.add(j);
        }
    }
}

/// `out[j] = a[j] - b[j]`.
#[inline(always)]
pub(crate) fn sub_f64(out: *mut f64, a: *const f64, b: *const f64, n: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        if simd_f64() {
            unsafe { binop_avx2(out, a, b, n, BinOp::Sub) };
            return;
        }
    }
    unsafe {
        for j in 0..n {
            *out.add(j) = *a.add(j) - *b.add(j);
        }
    }
}

/// `out[j] = a[j] * b[j]`.
#[inline(always)]
pub(crate) fn mul_f64(out: *mut f64, a: *const f64, b: *const f64, n: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        if simd_f64() {
            unsafe { binop_avx2(out, a, b, n, BinOp::Mul) };
            return;
        }
    }
    unsafe {
        for j in 0..n {
            *out.add(j) = *a.add(j) * *b.add(j);
        }
    }
}

/// Clamp each `y[j]` into `[lo, hi]`.
#[inline(always)]
pub(crate) fn clamp_f64(y: *mut f64, n: usize, lo: f64, hi: f64) {
    #[cfg(target_arch = "x86_64")]
    {
        if simd_f64() {
            unsafe { clamp_avx2(y, n, lo, hi) };
            return;
        }
    }
    unsafe {
        for j in 0..n {
            *y.add(j) = (*y.add(j)).clamp(lo, hi);
        }
    }
}

/// Hebbian row: `w[j] = clamp(w[j]*keep + ui * v[j], lo, hi) * mask[j]`.
#[inline(always)]
pub(crate) fn hebbian_row_f64(
    w: *mut f64,
    v: *const f64,
    mask: *const f64,
    n: usize,
    ui: f64,
    keep: f64,
    lo: f64,
    hi: f64,
) {
    #[cfg(target_arch = "x86_64")]
    {
        if simd_f64() {
            unsafe { hebbian_row_avx2(w, v, mask, n, ui, keep, lo, hi) };
            return;
        }
    }
    unsafe { hebbian_row_scalar(w, v, mask, n, ui, keep, lo, hi) }
}

/// `y = clamp(x @ W, lo, hi)` with `W` row-major `m×n`, `x` len `m`, `y` len `n`.
#[inline(always)]
pub(crate) unsafe fn project_clamp_f64(
    w: *const f64,
    x: *const f64,
    y: *mut f64,
    m: usize,
    n: usize,
    lo: f64,
    hi: f64,
) {
    zero_f64(y, n);
    for i in 0..m {
        let xi = *x.add(i);
        if xi != 0.0 {
            axpy_scale_f64(y, w.add(i * n), xi, n);
        }
    }
    clamp_f64(y, n, lo, hi);
}

/// `y[i] = clamp(row_i · x, lo, hi)` with `W` row-major `m×n`, `x` len `n`.
#[inline(always)]
pub(crate) unsafe fn matvec_clamp_f64(
    w: *const f64,
    x: *const f64,
    y: *mut f64,
    m: usize,
    n: usize,
    lo: f64,
    hi: f64,
) {
    for i in 0..m {
        let s = dot_f64(w.add(i * n), x, n);
        *y.add(i) = s.clamp(lo, hi);
    }
}

/// `y[i] = row_i · x` (no clamp).
#[inline(always)]
pub(crate) unsafe fn matvec_f64(w: *const f64, x: *const f64, y: *mut f64, m: usize, n: usize) {
    for i in 0..m {
        *y.add(i) = dot_f64(w.add(i * n), x, n);
    }
}

unsafe fn dot_scalar(a: *const f64, b: *const f64, n: usize) -> f64 {
    let mut s = 0.0_f64;
    for j in 0..n {
        s += *a.add(j) * *b.add(j);
    }
    s
}

unsafe fn axpy_scale_scalar(y: *mut f64, x: *const f64, scale: f64, n: usize) {
    for j in 0..n {
        *y.add(j) += scale * *x.add(j);
    }
}

unsafe fn hebbian_row_scalar(
    w: *mut f64,
    v: *const f64,
    mask: *const f64,
    n: usize,
    ui: f64,
    keep: f64,
    lo: f64,
    hi: f64,
) {
    for j in 0..n {
        let mut zij = *w.add(j) * keep + ui * *v.add(j);
        zij = zij.clamp(lo, hi);
        *w.add(j) = zij * *mask.add(j);
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn hsum256(acc: std::arch::x86_64::__m256d) -> f64 {
    use std::arch::x86_64::*;
    let hi = _mm256_extractf128_pd(acc, 1);
    let lo = _mm256_castpd256_pd128(acc);
    let sum2 = _mm_add_pd(lo, hi);
    let shuf = _mm_unpackhi_pd(sum2, sum2);
    _mm_cvtsd_f64(_mm_add_sd(sum2, shuf))
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn dot_avx2(a: *const f64, b: *const f64, n: usize) -> f64 {
    use std::arch::x86_64::*;
    let mut acc0 = _mm256_setzero_pd();
    let mut acc1 = _mm256_setzero_pd();
    let mut j = 0usize;
    while j + 8 <= n {
        let va0 = _mm256_loadu_pd(a.add(j));
        let vb0 = _mm256_loadu_pd(b.add(j));
        let va1 = _mm256_loadu_pd(a.add(j + 4));
        let vb1 = _mm256_loadu_pd(b.add(j + 4));
        acc0 = _mm256_fmadd_pd(va0, vb0, acc0);
        acc1 = _mm256_fmadd_pd(va1, vb1, acc1);
        j += 8;
    }
    while j + 4 <= n {
        let va = _mm256_loadu_pd(a.add(j));
        let vb = _mm256_loadu_pd(b.add(j));
        acc0 = _mm256_fmadd_pd(va, vb, acc0);
        j += 4;
    }
    let mut s = hsum256(_mm256_add_pd(acc0, acc1));
    while j < n {
        s += *a.add(j) * *b.add(j);
        j += 1;
    }
    s
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn axpy_scale_avx2(y: *mut f64, x: *const f64, scale: f64, n: usize) {
    use std::arch::x86_64::*;
    let vs = _mm256_set1_pd(scale);
    let mut j = 0usize;
    while j + 8 <= n {
        let vy0 = _mm256_loadu_pd(y.add(j));
        let vx0 = _mm256_loadu_pd(x.add(j));
        let vy1 = _mm256_loadu_pd(y.add(j + 4));
        let vx1 = _mm256_loadu_pd(x.add(j + 4));
        _mm256_storeu_pd(y.add(j), _mm256_fmadd_pd(vs, vx0, vy0));
        _mm256_storeu_pd(y.add(j + 4), _mm256_fmadd_pd(vs, vx1, vy1));
        j += 8;
    }
    while j + 4 <= n {
        let vy = _mm256_loadu_pd(y.add(j));
        let vx = _mm256_loadu_pd(x.add(j));
        _mm256_storeu_pd(y.add(j), _mm256_fmadd_pd(vs, vx, vy));
        j += 4;
    }
    while j < n {
        *y.add(j) += scale * *x.add(j);
        j += 1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn axpy_clamp_avx2(y: *mut f64, x: *const f64, scale: f64, n: usize, lo: f64, hi: f64) {
    use std::arch::x86_64::*;
    let vs = _mm256_set1_pd(scale);
    let vlo = _mm256_set1_pd(lo);
    let vhi = _mm256_set1_pd(hi);
    let mut j = 0usize;
    while j + 4 <= n {
        let r = _mm256_fmadd_pd(vs, _mm256_loadu_pd(x.add(j)), _mm256_loadu_pd(y.add(j)));
        _mm256_storeu_pd(y.add(j), _mm256_min_pd(_mm256_max_pd(r, vlo), vhi));
        j += 4;
    }
    while j < n {
        *y.add(j) = (*y.add(j) + scale * *x.add(j)).clamp(lo, hi);
        j += 1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn fill_avx2(y: *mut f64, n: usize, v: f64) {
    use std::arch::x86_64::*;
    let vv = _mm256_set1_pd(v);
    let mut j = 0usize;
    while j + 8 <= n {
        _mm256_storeu_pd(y.add(j), vv);
        _mm256_storeu_pd(y.add(j + 4), vv);
        j += 8;
    }
    while j + 4 <= n {
        _mm256_storeu_pd(y.add(j), vv);
        j += 4;
    }
    while j < n {
        *y.add(j) = v;
        j += 1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn scale_avx2(y: *mut f64, n: usize, alpha: f64) {
    use std::arch::x86_64::*;
    let va = _mm256_set1_pd(alpha);
    let mut j = 0usize;
    while j + 8 <= n {
        _mm256_storeu_pd(y.add(j), _mm256_mul_pd(_mm256_loadu_pd(y.add(j)), va));
        _mm256_storeu_pd(y.add(j + 4), _mm256_mul_pd(_mm256_loadu_pd(y.add(j + 4)), va));
        j += 8;
    }
    while j + 4 <= n {
        _mm256_storeu_pd(y.add(j), _mm256_mul_pd(_mm256_loadu_pd(y.add(j)), va));
        j += 4;
    }
    while j < n {
        *y.add(j) *= alpha;
        j += 1;
    }
}

#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy)]
enum BinOp {
    Add,
    Sub,
    Mul,
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn binop_avx2(out: *mut f64, a: *const f64, b: *const f64, n: usize, op: BinOp) {
    use std::arch::x86_64::*;
    let mut j = 0usize;
    while j + 4 <= n {
        let va = _mm256_loadu_pd(a.add(j));
        let vb = _mm256_loadu_pd(b.add(j));
        let r = match op {
            BinOp::Add => _mm256_add_pd(va, vb),
            BinOp::Sub => _mm256_sub_pd(va, vb),
            BinOp::Mul => _mm256_mul_pd(va, vb),
        };
        _mm256_storeu_pd(out.add(j), r);
        j += 4;
    }
    while j < n {
        *out.add(j) = match op {
            BinOp::Add => *a.add(j) + *b.add(j),
            BinOp::Sub => *a.add(j) - *b.add(j),
            BinOp::Mul => *a.add(j) * *b.add(j),
        };
        j += 1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn clamp_avx2(y: *mut f64, n: usize, lo: f64, hi: f64) {
    use std::arch::x86_64::*;
    let vlo = _mm256_set1_pd(lo);
    let vhi = _mm256_set1_pd(hi);
    let mut j = 0usize;
    while j + 8 <= n {
        let z0 = _mm256_min_pd(_mm256_max_pd(_mm256_loadu_pd(y.add(j)), vlo), vhi);
        let z1 = _mm256_min_pd(_mm256_max_pd(_mm256_loadu_pd(y.add(j + 4)), vlo), vhi);
        _mm256_storeu_pd(y.add(j), z0);
        _mm256_storeu_pd(y.add(j + 4), z1);
        j += 8;
    }
    while j + 4 <= n {
        let z = _mm256_min_pd(_mm256_max_pd(_mm256_loadu_pd(y.add(j)), vlo), vhi);
        _mm256_storeu_pd(y.add(j), z);
        j += 4;
    }
    while j < n {
        *y.add(j) = (*y.add(j)).clamp(lo, hi);
        j += 1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn hebbian_row_avx2(
    w: *mut f64,
    v: *const f64,
    mask: *const f64,
    n: usize,
    ui: f64,
    keep: f64,
    lo: f64,
    hi: f64,
) {
    use std::arch::x86_64::*;
    let vui = _mm256_set1_pd(ui);
    let vkeep = _mm256_set1_pd(keep);
    let vlo = _mm256_set1_pd(lo);
    let vhi = _mm256_set1_pd(hi);
    let mut j = 0usize;
    while j + 4 <= n {
        let vw = _mm256_loadu_pd(w.add(j));
        let vv = _mm256_loadu_pd(v.add(j));
        let vm = _mm256_loadu_pd(mask.add(j));
        let mut z = _mm256_fmadd_pd(vw, vkeep, _mm256_mul_pd(vui, vv));
        z = _mm256_min_pd(z, vhi);
        z = _mm256_max_pd(z, vlo);
        z = _mm256_mul_pd(z, vm);
        _mm256_storeu_pd(w.add(j), z);
        j += 4;
    }
    while j < n {
        let mut zij = *w.add(j) * keep + ui * *v.add(j);
        zij = zij.clamp(lo, hi);
        *w.add(j) = zij * *mask.add(j);
        j += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_and_axpy_match_scalar() {
        let a = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let b = [0.5; 9];
        let d = dot_f64(a.as_ptr(), b.as_ptr(), 9);
        assert!((d - 22.5).abs() < 1e-9, "d={d}");
        let mut y = [0.0; 9];
        let x = [1.0; 9];
        axpy_scale_f64(y.as_mut_ptr(), x.as_ptr(), 2.0, 9);
        assert!(y.iter().all(|&v| (v - 2.0).abs() < 1e-12));
    }

    #[test]
    fn elementwise_and_clamp() {
        let a = [1.0, 2.0, 3.0, 4.0, 5.0];
        let b = [5.0, 4.0, 3.0, 2.0, 1.0];
        let mut out = [0.0; 5];
        add_f64(out.as_mut_ptr(), a.as_ptr(), b.as_ptr(), 5);
        assert!(out.iter().all(|&v| (v - 6.0).abs() < 1e-12));
        sub_f64(out.as_mut_ptr(), a.as_ptr(), b.as_ptr(), 5);
        assert!((out[0] + 4.0).abs() < 1e-12);
        fill_f64(out.as_mut_ptr(), 5, 9.0);
        assert!(out.iter().all(|&v| v == 9.0));
        scale_f64(out.as_mut_ptr(), 5, 0.5);
        assert!(out.iter().all(|&v| (v - 4.5).abs() < 1e-12));
        clamp_f64(out.as_mut_ptr(), 5, 0.0, 1.0);
        assert!(out.iter().all(|&v| (v - 1.0).abs() < 1e-12));
        let mut y = [1.0; 5];
        axpy_clamp_f64(y.as_mut_ptr(), a.as_ptr(), 1.0, 5, 0.0, 3.0);
        assert_eq!(y, [2.0, 3.0, 3.0, 3.0, 3.0]);
    }
}
