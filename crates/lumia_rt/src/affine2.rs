//! Nested affine rem-accumulate recognized by codegen.

/// `sum_{i=0}^{n-1} sum_{j=0}^{n-1} ((a*i + b*j + c) % m)`.
///
/// Assumes the non-negative domain of the Lumia source pattern (`i,j ≥ 0`,
/// positive `a,b,c,m`). When `gcd(b,m)=1`, each block of `m` consecutive `j`
/// hits every residue once, so full periods collapse to `m*(m-1)/2`.
#[no_mangle]
pub extern "C" fn lumia_affine2_rem_sum(n: i64, a: i64, b: i64, c: i64, m: i64) -> i64 {
    if n <= 0 || m < 2 {
        return 0;
    }
    debug_assert!(
        a >= 0 && b >= 0 && c >= 0,
        "affine2 SR domain is nonneg (a,b,c); got a={a} b={b} c={c}"
    );
    let a_m = a.rem_euclid(m);
    let b_m = b.rem_euclid(m);
    let c_m = c.rem_euclid(m);
    let g = gcd(b_m, m);
    let mut s: i64 = 0;
    if g == 1 {
        // Full residue period of length `m`.
        let t_full = m.wrapping_mul(m - 1) / 2;
        let q = n / m;
        let r = n % m;
        let q_t = q.wrapping_mul(t_full);
        for i in 0..n {
            let mut term = (a_m.wrapping_mul(i).wrapping_add(c_m)).rem_euclid(m);
            s = s.wrapping_add(q_t);
            for _ in 0..r {
                s = s.wrapping_add(term);
                term += b_m;
                if term >= m {
                    term -= m;
                }
            }
        }
    } else {
        // General: period `m/g` of the residue class; sum depends on start.
        let per = m / g;
        let q = n / per;
        let r = n % per;
        for i in 0..n {
            let a0 = (a_m.wrapping_mul(i).wrapping_add(c_m)).rem_euclid(m);
            let s_per = prefix_rem_sum(a0, b_m, per, m);
            s = s.wrapping_add(q.wrapping_mul(s_per));
            s = s.wrapping_add(prefix_rem_sum(a0, b_m, r, m));
        }
    }
    s
}

#[inline]
fn prefix_rem_sum(mut term: i64, b_m: i64, len: i64, m: i64) -> i64 {
    let mut s = 0i64;
    for _ in 0..len {
        s = s.wrapping_add(term);
        term += b_m;
        if term >= m {
            term -= m;
        }
    }
    s
}

#[inline]
fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a.abs()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Force the general period path (`gcd(b,m) > 1`).
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
}
