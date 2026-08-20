// Extracted from production module (Todo: RT 测例半迁).
use super::*;

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a.abs()
}

#[test]
fn bench_cpu_poly_checksum() {
    assert_eq!(
        lumia_affine2_rem_sum(12_000, 131, 17, 1, 10_007),
        720_427_763_375
    );
}

#[test]
fn matches_naive_small() {
    let (a, b, c, m) = (131, 17, 1, 10_007);
    for n in [0i64, 1, 50, 100] {
        let mut naive = 0i64;
        for i in 0..n {
            for j in 0..n {
                let v = (a * i + b * j + c).rem_euclid(m);
                naive = naive.wrapping_add(v);
            }
        }
        assert_eq!(lumia_affine2_rem_sum(n, a, b, c, m), naive, "n={n}");
    }
}

#[test]
fn matches_naive_when_gcd_b_m_gt_1() {
    let (a, b, c, m) = (3i64, 6i64, 2i64, 15i64);
    assert!(gcd(b.rem_euclid(m), m) > 1);
    for n in [0i64, 1, 2, 7, 30, 60, 120] {
        let mut naive = 0i64;
        for i in 0..n {
            for j in 0..n {
                let v = (a * i + b * j + c).rem_euclid(m);
                naive = naive.wrapping_add(v);
            }
        }
        assert_eq!(lumia_affine2_rem_sum(n, a, b, c, m), naive, "n={n}");
    }
}

#[test]
fn affine2_edges() {
    assert_eq!(lumia_affine2_rem_sum(10, 1, 1, 1, 1), 0);
    assert_eq!(lumia_affine2_rem_sum(-1, 1, 1, 1, 10), 0);
    assert_eq!(lumia_affine2_rem_sum(10, 1, 1, 1, 0), 0);
}

#[test]
fn medium_matches_naive() {
    let (a, b, c, m) = (131, 17, 1, 10_007);
    for n in [200i64, 400] {
        let mut naive = 0i64;
        for i in 0..n {
            for j in 0..n {
                let v = (a * i + b * j + c).rem_euclid(m);
                naive = naive.wrapping_add(v);
            }
        }
        assert_eq!(lumia_affine2_rem_sum(n, a, b, c, m), naive, "n={n}");
    }
}
