//! Number-theory kernels recognized by codegen SR.

/// `sum_{i=1}^{n} sum_{j=1}^{n} gcd(i, j) = sum_{k=1}^{n} φ(k)·⌊n/k⌋²`.
#[no_mangle]
pub extern "C" fn lumia_gcd_sum(n: i64) -> i64 {
    if n < 1 {
        return 0;
    }
    let n = n as usize;
    let mut phi = vec![0i64; n + 1];
    let mut primes = Vec::new();
    let mut is_comp = vec![false; n + 1];
    phi[1] = 1;
    for i in 2..=n {
        if !is_comp[i] {
            primes.push(i);
            phi[i] = (i as i64) - 1;
        }
        for &p in &primes {
            let v = i.saturating_mul(p);
            if v > n {
                break;
            }
            is_comp[v] = true;
            if i % p == 0 {
                phi[v] = phi[i] * (p as i64);
                break;
            }
            phi[v] = phi[i] * (p as i64 - 1);
        }
    }
    let mut total: i64 = 0;
    let mut k = 1usize;
    while k <= n {
        let q = n / k;
        let k2 = n / q;
        let mut phi_sum = 0i64;
        #[allow(clippy::needless_range_loop)]
        for t in k..=k2 {
            phi_sum = phi_sum.wrapping_add(phi[t]);
        }
        let qq = q as i64;
        total = total.wrapping_add(phi_sum.wrapping_mul(qq.wrapping_mul(qq)));
        k = k2 + 1;
    }
    total
}

/// `sum_{i=1}^{n} ⌊n/i⌋` via Dirichlet hyperbola (O(√n)).
#[no_mangle]
pub extern "C" fn lumia_divisor_sum(n: i64) -> i64 {
    if n < 1 {
        return 0;
    }
    let mut total: i64 = 0;
    let mut i = 1i64;
    while i <= n {
        let q = n / i;
        let i2 = n / q;
        total = total.wrapping_add(q.wrapping_mul(i2 - i + 1));
        i = i2 + 1;
    }
    total
}

/// `sum_{i=0}^{n-1} sum_{j=0}^{n-1} ((i*j + 1) % m)` via floor_sum.
/// When `n > m`, group by `i % m` → **O(m log m)**; else **O(n log m)**.
#[no_mangle]
pub extern "C" fn lumia_product_rem_sum(n: i64, m: i64) -> i64 {
    if n <= 0 || m < 2 {
        return 0;
    }
    if n > m {
        let full = n / m;
        let rem = n % m;
        let mut s: i64 = 0;
        for r in 0..m {
            let cnt = full + if r < rem { 1 } else { 0 };
            if cnt == 0 {
                continue;
            }
            let row = lumia_affine1_rem_sum(n, r, 1, m);
            s = s.wrapping_add(row.wrapping_mul(cnt));
        }
        return s;
    }
    let mut s: i64 = 0;
    for i in 0..n {
        s = s.wrapping_add(lumia_affine1_rem_sum(n, i, 1, m));
    }
    s
}

/// Affine integer matmul checksum:
/// `Σ_{i,j} ( Σ_k (i·n+k+1)·(k·n+j+1) ) % modulus`.
///
/// Inner `k` is closed-form; for each `i` the sum over `j` is an affine rem-sum
/// → **O(n log modulus)** via [`lumia_affine1_rem_sum`].
#[no_mangle]
pub extern "C" fn lumia_matmul_affine_checksum(n: i64, modulus: i64) -> i64 {
    if n <= 0 || modulus < 2 {
        return 0;
    }
    // sum_k (a+k)(b+n·k) for k=0..n-1, a=i·n+1, b=j+1
    // = (n·a + Σk)·b + (a·n·Σk + n·Σk²) = K·b + L, b = 1..=n
    let nm1 = n - 1;
    let sum_k = nm1.wrapping_mul(n) / 2;
    let sum_k2 = nm1.wrapping_mul(n).wrapping_mul(2 * n - 1) / 6;
    let mut total: i64 = 0;
    for i in 0..n {
        let a = i.wrapping_mul(n).wrapping_add(1);
        let k = n.wrapping_mul(a).wrapping_add(sum_k);
        let l = a
            .wrapping_mul(n)
            .wrapping_mul(sum_k)
            .wrapping_add(n.wrapping_mul(sum_k2));
        // Σ_{b=1}^{n} (K·b+L)%m = Σ_{t=0}^{n-1} (K·t + K+L)%m
        total = total.wrapping_add(lumia_affine1_rem_sum(n, k, k.wrapping_add(l), modulus));
    }
    total
}

/// `sum_{i=0}^{n-1} ((a*i + c) % m)` via floor_sum (O(log)).
#[no_mangle]
pub extern "C" fn lumia_affine1_rem_sum(n: i64, a: i64, c: i64, m: i64) -> i64 {
    if n <= 0 || m < 2 {
        return 0;
    }
    debug_assert!(
        a >= 0 && c >= 0,
        "affine1 SR domain is nonneg (a,c); got a={a} c={c}"
    );
    let a_m = a.rem_euclid(m);
    let c_m = c.rem_euclid(m);
    let linear = (a_m as i128) * (n as i128) * ((n - 1) as i128) / 2 + (c_m as i128) * (n as i128);
    let fs = floor_sum(n, m, a_m, c_m) as i128;
    (linear - (m as i128) * fs) as i64
}

/// AtCoder ACL `floor_sum`: `Σ_{i=0}^{n-1} ⌊(a·i + b) / m⌋` (nonneg domain).
fn floor_sum(n: i64, m: i64, a: i64, b: i64) -> u64 {
    debug_assert!(n >= 0 && m >= 1);
    let mut ans: u64 = 0;
    let mut n = n as u64;
    let mut m = m as u64;
    let mut a = a.rem_euclid(m as i64) as u64;
    let mut b = b.rem_euclid(m as i64) as u64;
    loop {
        if a >= m {
            ans = ans.wrapping_add(n.wrapping_mul(n.wrapping_sub(1)) / 2 * (a / m));
            a %= m;
        }
        if b >= m {
            ans = ans.wrapping_add(n * (b / m));
            b %= m;
        }
        let y_max = (a as u128) * (n as u128) + (b as u128);
        if y_max < m as u128 {
            break;
        }
        n = (y_max / m as u128) as u64;
        b = (y_max % m as u128) as u64;
        std::mem::swap(&mut m, &mut a);
    }
    ans
}

#[cfg(test)]
#[path = "number_theory_tests.rs"]
mod tests;
