// Extracted from production module (Todo: RT 测例半迁).
use super::*;
use crate::dense_f64::lumia_list_f64_zeros;
use crate::list::lumia_list_get;

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

fn get_f(list: *mut u8, i: i64) -> f64 {
    f64::from_bits(unsafe { lumia_list_get(list, i) } as u64)
}

#[test]
fn nucleus_identity_encoder() {
    let size = 4i64;
    let mu = from_slice(&[0.0, 0.0, 0.0, 0.0]);
    let mut eye = vec![0.0; 16];
    for i in 0..4 {
        eye[i * 4 + i] = 1.0;
    }
    let enc = from_slice(&eye);
    let pred_w = from_slice(&eye);
    let bu = from_slice(&[1.0, 0.0, 0.0, 0.0]);
    let td = from_slice(&[0.0, 0.0, 0.0, 0.0]);
    let err = lumia_list_f64_zeros(size);
    let pred = lumia_list_f64_zeros(size);
    let mu = unsafe { lumia_cn_nucleus_step(mu, enc, pred_w, bu, td, err, pred, size, 0.5, 1.0, 10.0) };
    // err=[1,0,0,0]; delta=err; mu=0.5*err; pred=mu
    assert!((get_f(mu, 0) - 0.5).abs() < 1e-12);
    assert!((get_f(pred, 0) - 0.5).abs() < 1e-12);
    assert!((get_f(err, 0) - 1.0).abs() < 1e-12);
}

#[test]
fn hebbian_outer_and_mask() {
    let w = from_slice(&[0.0, 0.0, 0.0, 0.0]);
    let u = from_slice(&[3.0, 0.0]); // → ≈[1,0]
    let v = from_slice(&[0.0, 4.0]); // → ≈[0,1]
                                     // Zero the (1,0) synapse; keep (0,1) which receives the outer product.
    let mask = from_slice(&[1.0, 1.0, 0.0, 1.0]);
    let w = unsafe { lumia_cn_hebbian(w, u, v, mask, 2, 2, 1.0, 10.0, 0.0, 1e-3) };
    assert!(get_f(w, 0).abs() < 1e-9);
    assert!((get_f(w, 1) - 1.0).abs() < 1e-3);
    assert!(get_f(w, 2).abs() < 1e-9);
    assert!(get_f(w, 3).abs() < 1e-9);
}

#[test]
fn project_and_axpy_clamp() {
    // W 2×2 identity-ish: project [1,0] → [1,0] then clamp
    let w = from_slice(&[1.0, 0.0, 0.0, 1.0]);
    let x = from_slice(&[1.0, 0.0]);
    let y = lumia_list_f64_zeros(2);
    let y = unsafe { lumia_cn_project_clamp(2, 2, w, x, y, 10.0) };
    assert!((get_f(y, 0) - 1.0).abs() < 1e-12);
    assert!(get_f(y, 1).abs() < 1e-12);

    let y = from_slice(&[9.0, 0.0]);
    let x = from_slice(&[2.0, 0.0]);
    let y = unsafe { lumia_cn_axpy_clamp(y, 1.0, x, 10.0) };
    assert!((get_f(y, 0) - 10.0).abs() < 1e-12); // clamped
    assert_eq!(unsafe { lumia_cn_argmax(y) }, 0);
}

#[test]
fn cluster_rates_square_relu() {
    // size=4, n_clusters=2 → chunk=2; pot0=0, x=[1,0.4, 2,0]
    let pot = from_slice(&[0.0, 0.0, 0.0, 0.0]);
    let x = from_slice(&[1.0, 0.4, 2.0, 0.0]);
    let out = from_slice(&[0.0, 0.0, 0.0, 0.0]);
    let out = unsafe { lumia_cn_cluster_rates(pot, x, out, 4, 2, 0.9, 0.5) };
    // pot=x; out=(max(0,pot-0.5))^2 → [0.25, 0, 2.25, 0]
    assert!((get_f(out, 0) - 0.25).abs() < 1e-12);
    assert!(get_f(out, 1).abs() < 1e-12);
    assert!((get_f(out, 2) - 2.25).abs() < 1e-12);
    assert!(get_f(out, 3).abs() < 1e-12);
    assert!((get_f(pot, 0) - 1.0).abs() < 1e-12);
    assert!((get_f(pot, 2) - 2.0).abs() < 1e-12);
}

#[test]
fn learn_generative_outer_products() {
    let size = 2i64;
    let enc = from_slice(&[0.0, 0.0, 0.0, 0.0]);
    let pred = from_slice(&[0.0, 0.0, 0.0, 0.0]);
    let mu = from_slice(&[1.0, 0.0]);
    let err = from_slice(&[0.0, 2.0]);
    let enc = unsafe { lumia_cn_learn_generative(enc, pred, mu, err, size, 1.0, 10.0, 0.0, 1.0) };
    // pred += μ⊗err → [[0,2],[0,0]]; enc += 0.5·err⊗(π·err) → [[0,0],[0,2]]
    assert!(get_f(pred, 0).abs() < 1e-12);
    assert!((get_f(pred, 1) - 2.0).abs() < 1e-12);
    assert!(get_f(pred, 2).abs() < 1e-12);
    assert!(get_f(pred, 3).abs() < 1e-12);
    assert!(get_f(enc, 0).abs() < 1e-12);
    assert!(get_f(enc, 1).abs() < 1e-12);
    assert!(get_f(enc, 2).abs() < 1e-12);
    assert!((get_f(enc, 3) - 2.0).abs() < 1e-12);
}

#[test]
fn update_state_encoder_step() {
    let size = 2i64;
    let mu = from_slice(&[0.0, 0.0]);
    let mut eye = vec![0.0; 4];
    eye[0] = 1.0;
    eye[3] = 1.0;
    let enc = from_slice(&eye);
    let err = from_slice(&[2.0, 0.0]);
    let mu = unsafe { lumia_cn_update_state(mu, enc, err, size, 0.5, 1.0, 10.0) };
    // δ = err; μ += 0.5·δ → [1, 0]
    assert!((get_f(mu, 0) - 1.0).abs() < 1e-12);
    assert!(get_f(mu, 1).abs() < 1e-12);
}
