//! Nested affine rem-accumulate recognized by codegen.

use crate::number_theory::lumia_affine1_rem_sum;

/// `sum_{i=0}^{n-1} sum_{j=0}^{n-1} ((a*i + b*j + c) % m)`.
///
/// Assumes the non-negative domain of the Lumia source pattern (`i,j ≥ 0`,
/// positive `a,b,c,m`). Inner `j`-sums reuse the O(log) affine-1 kernel.
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
    // Σ_i Σ_j (a·i + b·j + c) % m = Σ_i affine1_rem_sum(n, b, a·i+c, m)
    let mut s: i64 = 0;
    for i in 0..n {
        let c_i = (a_m.wrapping_mul(i).wrapping_add(c_m)).rem_euclid(m);
        s = s.wrapping_add(lumia_affine1_rem_sum(n, b_m, c_i, m));
    }
    s
}

#[cfg(test)]
#[path = "affine2_tests.rs"]
mod tests;
