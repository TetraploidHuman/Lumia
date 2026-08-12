//! CogniNucleus fused kernels: predictive-coding nucleus step + Hebbian update.
//!
//! Semantics mirror `cogninucleus/{nucleus,connection}.py` for tiny dense float
//! (size ≤ 64). Scratch stays on the stack so unique destination buffers remain
//! zero-alloc across a step.

use crate::common::{list_rc_is_unique, trap_abort, GcInhibitGuard, TYPE_LIST_F64};
use crate::gc::{list_payload_bytes, lumia_alloc};
use crate::list::{force_heap_list, list_float_elems, list_len_of};
use std::ptr;

const MAX_DIM: usize = 64;

/// One predictive-coding nucleus update (no soft-async skip).
///
/// ```text
/// err  = bottom_up − top_down
/// δ    = enc @ (precision · err)     // ≡ (π·err) @ encᵀ for square enc
/// mu  += state_lr · δ;  clamp(mu)
/// pred = pred_w @ mu                 // ≡ mu @ pred_wᵀ
/// ```
///
/// Updates `mu` in place; writes `err` and `pred`. Returns `mu`.
#[no_mangle]
pub extern "C" fn lumia_cn_nucleus_step(
    mu: *mut u8,
    enc_w: *mut u8,
    pred_w: *mut u8,
    bottom_up: *mut u8,
    top_down: *mut u8,
    err: *mut u8,
    pred: *mut u8,
    size: i64,
    state_lr: f64,
    precision: f64,
    mu_clip: f64,
) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if size < 1 || size as usize > MAX_DIM {
        trap_abort("lumia: cn nucleus_step size out of range");
    }
    let n = size as usize;

    let enc_w = force_f64(enc_w);
    let pred_w = force_f64(pred_w);
    let bottom_up = force_f64(bottom_up);
    let top_down = force_f64(top_down);
    let mu = ensure_unique_f64(mu);
    let err = ensure_unique_f64(err);
    let pred = ensure_unique_f64(pred);

    require_len(enc_w, size * size, "cn enc");
    require_len(pred_w, size * size, "cn pred_w");
    require_len(bottom_up, size, "cn bottom_up");
    require_len(top_down, size, "cn top_down");
    require_len(mu, size, "cn mu");
    require_len(err, size, "cn err");
    require_len(pred, size, "cn pred");

    unsafe {
        let (bu, _) = f64_elems(bottom_up);
        let (td, _) = f64_elems(top_down);
        let (ep, _) = f64_elems_mut(err);
        let (mp, _) = f64_elems_mut(mu);
        let (pp, _) = f64_elems_mut(pred);
        let (enc, _) = f64_elems(enc_w);
        let (pw, _) = f64_elems(pred_w);

        // err = bu - td; scratch = π * err
        let mut scratch = [0.0_f64; MAX_DIM];
        let mut delta = [0.0_f64; MAX_DIM];
        for i in 0..n {
            let e = *bu.add(i) - *td.add(i);
            *ep.add(i) = e;
            scratch[i] = precision * e;
        }

        // delta = enc @ scratch
        for i in 0..n {
            let mut s = 0.0_f64;
            let row = enc.add(i * n);
            for j in 0..n {
                s += *row.add(j) * scratch[j];
            }
            delta[i] = s;
        }

        // mu += lr * delta; clamp
        let lo = -mu_clip;
        let hi = mu_clip;
        for i in 0..n {
            let v = (*mp.add(i) + state_lr * delta[i]).clamp(lo, hi);
            *mp.add(i) = v;
        }

        // pred = pred_w @ mu
        for i in 0..n {
            let mut s = 0.0_f64;
            let row = pw.add(i * n);
            for j in 0..n {
                s += *row.add(j) * *mp.add(j);
            }
            *pp.add(i) = s;
        }
    }
    mu
}

/// Fused Hebbian: normalize `u`,`v` (stack copies); optional decay; rank-1 add;
/// clamp; multiply by `mask`. `W` is row-major `from×to`.
///
/// Mirrors `ConnectionManager.hebbian_update` (`eps = 1e-3` typical).
#[no_mangle]
pub extern "C" fn lumia_cn_hebbian(
    w: *mut u8,
    u: *mut u8,
    v: *mut u8,
    mask: *mut u8,
    from_size: i64,
    to_size: i64,
    lr: f64,
    weight_clip: f64,
    weight_decay: f64,
    eps: f64,
) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if from_size < 1 || to_size < 1 {
        trap_abort("lumia: cn hebbian non-positive dims");
    }
    if from_size as usize > MAX_DIM || to_size as usize > MAX_DIM {
        trap_abort("lumia: cn hebbian dim out of range");
    }
    let m = from_size as usize;
    let n = to_size as usize;

    let u = force_f64(u);
    let v = force_f64(v);
    let mask = force_f64(mask);
    let w = ensure_unique_f64(w);

    require_len(w, from_size * to_size, "cn hebbian W");
    require_len(u, from_size, "cn hebbian u");
    require_len(v, to_size, "cn hebbian v");
    require_len(mask, from_size * to_size, "cn hebbian mask");

    unsafe {
        let (up, _) = f64_elems(u);
        let (vp, _) = f64_elems(v);
        let (wp, _) = f64_elems_mut(w);
        let (mp, _) = f64_elems(mask);

        let mut uu = [0.0_f64; MAX_DIM];
        let mut vv = [0.0_f64; MAX_DIM];
        let mut su = 0.0_f64;
        let mut sv = 0.0_f64;
        for i in 0..m {
            let x = *up.add(i);
            uu[i] = x;
            su += x * x;
        }
        for j in 0..n {
            let y = *vp.add(j);
            vv[j] = y;
            sv += y * y;
        }
        let inv_u = 1.0 / (su.sqrt() + eps);
        let inv_v = 1.0 / (sv.sqrt() + eps);
        for i in 0..m {
            uu[i] *= inv_u;
        }
        for j in 0..n {
            vv[j] *= inv_v;
        }

        let keep = 1.0 - weight_decay;
        let lo = -weight_clip;
        let hi = weight_clip;
        for i in 0..m {
            let ui = uu[i] * lr;
            let row = i * n;
            for j in 0..n {
                let idx = row + j;
                let mut zij = *wp.add(idx) * keep + ui * vv[j];
                zij = zij.clamp(lo, hi);
                *wp.add(idx) = zij * *mp.add(idx);
            }
        }
    }
    w
}

/// `y = clamp(x @ W, -clip, clip)` with `W` row-major `from×to` (CN `Connection.project`).
#[no_mangle]
pub extern "C" fn lumia_cn_project_clamp(
    from_size: i64,
    to_size: i64,
    w: *mut u8,
    x: *mut u8,
    y: *mut u8,
    clip: f64,
) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if from_size < 1 || to_size < 1 {
        trap_abort("lumia: cn project_clamp non-positive dims");
    }
    if from_size as usize > MAX_DIM || to_size as usize > MAX_DIM {
        trap_abort("lumia: cn project_clamp dim out of range");
    }
    let m = from_size as usize;
    let n = to_size as usize;
    let w = force_f64(w);
    let x = force_f64(x);
    let y = ensure_unique_f64(y);
    require_len(w, from_size * to_size, "cn project W");
    require_len(x, from_size, "cn project x");
    require_len(y, to_size, "cn project y");
    let lo = -clip;
    let hi = clip;
    unsafe {
        let (ap, _) = f64_elems(w);
        let (xp, _) = f64_elems(x);
        let (yp, _) = f64_elems_mut(y);
        for j in 0..n {
            *yp.add(j) = 0.0;
        }
        for i in 0..m {
            let xi = *xp.add(i);
            let row = ap.add(i * n);
            for j in 0..n {
                *yp.add(j) += *row.add(j) * xi;
            }
        }
        for j in 0..n {
            *yp.add(j) = (*yp.add(j)).clamp(lo, hi);
        }
    }
    y
}

/// `y = clamp(W @ x, -clip, clip)` with `W` row-major `m×n` (CN lateral `error @ Wᵀ`).
#[no_mangle]
pub extern "C" fn lumia_cn_backproj_clamp(
    m: i64,
    n: i64,
    w: *mut u8,
    x: *mut u8,
    y: *mut u8,
    clip: f64,
) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if m < 1 || n < 1 {
        trap_abort("lumia: cn backproj_clamp non-positive dims");
    }
    if m as usize > MAX_DIM || n as usize > MAX_DIM {
        trap_abort("lumia: cn backproj_clamp dim out of range");
    }
    let mm = m as usize;
    let nn = n as usize;
    let w = force_f64(w);
    let x = force_f64(x);
    let y = ensure_unique_f64(y);
    require_len(w, m * n, "cn backproj W");
    require_len(x, n, "cn backproj x");
    require_len(y, m, "cn backproj y");
    let lo = -clip;
    let hi = clip;
    unsafe {
        let (ap, _) = f64_elems(w);
        let (xp, _) = f64_elems(x);
        let (yp, _) = f64_elems_mut(y);
        for i in 0..mm {
            let mut s = 0.0_f64;
            let row = ap.add(i * nn);
            for j in 0..nn {
                s += *row.add(j) * *xp.add(j);
            }
            *yp.add(i) = s.clamp(lo, hi);
        }
    }
    y
}

/// `y = clamp(y + α·x, -clip, clip)`.
#[no_mangle]
pub extern "C" fn lumia_cn_axpy_clamp(
    y: *mut u8,
    alpha: f64,
    x: *mut u8,
    clip: f64,
) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let x = force_f64(x);
    let y = ensure_unique_f64(y);
    let n = list_len_of(x);
    require_len(y, n, "cn axpy_clamp y");
    let lo = -clip;
    let hi = clip;
    unsafe {
        let (yp, nn) = f64_elems_mut(y);
        let (xp, _) = f64_elems(x);
        for i in 0..nn {
            *yp.add(i) = (*yp.add(i) + alpha * *xp.add(i)).clamp(lo, hi);
        }
    }
    y
}

/// Index of the maximum element (first on ties). Empty → `-1`.
#[no_mangle]
pub extern "C" fn lumia_cn_argmax(xs: *mut u8) -> i64 {
    let xs = force_f64(xs);
    unsafe {
        let (p, n) = f64_elems(xs);
        if n == 0 {
            return -1;
        }
        let mut best_i = 0usize;
        let mut best = *p;
        for i in 1..n {
            let v = *p.add(i);
            if v > best {
                best = v;
                best_i = i;
            }
        }
        best_i as i64
    }
}

fn force_f64(list: *mut u8) -> *mut u8 {
    let list = force_heap_list(list);
    if list.is_null() {
        trap_abort("lumia: cn on null list");
    }
    if !list_float_elems(list) {
        trap_abort("lumia: cn expects List[Float]");
    }
    list
}

fn ensure_unique_f64(list: *mut u8) -> *mut u8 {
    let list = force_f64(list);
    if list_rc_is_unique(list) {
        return list;
    }
    unsafe {
        let n = *(list as *const i64);
        let dest = lumia_alloc(list_payload_bytes(n), TYPE_LIST_F64);
        if dest.is_null() {
            trap_abort("lumia: cn clone OOM");
        }
        ptr::copy_nonoverlapping(list as *const i64, dest as *mut i64, (n as usize) + 1);
        dest
    }
}

fn require_len(list: *mut u8, expect: i64, what: &str) {
    let n = list_len_of(list);
    if n != expect {
        trap_abort(&format!("lumia: {what} len {n} != {expect}"));
    }
}

unsafe fn f64_elems(list: *mut u8) -> (*const f64, usize) {
    let n = *(list as *const i64) as usize;
    ((list as *const i64).add(1) as *const f64, n)
}

unsafe fn f64_elems_mut(list: *mut u8) -> (*mut f64, usize) {
    let n = *(list as *const i64) as usize;
    ((list as *mut i64).add(1) as *mut f64, n)
}

#[cfg(test)]
mod tests {
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
        f64::from_bits(lumia_list_get(list, i) as u64)
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
        let mu = lumia_cn_nucleus_step(
            mu, enc, pred_w, bu, td, err, pred, size, 0.5, 1.0, 10.0,
        );
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
        let w = lumia_cn_hebbian(w, u, v, mask, 2, 2, 1.0, 10.0, 0.0, 1e-3);
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
        let y = lumia_cn_project_clamp(2, 2, w, x, y, 10.0);
        assert!((get_f(y, 0) - 1.0).abs() < 1e-12);
        assert!(get_f(y, 1).abs() < 1e-12);

        let y = from_slice(&[9.0, 0.0]);
        let x = from_slice(&[2.0, 0.0]);
        let y = lumia_cn_axpy_clamp(y, 1.0, x, 10.0);
        assert!((get_f(y, 0) - 10.0).abs() < 1e-12); // clamped
        assert_eq!(lumia_cn_argmax(y), 0);
    }
}
