//! Recognized prime counting (`#{p ≤ limit | p prime}`) via the sieve of Eratosthenes.
//!
//! Odds-only bitset: half the memory traffic of a dense `bool` sieve.

/// Count primes in `2..=limit`. Same result as trial division over that range.
#[no_mangle]
pub extern "C" fn lumi_count_primes(limit: i64) -> i64 {
    if limit < 2 {
        return 0;
    }
    if limit == 2 {
        return 1;
    }
    let n = limit as usize;
    // Index `i` stores odd integer `2*i+3` (so 3,5,7,…).
    let n_odds = (n - 1) / 2; // odds in 3..=n when n>=3: floor((n-1)/2) entries for 3,5,..., up to ≤n
                              // Number of odd candidates from 3 to n inclusive:
                              // last odd ≤ n is n if n odd else n-1; count = ((last-3)/2)+1 = (n-1)/2 when n odd...
                              // For n=10: odds 3,5,7,9 → 4 = n/2 - 0? (10-1)/2=4. OK
                              // For n=11: 3,5,7,9,11 → 5 = 11/2? (11-1)/2=5. OK
    let mut bits = vec![0u64; n_odds.div_ceil(64)];
    let mark = |bits: &mut [u64], idx: usize| {
        bits[idx / 64] |= 1u64 << (idx % 64);
    };
    let is_marked = |bits: &[u64], idx: usize| -> bool { (bits[idx / 64] >> (idx % 64)) & 1 != 0 };

    let sqrt_n = (n as f64).sqrt() as usize + 1;
    let mut count: i64 = 1; // prime 2
    for i in 0..n_odds {
        if is_marked(&bits, i) {
            continue;
        }
        let p = 2 * i + 3;
        count += 1;
        if p <= sqrt_n {
            // Mark odd multiples of p starting at p*p.
            let mut m = p * p;
            if m % 2 == 0 {
                m += p; // should already be odd for odd p
            }
            while m <= n {
                // m is odd; index = (m-3)/2
                mark(&mut bits, (m - 3) / 2);
                m += 2 * p; // next odd multiple
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_cpu_primes_800k() {
        assert_eq!(lumi_count_primes(800_000), 63_951);
    }

    #[test]
    fn primes_vs_trial_oracle() {
        fn is_prime(n: i64) -> bool {
            if n < 2 {
                return false;
            }
            let mut d = 2i64;
            while d * d <= n {
                if n % d == 0 {
                    return false;
                }
                d += 1;
            }
            true
        }
        fn count(limit: i64) -> i64 {
            (2..=limit).filter(|&n| is_prime(n)).count() as i64
        }
        assert_eq!(lumi_count_primes(0), 0);
        assert_eq!(lumi_count_primes(1), 0);
        assert_eq!(lumi_count_primes(-1), 0);
        assert_eq!(lumi_count_primes(2), 1);
        assert_eq!(lumi_count_primes(3), 2);
        for limit in [2i64, 3, 10, 100, 1_000, 5_000, 20_000, 50_000] {
            assert_eq!(lumi_count_primes(limit), count(limit), "limit={limit}");
        }
    }
}
