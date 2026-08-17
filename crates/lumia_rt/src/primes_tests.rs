use super::*;

#[test]
fn bench_cpu_primes_800k() {
    assert_eq!(lumia_count_primes(800_000), 63_951);
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
    assert_eq!(lumia_count_primes(0), 0);
    assert_eq!(lumia_count_primes(1), 0);
    assert_eq!(lumia_count_primes(-1), 0);
    assert_eq!(lumia_count_primes(2), 1);
    assert_eq!(lumia_count_primes(3), 2);
    for limit in [2i64, 3, 10, 100, 1_000, 5_000, 20_000, 50_000] {
        assert_eq!(lumia_count_primes(limit), count(limit), "limit={limit}");
    }
}
