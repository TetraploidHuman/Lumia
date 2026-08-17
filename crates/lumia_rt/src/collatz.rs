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
    // Odds: one Syracuse hop often lands on an already-filled cell (skip stack).
    for n in 1..=lim {
        if n % 2 == 0 {
            let steps = cache[n / 2] + 1;
            cache[n] = steps;
            total += i64::from(steps);
        } else if n != 1 {
            total += collatz_odd_cached(n, &mut cache, lim, &mut stack);
        }
    }
    total
}

/// Odd `n > 1`: try `steps = 1 + cttz(3n+1) + cache[next]` before the general walker.
#[inline]
fn collatz_odd_cached(
    n: usize,
    cache: &mut [Step],
    lim: usize,
    stack: &mut Vec<(i64, i64)>,
) -> i64 {
    if cache_hit(cache, n, lim) {
        return i64::from(cache[n]);
    }
    let y = (n as u64).wrapping_mul(3).wrapping_add(1);
    let k = y.trailing_zeros();
    let nxt = (y >> k) as usize;
    if cache_hit(cache, nxt, lim) {
        let steps = i64::from(cache[nxt]) + 1 + i64::from(k);
        cache_set_with_doubles(cache, lim, n, steps as Step);
        return steps;
    }
    collatz_steps_cached(n as i64, cache, lim, stack)
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
        let s = if (n & 1) == 1 && n > 1 {
            collatz_odd_cached(n as usize, &mut cache, lim, &mut stack)
        } else {
            collatz_steps_cached(n, &mut cache, lim, &mut stack)
        };
        total = total.wrapping_add(s);
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
#[path = "collatz_tests.rs"]
mod tests;
