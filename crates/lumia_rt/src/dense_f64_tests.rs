// Extracted from production module (Todo: RT 测例半迁).
use super::*;
use crate::list::{lumia_list_get, lumia_list_len, lumia_list_retain};

fn bits(f: f64) -> i64 {
    f.to_bits() as i64
}

fn from_slice(xs: &[f64]) -> *mut u8 {
    let p = lumia_list_f64_zeros(xs.len() as i64);
    unsafe {
        let (dst, _) = f64_elems_mut(p);
        for (i, &v) in xs.iter().enumerate() {
            *dst.add(i) = v;
        }
    }
    p
}

#[test]
fn scale_mul_add() {
    let a = from_slice(&[1.0, 2.0, 3.0, 4.0]);
    let b = from_slice(&[2.0, 2.0, 2.0, 2.0]);
    let a = unsafe { lumia_f64_scale(a, 0.5) };
    assert_eq!(unsafe { lumia_list_get(a, 0) }, bits(0.5));
    let out = lumia_list_f64_zeros(4);
    let out = unsafe { lumia_f64_mul(out, a, b) };
    assert_eq!(unsafe { lumia_list_get(out, 1) }, bits(2.0));
    let out2 = lumia_list_f64_zeros(4);
    let out2 = unsafe { lumia_f64_add(out2, a, b) };
    assert_eq!(unsafe { lumia_list_get(out2, 0) }, bits(2.5));
}

#[test]
fn zeros_and_fill() {
    let xs = lumia_list_f64_zeros(3);
    assert_eq!(unsafe { lumia_list_len(xs) }, 3);
    assert_eq!(unsafe { lumia_list_get(xs, 0) }, bits(0.0));
    let xs = unsafe { lumia_f64_fill(xs, 2.5) };
    assert_eq!(unsafe { lumia_list_get(xs, 1) }, bits(2.5));
}

#[test]
fn gemv_matches_naive() {
    // A = [[1,2],[3,4],[5,6]], x = [1,2] → y = [5,11,17]
    let a = from_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    let x = from_slice(&[1.0, 2.0]);
    let y = lumia_list_f64_zeros(3);
    let y = unsafe { lumia_f64_gemv(3, 2, a, x, y) };
    assert_eq!(unsafe { lumia_list_get(y, 0) }, bits(5.0));
    assert_eq!(unsafe { lumia_list_get(y, 1) }, bits(11.0));
    assert_eq!(unsafe { lumia_list_get(y, 2) }, bits(17.0));
}

#[test]
fn gemv_t_and_addmm() {
    let a = from_slice(&[1.0, 2.0, 3.0, 4.0]); // 2×2
    let x = from_slice(&[1.0, 1.0]);
    let y = lumia_list_f64_zeros(2);
    let y = unsafe { lumia_f64_gemv_t(2, 2, a, x, y) };
    // Aᵀ @ [1,1] = [1+3, 2+4] = [4,6]
    assert_eq!(unsafe { lumia_list_get(y, 0) }, bits(4.0));
    assert_eq!(unsafe { lumia_list_get(y, 1) }, bits(6.0));

    let w = lumia_list_f64_zeros(4);
    let u = from_slice(&[1.0, 2.0]);
    let v = from_slice(&[3.0, 4.0]);
    let w = unsafe { lumia_f64_addmm(2, 2, w, u, v, 1.0) };
    // [[3,4],[6,8]]
    assert_eq!(unsafe { lumia_list_get(w, 0) }, bits(3.0));
    assert_eq!(unsafe { lumia_list_get(w, 1) }, bits(4.0));
    assert_eq!(unsafe { lumia_list_get(w, 2) }, bits(6.0));
    assert_eq!(unsafe { lumia_list_get(w, 3) }, bits(8.0));
}

#[test]
fn normalize_and_cow() {
    let xs = from_slice(&[3.0, 4.0]);
    let xs = unsafe { lumia_f64_l2_normalize(xs, 0.0) };
    assert!((unsafe { lumia_f64_l2_norm(xs) } - 1.0).abs() < 1e-12);

    let a = from_slice(&[1.0, 0.0]);
    unsafe { lumia_list_retain(a) };
    let b = unsafe { lumia_f64_fill(a, 9.0) };
    // Shared → COW; original retained binding keeps old bits.
    assert_ne!(a, b);
    assert_eq!(unsafe { lumia_list_get(a, 0) }, bits(1.0));
    assert_eq!(unsafe { lumia_list_get(b, 0) }, bits(9.0));
}

#[test]
fn nucleus_scale_gemv_checksum() {
    // 16×32 projection fingerprint used as a stable oracle.
    let m = 16i64;
    let n = 32i64;
    let a = lumia_list_f64_zeros(m * n);
    let x = lumia_list_f64_zeros(n);
    let y = lumia_list_f64_zeros(m);
    unsafe {
        let (ap, _) = f64_elems_mut(a);
        let (xp, _) = f64_elems_mut(x);
        for j in 0..n as usize {
            *xp.add(j) = (j as f64) * 0.01;
        }
        for i in 0..m as usize {
            for j in 0..n as usize {
                *ap.add(i * n as usize + j) = ((i + j) as f64) * 0.001;
            }
        }
    }
    let y = unsafe { lumia_f64_gemv(m, n, a, x, y) };
    assert_eq!(unsafe { lumia_f64_checksum(y) }, 2261);
}
