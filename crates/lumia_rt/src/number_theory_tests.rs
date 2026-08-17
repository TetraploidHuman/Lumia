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

fn naive_gcd_sum(n: i64) -> i64 {
    let mut s = 0i64;
    for i in 1..=n {
        for j in 1..=n {
            s += gcd(i, j);
        }
    }
    s
}

fn naive_div_sum(n: i64) -> i64 {
    (1..=n).map(|i| n / i).sum()
}

fn naive_prod(n: i64, m: i64) -> i64 {
    let mut s = 0i64;
    for i in 0..n {
        for j in 0..n {
            s = s.wrapping_add((i * j + 1).rem_euclid(m));
        }
    }
    s
}

fn naive_aff1(n: i64, a: i64, c: i64, m: i64) -> i64 {
    let mut s = 0i64;
    for i in 0..n {
        s = s.wrapping_add((a * i + c).rem_euclid(m));
    }
    s
}

#[test]
fn bench_checksums() {
    assert_eq!(lumia_gcd_sum(1400), 9_122_320);
    assert_eq!(lumia_divisor_sum(12_000_000), 197_458_334);
    assert_eq!(lumia_product_rem_sum(9000, 10007), 405_134_546_788);
    assert_eq!(
        lumia_affine1_rem_sum(5_000_000, 131, 17, 10007),
        25_014_941_572
    );
    assert_eq!(lumia_matmul_affine_checksum(360, 1_000_003), 65_061_251_965);
    assert_eq!(
        lumia_matmul_affine_checksum(2000, 1_000_003),
        1_998_964_270_721
    );
}

fn naive_matmul(n: i64, m: i64) -> i64 {
    let mut sum = 0i64;
    for i in 0..n {
        for j in 0..n {
            let mut cell = 0i64;
            for k in 0..n {
                cell = cell.wrapping_add((i * n + k + 1) * (k * n + j + 1));
            }
            sum = sum.wrapping_add(cell.rem_euclid(m));
        }
    }
    sum
}

#[test]
fn small_matches_naive() {
    for n in [1i64, 10, 50, 100] {
        assert_eq!(lumia_gcd_sum(n), naive_gcd_sum(n), "gcd n={n}");
        assert_eq!(lumia_divisor_sum(n), naive_div_sum(n), "div n={n}");
        assert_eq!(lumia_product_rem_sum(n, 10007), naive_prod(n, 10007));
        assert_eq!(
            lumia_affine1_rem_sum(n, 131, 17, 10007),
            naive_aff1(n, 131, 17, 10007)
        );
        assert_eq!(
            lumia_matmul_affine_checksum(n, 1_000_003),
            naive_matmul(n, 1_000_003),
            "matmul n={n}"
        );
    }
}

#[test]
fn product_rem_grouping_path_when_n_gt_m() {
    // Exercises the O(m log m) branch (`n > m`).
    for &(n, m) in &[(20i64, 7), (100, 13), (10007 + 3, 10007), (20_000, 10007)] {
        assert_eq!(
            lumia_product_rem_sum(n, m),
            naive_prod(n, m),
            "prod n={n} m={m}"
        );
    }
}

#[test]
fn product_rem_and_aff1_edges() {
    assert_eq!(lumia_product_rem_sum(0, 10007), 0);
    assert_eq!(lumia_product_rem_sum(-1, 10007), 0);
    assert_eq!(lumia_product_rem_sum(10, 1), 0);
    assert_eq!(lumia_affine1_rem_sum(0, 131, 17, 10007), 0);
    assert_eq!(lumia_affine1_rem_sum(100, 0, 0, 10007), 0);
    for n in 0..64 {
        assert_eq!(
            lumia_affine1_rem_sum(n, 3, 5, 17),
            naive_aff1(n, 3, 5, 17),
            "aff1 n={n}"
        );
    }
}

#[test]
fn matmul_dense_sweep_vs_naive() {
    for n in [0i64, 1, 2, 3, 7, 16, 32, 64, 128] {
        assert_eq!(
            lumia_matmul_affine_checksum(n, 1_000_003),
            naive_matmul(n, 1_000_003),
            "matmul n={n}"
        );
    }
}

#[test]
fn gcd_div_medium_vs_naive() {
    for n in [200i64, 400, 800] {
        assert_eq!(lumia_gcd_sum(n), naive_gcd_sum(n), "gcd n={n}");
        assert_eq!(lumia_divisor_sum(n), naive_div_sum(n), "div n={n}");
    }
    // Larger divisor sum still cheap (O(√n)).
    assert_eq!(lumia_divisor_sum(1_000_000), naive_div_sum(1_000_000));
}

#[test]
fn gcd_div_empty() {
    assert_eq!(lumia_gcd_sum(0), 0);
    assert_eq!(lumia_gcd_sum(-3), 0);
    assert_eq!(lumia_divisor_sum(0), 0);
    assert_eq!(lumia_divisor_sum(-1), 0);
}
