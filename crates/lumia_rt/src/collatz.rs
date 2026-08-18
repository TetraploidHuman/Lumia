//! Recognized Collatz accumulations with a dense step cache.
//!
//! Cache cells are `i16` (max Collatz steps in the bench window fit). `0` means
//! uncached for `n > 1`; `cache[1] = 0` is the valid step count for 1.
//!
//! Sequential [`lumia_collatz_total`] skips `2v,4v,…` fan-out on Syracuse hits:
//! the even scan later writes those cells as `cache[n/2]+1`. Strided paths keep
//! the fan-out (sparse IV). Sequential odds short-circuit when `nxt < n`
//! (prefix already filled) before the shared odd helper.

type Step = i16;

#[inline(always)]
unsafe fn cache_get(cache: &[Step], xu: usize) -> Step {
    *cache.get_unchecked(xu)
}

#[inline(always)]
unsafe fn cache_set(cache: &mut [Step], xu: usize, s: Step) {
    *cache.get_unchecked_mut(xu) = s;
}

#[inline(always)]
fn cache_hit(cache: &[Step], xu: usize, lim: usize) -> bool {
    // SAFETY: callers only pass `xu <= lim` or we gate on that first.
    xu <= lim && (xu == 1 || unsafe { cache_get(cache, xu) } > 0)
}

/// Sum of Collatz step counts for `n = 1..=limit`.
#[no_mangle]
pub extern "C" fn lumia_collatz_total(limit: i64) -> i64 {
    if limit < 1 {
        return 0;
    }
    let lim = limit as usize;
    let mut cache = vec![0 as Step; lim + 1];
    let mut stack = Vec::with_capacity(64);
    let mut total: i64 = 0;
    // Sequential: every even `n` has `n/2` already solved ⇒ O(1) write.
    // Odds: one Syracuse hop often lands on an already-filled cell (skip stack).
    for n in 1..=lim {
        if n & 1 == 0 {
            // SAFETY: `n/2 < n <= lim`.
            let steps = unsafe { cache_get(&cache, n >> 1) } + 1;
            unsafe { cache_set(&mut cache, n, steps) };
            total += i64::from(steps);
        } else if n != 1 {
            total += collatz_odd_sequential(n, &mut cache, lim, &mut stack);
        }
    }
    total
}

/// Sequential odd `n > 1`: `nxt < n` ⇒ unconditional hit (prefix filled).
/// On miss, continue from the already-computed first Syracuse hop (no redo).
#[inline]
fn collatz_odd_sequential(
    n: usize,
    cache: &mut [Step],
    lim: usize,
    stack: &mut Vec<(i64, i64)>,
) -> i64 {
    let y = (n as u64).wrapping_mul(3).wrapping_add(1);
    let k = y.trailing_zeros();
    let nxt = (y >> k) as usize;
    if nxt < n {
        // SAFETY: `1 <= nxt < n <= lim`; all cells `< n` are filled.
        let steps = i64::from(unsafe { cache_get(cache, nxt) }) + 1 + i64::from(k);
        unsafe { cache_set(cache, n, steps as Step) };
        return steps;
    }
    // First hop missed the filled prefix — try a second hop, else stack-walk.
    if nxt > 1 && nxt <= lim {
        let y2 = (nxt as u64).wrapping_mul(3).wrapping_add(1);
        let k2 = y2.trailing_zeros();
        let nxt2 = (y2 >> k2) as usize;
        if nxt2 < n || cache_hit(cache, nxt2, lim) {
            let steps = i64::from(unsafe { cache_get(cache, nxt2) })
                + 1
                + i64::from(k2)
                + 1
                + i64::from(k);
            unsafe {
                cache_set(cache, n, steps as Step);
                if !cache_hit(cache, nxt, lim) {
                    let mid = i64::from(cache_get(cache, nxt2)) + 1 + i64::from(k2);
                    cache_set(cache, nxt, mid as Step);
                }
            }
            return steps;
        }
    }
    collatz_steps_cached(n as i64, cache, lim, stack, false)
}

/// Odd `n > 1`: try `steps = 1 + cttz(3n+1) + cache[next]` before the general walker.
///
/// `fill_doubles`: when true, also memoize `2n,4n,…` (helps sparse/strided IV).
#[inline]
fn collatz_odd_cached(
    n: usize,
    cache: &mut [Step],
    lim: usize,
    stack: &mut Vec<(i64, i64)>,
    fill_doubles: bool,
) -> i64 {
    if cache_hit(cache, n, lim) {
        return i64::from(unsafe { cache_get(cache, n) });
    }
    let y = (n as u64).wrapping_mul(3).wrapping_add(1);
    let k = y.trailing_zeros();
    let nxt = (y >> k) as usize;
    if cache_hit(cache, nxt, lim) {
        let steps = i64::from(unsafe { cache_get(cache, nxt) }) + 1 + i64::from(k);
        if fill_doubles {
            cache_set_with_doubles(cache, lim, n, steps as Step);
        } else {
            unsafe { cache_set(cache, n, steps as Step) };
        }
        return steps;
    }
    // Second fused hop before falling back to the stack walker.
    if nxt > 1 && nxt <= lim {
        let y2 = (nxt as u64).wrapping_mul(3).wrapping_add(1);
        let k2 = y2.trailing_zeros();
        let nxt2 = (y2 >> k2) as usize;
        if cache_hit(cache, nxt2, lim) {
            let steps = i64::from(unsafe { cache_get(cache, nxt2) })
                + 1
                + i64::from(k2)
                + 1
                + i64::from(k);
            if fill_doubles {
                cache_set_with_doubles(cache, lim, n, steps as Step);
                if !cache_hit(cache, nxt, lim) {
                    let mid =
                        i64::from(unsafe { cache_get(cache, nxt2) }) + 1 + i64::from(k2);
                    cache_set_with_doubles(cache, lim, nxt, mid as Step);
                }
            } else {
                unsafe {
                    cache_set(cache, n, steps as Step);
                    if !cache_hit(cache, nxt, lim) {
                        let mid = i64::from(cache_get(cache, nxt2)) + 1 + i64::from(k2);
                        cache_set(cache, nxt, mid as Step);
                    }
                }
            }
            return steps;
        }
    }
    collatz_steps_cached(n as i64, cache, lim, stack, fill_doubles)
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
            collatz_odd_cached(n as usize, &mut cache, lim, &mut stack, true)
        } else {
            collatz_steps_cached(n, &mut cache, lim, &mut stack, true)
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
    fill_doubles: bool,
) -> i64 {
    let su = start as usize;
    if cache_hit(cache, su, lim) {
        return if start <= 1 {
            0
        } else {
            i64::from(unsafe { cache_get(cache, su) })
        };
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
        i64::from(unsafe { cache_get(cache, x as usize) })
    };
    steps += edge;
    while let Some((v, e)) = stack.pop() {
        steps += 1 + e;
        let vu = v as usize;
        if vu <= lim && !cache_hit(cache, vu, lim) {
            if fill_doubles {
                cache_set_with_doubles(cache, lim, vu, steps as Step);
            } else {
                unsafe { cache_set(cache, vu, steps as Step) };
            }
        }
    }
    steps
}

/// Record `cache[v] = steps` and fill `2v, 4v, …` while still inside `lim`.
fn cache_set_with_doubles(cache: &mut [Step], lim: usize, v: usize, steps: Step) {
    unsafe { cache_set(cache, v, steps) };
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
        unsafe { cache_set(cache, nxt, s) };
        cur = nxt;
    }
}

#[cfg(test)]
#[path = "collatz_tests.rs"]
mod tests;
