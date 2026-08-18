// Extracted from production module (Todo: RT 测例半迁).
use super::*;

#[test]
fn dot_and_axpy_match_scalar() {
    let a = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
    let b = [0.5; 9];
    let d = dot_f64(a.as_ptr(), b.as_ptr(), 9);
    assert!((d - 22.5).abs() < 1e-9, "d={d}");
    let mut y = [0.0; 9];
    let x = [1.0; 9];
    axpy_scale_f64(y.as_mut_ptr(), x.as_ptr(), 2.0, 9);
    assert!(y.iter().all(|&v| (v - 2.0).abs() < 1e-12));
}

#[test]
fn elementwise_and_clamp() {
    let a = [1.0, 2.0, 3.0, 4.0, 5.0];
    let b = [5.0, 4.0, 3.0, 2.0, 1.0];
    let mut out = [0.0; 5];
    add_f64(out.as_mut_ptr(), a.as_ptr(), b.as_ptr(), 5);
    assert!(out.iter().all(|&v| (v - 6.0).abs() < 1e-12));
    sub_f64(out.as_mut_ptr(), a.as_ptr(), b.as_ptr(), 5);
    assert!((out[0] + 4.0).abs() < 1e-12);
    fill_f64(out.as_mut_ptr(), 5, 9.0);
    assert!(out.iter().all(|&v| v == 9.0));
    scale_f64(out.as_mut_ptr(), 5, 0.5);
    assert!(out.iter().all(|&v| (v - 4.5).abs() < 1e-12));
    clamp_f64(out.as_mut_ptr(), 5, 0.0, 1.0);
    assert!(out.iter().all(|&v| (v - 1.0).abs() < 1e-12));
    let mut y = [1.0; 5];
    axpy_clamp_f64(y.as_mut_ptr(), a.as_ptr(), 1.0, 5, 0.0, 3.0);
    assert_eq!(y, [2.0, 3.0, 3.0, 3.0, 3.0]);
}
