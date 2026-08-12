//! Recognized prime counting (`#{p ≤ limit | p prime}`) via the sieve of Eratosthenes.

/// Count primes in `2..=limit`. Same result as trial division over that range.
#[no_mangle]
pub extern "C" fn lumia_count_primes(limit: i64) -> i64 {
    if limit < 2 {
        return 0;
    }
    let n = limit as usize;
    let mut composite = vec![false; n + 1];
    let mut count: i64 = 0;
    let sqrt_n = (n as f64).sqrt() as usize + 1;
    for i in 2..=n {
        if composite[i] {
            continue;
        }
        count += 1;
        if i <= sqrt_n {
            let mut m = i * i;
            while m <= n {
                composite[m] = true;
                m += i;
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
        assert_eq!(lumia_count_primes(800_000), 63_951);
    }
}
