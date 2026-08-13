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
        crate::f64_simd::matvec_f64(enc, scratch.as_ptr(), delta.as_mut_ptr(), n, n);

        // mu += lr * delta; clamp
        let lo = -mu_clip;
        let hi = mu_clip;
        for i in 0..n {
            let v = (*mp.add(i) + state_lr * delta[i]).clamp(lo, hi);
            *mp.add(i) = v;
        }

        // pred = pred_w @ mu
        crate::f64_simd::matvec_f64(pw, mp, pp, n, n);
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
        let su = crate::f64_simd::dot_f64(up, up, m);
        let sv = crate::f64_simd::dot_f64(vp, vp, n);
        std::ptr::copy_nonoverlapping(up, uu.as_mut_ptr(), m);
        std::ptr::copy_nonoverlapping(vp, vv.as_mut_ptr(), n);
        crate::f64_simd::scale_f64(uu.as_mut_ptr(), m, 1.0 / (su.sqrt() + eps));
        crate::f64_simd::scale_f64(vv.as_mut_ptr(), n, 1.0 / (sv.sqrt() + eps));

        let keep = 1.0 - weight_decay;
        let lo = -weight_clip;
        let hi = weight_clip;
        for i in 0..m {
            let ui = uu[i] * lr;
            let row = wp.add(i * n);
            crate::f64_simd::hebbian_row_f64(row, vv.as_ptr(), mp.add(i * n), n, ui, keep, lo, hi);
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
        crate::f64_simd::project_clamp_f64(ap, xp, yp, m, n, lo, hi);
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
        crate::f64_simd::matvec_clamp_f64(ap, xp, yp, mm, nn, lo, hi);
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
        crate::f64_simd::axpy_clamp_f64(yp, xp, alpha, nn, lo, hi);
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

/// Cluster rate coding (`Nucleus._effective_bottom_up` / `Cluster.step`).
///
/// `cluster_size = size / n_clusters`; only the first `n_clusters · cluster_size`
/// elements are rate-coded (`pot = decay·pot + x`; `out = max(0, pot − thr)²`).
/// Any remainder is written as `0` in `output` (potential left unchanged).
/// Updates `potential` in place; returns `output`.
#[no_mangle]
pub extern "C" fn lumia_cn_cluster_rates(
    potential: *mut u8,
    input: *mut u8,
    output: *mut u8,
    size: i64,
    n_clusters: i64,
    decay: f64,
    threshold: f64,
) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if size < 1 || size as usize > MAX_DIM {
        trap_abort("lumia: cn cluster_rates size out of range");
    }
    if n_clusters < 1 {
        trap_abort("lumia: cn cluster_rates n_clusters < 1");
    }
    let n = size as usize;
    let nc = n_clusters as usize;
    let chunk = (n / nc).max(1);
    let covered = (chunk * nc).min(n);
    let input = force_f64(input);
    let potential = ensure_unique_f64(potential);
    let output = ensure_unique_f64(output);
    require_len(input, size, "cn cluster input");
    require_len(potential, size, "cn cluster pot");
    require_len(output, size, "cn cluster out");
    unsafe {
        let (xp, _) = f64_elems(input);
        let (pp, _) = f64_elems_mut(potential);
        let (op, _) = f64_elems_mut(output);
        let mut i = 0usize;
        while i < covered {
            let pot = *pp.add(i) * decay + *xp.add(i);
            *pp.add(i) = pot;
            let o = (pot - threshold).max(0.0);
            *op.add(i) = o * o;
            i += 1;
        }
        while i < n {
            *op.add(i) = 0.0;
            i += 1;
        }
    }
    output
}

/// Nucleus local generative plasticity (`Nucleus.learn_generative`).
///
/// ```text
/// pred_w += lr · (μ ⊗ err)          // after optional decay
/// enc_w  += (lr/2) · (err ⊗ (π·err))
/// clamp both to ±weight_clip
/// ```
/// Mutates `enc_w` and `pred_w` (unique). Returns `enc_w`.
#[no_mangle]
pub extern "C" fn lumia_cn_learn_generative(
    enc_w: *mut u8,
    pred_w: *mut u8,
    mu: *mut u8,
    err: *mut u8,
    size: i64,
    lr: f64,
    weight_clip: f64,
    weight_decay: f64,
    precision: f64,
) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if size < 1 || size as usize > MAX_DIM {
        trap_abort("lumia: cn learn_generative size out of range");
    }
    let n = size as usize;
    let mu = force_f64(mu);
    let err = force_f64(err);
    let enc_w = ensure_unique_f64(enc_w);
    let pred_w = ensure_unique_f64(pred_w);
    require_len(mu, size, "cn learn mu");
    require_len(err, size, "cn learn err");
    require_len(enc_w, size * size, "cn learn enc");
    require_len(pred_w, size * size, "cn learn pred");
    let keep = 1.0 - weight_decay;
    let lo = -weight_clip;
    let hi = weight_clip;
    let half = lr * 0.5;
    unsafe {
        let (mp, _) = f64_elems(mu);
        let (ep, _) = f64_elems(err);
        let (enc, _) = f64_elems_mut(enc_w);
        let (pw, _) = f64_elems_mut(pred_w);
        if std::ptr::eq(enc, ep as *mut f64)
            || std::ptr::eq(pw, ep as *mut f64)
            || std::ptr::eq(enc, pw)
        {
            trap_abort("lumia: cn learn_generative aliased buffers");
        }
        // Match composed `std.linalg` path bit-for-bit:
        //   scale(keep); pred += lr·(μ⊗err); tmp=π·err; enc += (lr/2)·(err⊗tmp); clamp
        // (Do **not** fuse `(lr/2)·ei·π` — that changes IEEE association vs addmm.)
        let mut err_s = [0.0_f64; MAX_DIM];
        let mut weighted = [0.0_f64; MAX_DIM];
        for i in 0..n {
            let e = *ep.add(i);
            err_s[i] = e;
            weighted[i] = precision * e;
        }
        if weight_decay > 0.0 {
            crate::f64_simd::scale_f64(enc, n * n, keep);
            crate::f64_simd::scale_f64(pw, n * n, keep);
        }
        for i in 0..n {
            let ui = lr * *mp.add(i);
            if ui != 0.0 {
                crate::f64_simd::axpy_scale_f64(pw.add(i * n), err_s.as_ptr(), ui, n);
            }
        }
        for i in 0..n {
            let ui = half * err_s[i];
            if ui != 0.0 {
                crate::f64_simd::axpy_scale_f64(enc.add(i * n), weighted.as_ptr(), ui, n);
            }
        }
        crate::f64_simd::clamp_f64(enc, n * n, lo, hi);
        crate::f64_simd::clamp_f64(pw, n * n, lo, hi);
    }
    enc_w
}

/// Nucleus belief update from a PE (`Nucleus.update_state`).
///
/// ```text
/// δ = enc @ (π · err);  μ += state_lr · δ;  clamp μ
/// ```
/// Mutates `mu` (and leaves `err` unchanged). Returns `mu`.
#[no_mangle]
pub extern "C" fn lumia_cn_update_state(
    mu: *mut u8,
    enc_w: *mut u8,
    err: *mut u8,
    size: i64,
    state_lr: f64,
    precision: f64,
    mu_clip: f64,
) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if size < 1 || size as usize > MAX_DIM {
        trap_abort("lumia: cn update_state size out of range");
    }
    let n = size as usize;
    let enc_w = force_f64(enc_w);
    let err = force_f64(err);
    let mu = ensure_unique_f64(mu);
    require_len(enc_w, size * size, "cn update enc");
    require_len(err, size, "cn update err");
    require_len(mu, size, "cn update mu");
    let lo = -mu_clip;
    let hi = mu_clip;
    unsafe {
        let (ep, _) = f64_elems(err);
        let (enc, _) = f64_elems(enc_w);
        let (mp, _) = f64_elems_mut(mu);
        // Same association as: scratch=π·err; δ=enc@scratch; μ=axpy(μ,lr,δ); clamp(μ)
        let mut scratch = [0.0_f64; MAX_DIM];
        let mut delta = [0.0_f64; MAX_DIM];
        for i in 0..n {
            scratch[i] = precision * *ep.add(i);
        }
        crate::f64_simd::matvec_f64(enc, scratch.as_ptr(), delta.as_mut_ptr(), n, n);
        crate::f64_simd::axpy_scale_f64(mp, delta.as_ptr(), state_lr, n);
        crate::f64_simd::clamp_f64(mp, n, lo, hi);
    }
    mu
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

    #[test]
    fn cluster_rates_square_relu() {
        // size=4, n_clusters=2 → chunk=2; pot0=0, x=[1,0.4, 2,0]
        let pot = from_slice(&[0.0, 0.0, 0.0, 0.0]);
        let x = from_slice(&[1.0, 0.4, 2.0, 0.0]);
        let out = from_slice(&[0.0, 0.0, 0.0, 0.0]);
        let out = lumia_cn_cluster_rates(pot, x, out, 4, 2, 0.9, 0.5);
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
        let enc = lumia_cn_learn_generative(enc, pred, mu, err, size, 1.0, 10.0, 0.0, 1.0);
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
        let mu = lumia_cn_update_state(mu, enc, err, size, 0.5, 1.0, 10.0);
        // δ = err; μ += 0.5·δ → [1, 0]
        assert!((get_f(mu, 0) - 1.0).abs() < 1e-12);
        assert!(get_f(mu, 1).abs() < 1e-12);
    }
}
