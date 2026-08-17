//! Portable i64 helpers with optional AVX2 fast paths (dense List[Int] scans).

#[inline(always)]
fn simd_i64() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        return is_x86_feature_detected!("avx2");
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Sum `a[0..n]` as wrapping i64.
#[inline(always)]
pub(crate) fn sum_i64(a: *const i64, n: usize) -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        if simd_i64() {
            return unsafe { sum_avx2(a, n) };
        }
    }
    unsafe { sum_scalar(a, n) }
}

/// Fill `out[i] = start + i` for `i in 0..n` (wrapping).
#[inline(always)]
pub(crate) fn fill_iota_i64(out: *mut i64, n: usize, start: i64) {
    #[cfg(target_arch = "x86_64")]
    {
        if simd_i64() {
            unsafe { fill_iota_avx2(out, n, start) };
            return;
        }
    }
    unsafe {
        for i in 0..n {
            *out.add(i) = start.wrapping_add(i as i64);
        }
    }
}

#[inline(always)]
unsafe fn sum_scalar(a: *const i64, n: usize) -> i64 {
    let mut s = 0i64;
    for i in 0..n {
        s = s.wrapping_add(*a.add(i));
    }
    s
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn sum_avx2(a: *const i64, n: usize) -> i64 {
    use std::arch::x86_64::*;
    let mut acc = _mm256_setzero_si256();
    let mut i = 0usize;
    while i + 4 <= n {
        let v = _mm256_loadu_si256(a.add(i) as *const __m256i);
        acc = _mm256_add_epi64(acc, v);
        i += 4;
    }
    let mut lanes = [0i64; 4];
    _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, acc);
    let mut s = lanes[0]
        .wrapping_add(lanes[1])
        .wrapping_add(lanes[2])
        .wrapping_add(lanes[3]);
    while i < n {
        s = s.wrapping_add(*a.add(i));
        i += 1;
    }
    s
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn fill_iota_avx2(out: *mut i64, n: usize, start: i64) {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let offs = _mm256_set_epi64x(3, 2, 1, 0);
    while i + 4 <= n {
        let base = _mm256_set1_epi64x(start.wrapping_add(i as i64));
        let v = _mm256_add_epi64(base, offs);
        _mm256_storeu_si256(out.add(i) as *mut __m256i, v);
        i += 4;
    }
    while i < n {
        *out.add(i) = start.wrapping_add(i as i64);
        i += 1;
    }
}

#[cfg(test)]
#[path = "i64_simd_tests.rs"]
mod tests;
