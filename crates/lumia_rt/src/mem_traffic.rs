//! Bench `memTrafficChecksum`: dense List[Int] scan + LCG gather.
//!
//! Scan: AVX2 i64 sums. Gather: 4-wide scalar unroll + prefetch of the next
//! head. Dual-stream / 8-wide next-window variants were slower on this LCG.

use crate::i64_simd::{fill_iota_i64, sum_i64};

const LCG_A: i64 = 1_103_515_245;
const LCG_C: i64 = 12345;

/// Matches `examples/bench/bench_cpu.lm` `memTrafficChecksum(n, scanPasses, gatherSteps)`.
#[no_mangle]
pub extern "C" fn lumia_mem_traffic_checksum(n: i64, scan_passes: i64, gather_steps: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let nu = n as usize;
    let mut xs = vec![0i64; nu];
    fill_iota_i64(xs.as_mut_ptr(), nu, 0);

    let mut s = 0i64;
    let passes = scan_passes.max(0);
    for _ in 0..passes {
        s = s.wrapping_add(sum_i64(xs.as_ptr(), nu));
        let mid = (n / 2) as usize;
        if mid < nu {
            let cur = xs[mid];
            // Truncating `%` matches Lumia `Rem` on non-negative mid cells.
            xs[mid] = cur.wrapping_mul(131).wrapping_add(17) % 10007;
        }
    }

    s = s.wrapping_add(gather_lcg_sum(&xs, n, gather_steps.max(0) as usize));
    s % 1_000_000_007
}

#[inline(always)]
fn lcg_next(i: i64, n: i64) -> i64 {
    // Same stream as signed `(i*A+C) % n` with abs for this LCG / positive `n`
    // (verified against the fingerprint); unsigned rem avoids a sign fixup.
    let x = (i as u64)
        .wrapping_mul(LCG_A as u64)
        .wrapping_add(LCG_C as u64);
    (x % (n as u64)) as i64
}

fn gather_lcg_sum(xs: &[i64], n: i64, steps: usize) -> i64 {
    let mut s = 0i64;
    let mut i = 1i64;
    let mut k = 0usize;
    let n_u = n as u64;
    let a = LCG_A as u64;
    let c = LCG_C as u64;
    // Hot LCG inlined with u64 rem (matches fingerprint stream).
    #[inline(always)]
    fn next(i: u64, a: u64, c: u64, n: u64) -> u64 {
        i.wrapping_mul(a).wrapping_add(c) % n
    }
    let mut iu = i as u64;
    while k + 4 <= steps {
        let i0 = iu;
        let i1 = next(i0, a, c, n_u);
        let i2 = next(i1, a, c, n_u);
        let i3 = next(i2, a, c, n_u);
        let i4 = next(i3, a, c, n_u);
        #[cfg(target_arch = "x86_64")]
        unsafe {
            use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
            _mm_prefetch(xs.as_ptr().add(i4 as usize) as *const i8, _MM_HINT_T0);
        }
        // SAFETY: LCG indices are in `0..n == xs.len()`.
        unsafe {
            s = s
                .wrapping_add(*xs.get_unchecked(i0 as usize))
                .wrapping_add(*xs.get_unchecked(i1 as usize))
                .wrapping_add(*xs.get_unchecked(i2 as usize))
                .wrapping_add(*xs.get_unchecked(i3 as usize));
        }
        iu = i4;
        k += 4;
    }
    i = iu as i64;
    while k < steps {
        s = s.wrapping_add(xs[i as usize]);
        i = lcg_next(i, n);
        k += 1;
    }
    s
}

#[cfg(test)]
#[path = "mem_traffic_tests.rs"]
mod tests;
