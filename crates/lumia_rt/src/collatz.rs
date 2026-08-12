//! Recognized Collatz accumulations with a dense step cache.
//!
//! Cache cells are `i16` (max Collatz steps in the bench window fit). `0` means
//! uncached for `n > 1`; `cache[1] = 0` is the valid step count for 1.

type Step = i16;

#[inline]
fn cache_hit(cache: &[Step], xu: usize, lim: usize) -> bool {
    xu <= lim && (xu == 1 || cache[xu] > 0)
}

/// Sum of Collatz step counts for `n = 1..=limit`.
#[no_mangle]
pub extern "C" fn lumia_collatz_total(limit: i64) -> i64 {
    if limit < 1 {
        return 0;
    }
    let lim = limit as usize;
    // Zero-init: uncached sentinel for n>1; half the traffic of `-1` fills.
    let mut cache = vec![0 as Step; lim + 1];
    let mut stack = Vec::with_capacity(64);
    let mut total: i64 = 0;
    // Sequential: every even `n` has `n/2` already solved ⇒ O(1) write.
    // No software prefetch: HW streamer already covers this scan; SW hints
    // were neutral-to-slower on dense RMW and only ~3% on this kernel.
    for n in 1..=lim {
        if n % 2 == 0 {
            let steps = cache[n / 2] + 1;
            cache[n] = steps;
            total += i64::from(steps);
        } else if n != 1 {
            total += collatz_steps_cached(n as i64, &mut cache, lim, &mut stack);
        }
    }
    total
}

/// Sum of Collatz step counts for `n = start, start+stride, …` while `n ≤ limit`.
#[no_mangle]
pub extern "C" fn lumia_collatz_strided(start: i64, limit: i64, stride: i64) -> i64 {
    if stride == 1 && start <= 1 {
        return lumia_collatz_total(limit);
    }
    if limit < 1 || stride < 1 || start > limit {
        return 0;
    }
    let start = start.max(1);
    let lim = limit as usize;
    let mut cache = vec![0 as Step; lim + 1];
    let mut stack = Vec::with_capacity(64);
    let mut total: i64 = 0;
    let mut n = start;
    while n <= limit {
        total = total.wrapping_add(collatz_steps_cached(n, &mut cache, lim, &mut stack));
        n = n.saturating_add(stride);
    }
    total
}

/// `(value, edge)` — `edge` is the number of Collatz steps taken **after** leaving
/// `value` (via one apply) while the hailstone was **above** `lim`, before the next
/// stacked value or the cached sink.
fn collatz_steps_cached(
    start: i64,
    cache: &mut [Step],
    lim: usize,
    stack: &mut Vec<(i64, i64)>,
) -> i64 {
    let su = start as usize;
    if cache_hit(cache, su, lim) {
        return if start <= 1 { 0 } else { i64::from(cache[su]) };
    }
    stack.clear();
    let mut x = start;
    let mut edge = 0i64;
    while x > 1 {
        let xu = x as usize;
        if cache_hit(cache, xu, lim) {
            break;
        }
        if xu <= lim && x > 0 {
            if let Some(last) = stack.last_mut() {
                last.1 += edge;
                edge = 0;
            }
            stack.push((x, 0));
            if x & 1 == 0 {
                x >>= 1;
            } else {
                // Syracuse: `3x+1` then strip twos; halvings count on this frame's edge.
                let y = x.wrapping_mul(3).wrapping_add(1);
                let k = y.trailing_zeros() as i64;
                if let Some(last) = stack.last_mut() {
                    last.1 += k;
                }
                x = y >> k;
            }
        } else {
            // Above the memo window: Syracuse-fuse odd with trailing even run.
            if x & 1 == 0 {
                let k = x.trailing_zeros() as i64;
                edge += k;
                x >>= k;
            } else {
                let y = x.wrapping_mul(3).wrapping_add(1);
                let k = y.trailing_zeros() as i64;
                edge += 1 + k;
                x = y >> k;
            }
        }
    }
    let mut steps: i64 = if x <= 1 {
        0
    } else {
        i64::from(cache[x as usize])
    };
    steps += edge;
    while let Some((v, e)) = stack.pop() {
        steps += 1 + e;
        let vu = v as usize;
        if vu <= lim && !cache_hit(cache, vu, lim) {
            cache_set_with_doubles(cache, lim, vu, steps as Step);
        }
    }
    steps
}

/// Record `cache[v] = steps` and fill `2v, 4v, …` while still inside `lim`.
fn cache_set_with_doubles(cache: &mut [Step], lim: usize, v: usize, steps: Step) {
    cache[v] = steps;
    let mut cur = v;
    let mut s = steps;
    while let Some(nxt) = cur.checked_mul(2) {
        if nxt > lim {
            break;
        }
        s += 1;
        if cache_hit(cache, nxt, lim) {
            break;
        }
        cache[nxt] = s;
        cur = nxt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_cpu_collatz_checksum() {
        assert_eq!(lumia_collatz_total(250_000), 29_265_567);
    }

    #[test]
    fn bench_cpu_collatz_2_5m() {
        assert_eq!(lumia_collatz_total(2_500_000), 352_279_148);
    }

    #[test]
    fn bench_cpu_collatz_strided() {
        assert_eq!(lumia_collatz_strided(1, 3_000_000, 3), 142_794_532);
    }

    #[test]
    fn strided_1_matches_total() {
        assert_eq!(
            lumia_collatz_strided(1, 10_000, 1),
            lumia_collatz_total(10_000)
        );
    }

    #[test]
    fn collatz_edges_and_small_oracle() {
        assert_eq!(lumia_collatz_total(0), 0);
        assert_eq!(lumia_collatz_total(-5), 0);
        assert_eq!(lumia_collatz_strided(1, 0, 3), 0);
        assert_eq!(lumia_collatz_strided(10, 5, 1), 0);

        fn steps(mut n: i64) -> i64 {
            let mut s = 0i64;
            while n > 1 {
                if n % 2 == 0 {
                    n /= 2;
                } else {
                    n = 3 * n + 1;
                }
                s += 1;
            }
            s
        }
        fn naive_total(limit: i64) -> i64 {
            (1..=limit).map(steps).sum()
        }
        fn naive_strided(start: i64, limit: i64, stride: i64) -> i64 {
            let mut n = start;
            let mut t = 0i64;
            while n <= limit {
                t += steps(n);
                n += stride;
            }
            t
        }
        for limit in [1i64, 2, 3, 10, 100, 1_000, 5_000, 20_000] {
            assert_eq!(
                lumia_collatz_total(limit),
                naive_total(limit),
                "total {limit}"
            );
        }
        for &(start, limit, stride) in &[
            (1i64, 1_000, 2),
            (1, 5_000, 3),
            (7, 2_000, 5),
            (1, 10_000, 7),
        ] {
            assert_eq!(
                lumia_collatz_strided(start, limit, stride),
                naive_strided(start, limit, stride),
                "strided {start}/{limit}/{stride}"
            );
        }
    }
}
