//! Bench `memTrafficChecksum`: dense List[Int] scan + LCG gather.
//!
//! Scan: AVX2 i64 sums. Gather: 4-wide scalar unroll + prefetch of the next
//! head. Dual-stream / 8-wide next-window variants were slower on this LCG.

use crate::i64_simd::{fill_iota_i64, sum_i64};

const LCG_A: i64 = 1_103_515_245;
const LCG_C: i64 = 12345;

/// Matches `examples/bench_cpu.lm` `memTrafficChecksum(n, scanPasses, gatherSteps)`.
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
    let mut j = i.wrapping_mul(LCG_A).wrapping_add(LCG_C) % n;
    if j < 0 {
        j = -j;
    }
    j
}

fn gather_lcg_sum(xs: &[i64], n: i64, steps: usize) -> i64 {
    let mut s = 0i64;
    let mut i = 1i64;
    let mut k = 0usize;
    while k + 4 <= steps {
        let i0 = i;
        let i1 = lcg_next(i0, n);
        let i2 = lcg_next(i1, n);
        let i3 = lcg_next(i2, n);
        let i4 = lcg_next(i3, n);
        #[cfg(target_arch = "x86_64")]
        unsafe {
            use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
            _mm_prefetch(xs.as_ptr().add(i4 as usize) as *const i8, _MM_HINT_T0);
        }
        s = s
            .wrapping_add(xs[i0 as usize])
            .wrapping_add(xs[i1 as usize])
            .wrapping_add(xs[i2 as usize])
            .wrapping_add(xs[i3 as usize]);
        i = i4;
        k += 4;
    }
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
