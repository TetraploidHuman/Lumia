// Extracted from production module (Todo: RT 测例半迁).
use super::*;

#[test]
fn bench_cpu_collatz_checksum() {
    assert_eq!(lumia_collatz_total(250_000), 29_265_567);
}

#[test]
fn bench_cpu_collatz_2_5m() {
    assert_eq!(lumia_collatz_total(2_500_000), 352_279_148);
}

/// Catch an unoptimized / stale `liblumia_rt` build: Release wall time for the
/// bench_cpu Collatz window should stay well under a quarter second. (A known
/// bad staticlib sat near ~100ms+ and doubled `bench_cpu`.)
#[test]
fn collatz_2_5m_release_not_pathologically_slow() {
    let t0 = std::time::Instant::now();
    let _ = lumia_collatz_total(2_500_000);
    let dt = t0.elapsed();
    assert!(
        dt.as_secs_f64() < 0.25,
        "lumia_collatz_total(2.5e6) took {dt:?}; expected <250ms in release \
         (try `cargo clean -p lumia_rt --release && cargo build -p lumia_rt --release`)"
    );
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
