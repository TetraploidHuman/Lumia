//! Expected free energy (EFE) action scoring for CogniNucleus-scale obs (≤16).
//!
//! Mirrors `cogninucleus/efe.py`: kinematic one-step imagination + scalar G(a),
//! with optional two-step lookahead. All work stays in f64 buffers (no host sync).

use crate::common::{trap_abort, GcInhibitGuard};
use crate::list::list_len_of;
use std::ptr;

/// Match GridWorld.ACTIONS: up, down, left, right.
const ACTION_DELTAS: [(f64, f64); 4] = [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)];

/// `scores[a] = G(a)` (lower better). Writes `scores` (len `n_actions`). Returns `scores`.
///
/// Parameters mirror `efe_action_scores` in CogniNucleus (`threat_gain=0` disables threat).
#[no_mangle]
pub extern "C" fn lumia_efe_action_scores(
    obs: *mut u8,
    pred: *mut u8,
    scores: *mut u8,
    n_actions: i64,
    horizon: i64,
    relative_obs_index: i64,
    threat_rel_index: i64,
    cell_step: f64,
    pref_gain: f64,
    epi_gain: f64,
    wall_gain: f64,
    goal_bonus: f64,
    threat_gain: f64,
    complexity: f64,
) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if !(1..=16).contains(&n_actions) {
        trap_abort("lumia: efe n_actions out of range");
    }
    let obs = force_f64(obs);
    let pred = force_f64(pred);
    let scores = ensure_unique_f64(scores);
    let n_obs = list_len_of(obs) as usize;
    let n_pred = list_len_of(pred) as usize;
    require_len(scores, n_actions, "efe scores");
    if n_obs == 0 {
        trap_abort("lumia: efe empty obs");
    }

    let h = if horizon < 1 { 1 } else { horizon };
    let rel = relative_obs_index;
    let trel = threat_rel_index;

    // Scratch on stack — obs dims are tiny (CN default 16).
    let mut scratch = [0.0_f64; 64];
    let mut scratch2 = [0.0_f64; 64];
    if n_obs > scratch.len() {
        trap_abort("lumia: efe obs too long");
    }

    unsafe {
        let (op, _) = f64_elems(obs);
        let (pp, _) = f64_elems(pred);
        let (sp, _) = f64_elems_mut(scores);
        let obs_s = std::slice::from_raw_parts(op, n_obs);
        let pred_s = std::slice::from_raw_parts(pp, n_pred);

        for a in 0..n_actions as usize {
            let bumped = imagine_into(obs_s, a, rel, trel, cell_step, &mut scratch[..n_obs]);
            let g1 = expected_free_energy(
                &scratch[..n_obs],
                pred_s,
                bumped,
                rel,
                trel,
                pref_gain,
                epi_gain,
                wall_gain,
                goal_bonus,
                threat_gain,
                complexity,
                false,
                0.0,
                None,
            );
            if h <= 1 || bumped {
                *sp.add(a) = g1;
                continue;
            }
            let mut best_g2 = 1e9_f64;
            for a2 in 0..n_actions as usize {
                let bumped2 = imagine_into(
                    &scratch[..n_obs],
                    a2,
                    rel,
                    trel,
                    cell_step,
                    &mut scratch2[..n_obs],
                );
                let g2 = expected_free_energy(
                    &scratch2[..n_obs],
                    pred_s,
                    bumped2,
                    rel,
                    trel,
                    pref_gain,
                    epi_gain,
                    wall_gain,
                    goal_bonus,
                    threat_gain,
                    complexity,
                    false,
                    0.0,
                    None,
                );
                if g2 < best_g2 {
                    best_g2 = g2;
                }
            }
            *sp.add(a) = g1 + 0.85 * best_g2;
        }
    }
    scores
}

/// Embodied MiniEcoWorld EFE (`imagine_embodied_obs_after_action` + G(a)).
/// Extra params: `turn_angle`, `fov_range`, `hunger_explore_gain`. Obs dim ≤ 64.
#[no_mangle]
pub extern "C" fn lumia_efe_embodied_action_scores(
    obs: *mut u8,
    pred: *mut u8,
    scores: *mut u8,
    n_actions: i64,
    horizon: i64,
    relative_obs_index: i64,
    threat_rel_index: i64,
    cell_step: f64,
    pref_gain: f64,
    epi_gain: f64,
    wall_gain: f64,
    goal_bonus: f64,
    threat_gain: f64,
    complexity: f64,
    turn_angle: f64,
    fov_range: f64,
    hunger_explore_gain: f64,
) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if !(1..=16).contains(&n_actions) {
        trap_abort("lumia: efe embodied n_actions out of range");
    }
    let obs = force_f64(obs);
    let pred = force_f64(pred);
    let scores = ensure_unique_f64(scores);
    let n_obs = list_len_of(obs) as usize;
    let n_pred = list_len_of(pred) as usize;
    require_len(scores, n_actions, "efe embodied scores");
    if n_obs == 0 {
        trap_abort("lumia: efe embodied empty obs");
    }
    let h = if horizon < 1 { 1 } else { horizon };
    let mut scratch = [0.0_f64; 64];
    let mut scratch2 = [0.0_f64; 64];
    if n_obs > scratch.len() {
        trap_abort("lumia: efe embodied obs too long");
    }
    // Cache turn trig once (actions 1/2 both need ±turn_angle).
    let ca_pos = turn_angle.cos();
    let sa_pos = turn_angle.sin();
    unsafe {
        let (op, _) = f64_elems(obs);
        let (pp, _) = f64_elems(pred);
        let (sp, _) = f64_elems_mut(scores);
        let obs_s = std::slice::from_raw_parts(op, n_obs);
        let pred_s = std::slice::from_raw_parts(pp, n_pred);
        for a in 0..n_actions as usize {
            let bumped = imagine_embodied_into(
                obs_s,
                a,
                relative_obs_index,
                threat_rel_index,
                cell_step,
                ca_pos,
                sa_pos,
                fov_range,
                &mut scratch[..n_obs],
            );
            let g1 = expected_free_energy(
                &scratch[..n_obs],
                pred_s,
                bumped,
                relative_obs_index,
                threat_rel_index,
                pref_gain,
                epi_gain,
                wall_gain,
                goal_bonus,
                threat_gain,
                complexity,
                true,
                hunger_explore_gain,
                Some(a),
            );
            if h <= 1 || bumped {
                *sp.add(a) = g1;
                continue;
            }
            let mut best_g2 = 1e9_f64;
            for a2 in 0..n_actions as usize {
                let bumped2 = imagine_embodied_into(
                    &scratch[..n_obs],
                    a2,
                    relative_obs_index,
                    threat_rel_index,
                    cell_step,
                    ca_pos,
                    sa_pos,
                    fov_range,
                    &mut scratch2[..n_obs],
                );
                let g2 = expected_free_energy(
                    &scratch2[..n_obs],
                    pred_s,
                    bumped2,
                    relative_obs_index,
                    threat_rel_index,
                    pref_gain,
                    epi_gain,
                    wall_gain,
                    goal_bonus,
                    threat_gain,
                    complexity,
                    true,
                    hunger_explore_gain,
                    Some(a2),
                );
                if g2 < best_g2 {
                    best_g2 = g2;
                }
            }
            *sp.add(a) = g1 + 0.85 * best_g2;
        }
    }
    scores
}

fn imagine_embodied_into(
    obs: &[f64],
    action: usize,
    relative_obs_index: i64,
    threat_rel_index: i64,
    cell_step: f64,
    ca_pos: f64,
    sa_pos: f64,
    fov_range: f64,
    out: &mut [f64],
) -> bool {
    out.copy_from_slice(obs);
    let a = action % 4;
    let cos_h = out[0];
    let sin_h = out[1];
    let rel = relative_obs_index as usize;
    let trel = threat_rel_index as usize;
    let rotate_bearing = |out: &mut [f64], base: usize, ca: f64, sa: f64| {
        if out.len() <= base + 1 {
            return;
        }
        let fx = out[base];
        let fy = out[base + 1];
        out[base] = fx * ca - fy * sa;
        out[base + 1] = fx * sa + fy * ca;
    };
    let mut bumped = false;
    match a {
        0 => {
            if out.len() >= 10 {
                let wall = out[6].max(out[7]).max(out[8]).max(out[9]);
                if wall > 0.88 {
                    bumped = true;
                }
            }
            if !bumped {
                if out.len() >= 4 {
                    out[2] = (out[2] + cell_step * cos_h).clamp(0.0, 1.0);
                    out[3] = (out[3] + cell_step * sin_h).clamp(0.0, 1.0);
                }
                if out.len() > rel + 2 {
                    let fi = out[rel + 2];
                    let dist = fi * fov_range;
                    let align = (cos_h * out[rel] + sin_h * out[rel + 1]).max(0.0);
                    let new_dist = (dist - cell_step * align.max(0.15)).max(0.0);
                    out[rel + 2] = (new_dist / fov_range.max(1e-3)).min(1.0);
                }
                if out.len() > trel + 2 {
                    let ti = out[trel + 2];
                    let dist_t = ti * fov_range;
                    let align_t = (cos_h * out[trel] + sin_h * out[trel + 1]).max(0.0);
                    let new_dist_t = (dist_t - cell_step * align_t.max(0.15)).max(0.0);
                    out[trel + 2] = (new_dist_t / fov_range.max(1e-3)).min(1.0);
                }
            }
            if out.len() >= 10 {
                let margin = 0.08;
                let px = out[2];
                let py = out[3];
                out[6] = (1.0 - px / margin).max(0.0);
                out[7] = (1.0 - (1.0 - px) / margin).max(0.0);
                out[8] = (1.0 - py / margin).max(0.0);
                out[9] = (1.0 - (1.0 - py) / margin).max(0.0);
            }
        }
        1 => {
            out[0] = cos_h * ca_pos - sin_h * sa_pos;
            out[1] = sin_h * ca_pos + cos_h * sa_pos;
            rotate_bearing(out, rel, ca_pos, sa_pos);
            rotate_bearing(out, trel, ca_pos, sa_pos);
        }
        2 => {
            // −turn_angle: cos even, sin odd
            let ca = ca_pos;
            let sa = -sa_pos;
            out[0] = cos_h * ca - sin_h * sa;
            out[1] = sin_h * ca + cos_h * sa;
            rotate_bearing(out, rel, ca, sa);
            rotate_bearing(out, trel, ca, sa);
        }
        _ => {}
    }
    bumped
}

fn imagine_into(
    obs: &[f64],
    action: usize,
    relative_obs_index: i64,
    threat_rel_index: i64,
    cell_step: f64,
    out: &mut [f64],
) -> bool {
    out.copy_from_slice(obs);
    let a = action % 4;
    let block_i = (relative_obs_index + 2) as usize;
    if out.len() >= block_i + 4 && out[block_i + a] > 0.5 {
        return true;
    }
    let (dax, day) = ACTION_DELTAS[a];
    let step = cell_step;
    if out.len() >= 2 {
        out[0] = (out[0] + dax * step).clamp(0.0, 1.0);
        out[1] = (out[1] + day * step).clamp(0.0, 1.0);
    }
    let idx = relative_obs_index as usize;
    if out.len() > idx + 1 {
        out[idx] -= dax * step;
        out[idx + 1] -= day * step;
    }
    let tidx = threat_rel_index as usize;
    if out.len() > tidx + 2 {
        out[tidx] -= dax * step;
        out[tidx + 1] -= day * step;
        out[tidx + 2] = 0.0;
    }
    if out.len() >= block_i + 4 {
        for k in 0..4 {
            out[block_i + k] = 0.0;
        }
    }
    false
}

fn expected_free_energy(
    imagined: &[f64],
    pred: &[f64],
    bumped: bool,
    relative_obs_index: i64,
    threat_rel_index: i64,
    pref_gain: f64,
    epi_gain: f64,
    wall_gain: f64,
    goal_bonus: f64,
    threat_gain: f64,
    complexity: f64,
    embodied: bool,
    hunger_explore_gain: f64,
    action: Option<usize>,
) -> f64 {
    let idx = relative_obs_index as usize;
    let dx = if imagined.len() > idx {
        imagined[idx]
    } else {
        0.0
    };
    let dy = if imagined.len() > idx + 1 {
        imagined[idx + 1]
    } else {
        0.0
    };
    let intensity = if imagined.len() > idx + 2 {
        imagined[idx + 2]
    } else {
        0.0
    };
    let dist = if embodied && imagined.len() > idx + 2 {
        (0.0_f64).max(1.0 - intensity)
    } else {
        dx.abs() + dy.abs()
    };
    let pref = dist - goal_bonus * (0.0_f64).max(1.0 - 8.0 * dist);
    let n = imagined.len().min(pred.len());
    let mut s = 0.0_f64;
    for i in 0..n {
        let d = imagined[i] - pred[i];
        s += d * d;
    }
    let epi = s.sqrt();
    let wall = if bumped { 1.0 } else { 0.0 };
    let mut threat = 0.0_f64;
    let tidx = threat_rel_index as usize;
    if threat_gain != 0.0 && imagined.len() > tidx + 2 {
        let tdx = imagined[tidx];
        let tdy = imagined[tidx + 1];
        let local = imagined[tidx + 2];
        threat = local + 0.35 * (0.0_f64).max(0.35 - (tdx.abs() + tdy.abs()));
    }
    let mut homeo = 0.0_f64;
    if embodied && hunger_explore_gain != 0.0 && imagined.len() > 4 {
        if let Some(a) = action {
            let hunger = imagined[4];
            if hunger >= 0.35 {
                let drive = hunger_explore_gain * hunger;
                if a == 0 {
                    homeo = -drive;
                } else if a == 3 {
                    homeo = 0.85 * drive;
                }
            } else if hunger <= 0.25 {
                let rest = hunger_explore_gain * (0.25 - hunger);
                if a == 0 {
                    homeo = 0.55 * rest;
                } else if a == 3 {
                    homeo = -0.70 * rest;
                }
            }
        }
    }
    pref_gain * pref + epi_gain * epi + wall_gain * wall + threat_gain * threat + complexity + homeo
}

fn force_f64(list: *mut u8) -> *mut u8 {
    use crate::list::{force_heap_list, list_float_elems};
    let list = force_heap_list(list);
    if list.is_null() {
        trap_abort("lumia: efe on null list");
    }
    if !list_float_elems(list) {
        trap_abort("lumia: efe expects List[Float]");
    }
    list
}

fn ensure_unique_f64(list: *mut u8) -> *mut u8 {
    use crate::common::{list_rc_is_unique, TYPE_LIST_F64};
    use crate::gc::{list_payload_bytes, lumia_alloc};
    let list = force_f64(list);
    if list_rc_is_unique(list) {
        return list;
    }
    unsafe {
        let n = *(list as *const i64);
        let dest = lumia_alloc(list_payload_bytes(n), TYPE_LIST_F64);
        if dest.is_null() {
            trap_abort("lumia: efe clone OOM");
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
    fn efe_horizon1_finite() {
        // 16-dim obs: agent at (0.5,0.5), rel goal (0.25,0), clear blockers.
        let mut o = vec![0.0; 16];
        o[0] = 0.5;
        o[1] = 0.5;
        o[4] = 0.25;
        o[11] = 0.1;
        o[13] = 0.2;
        let obs = from_slice(&o);
        let mut p = o.clone();
        p[4] = 0.15;
        let pred = from_slice(&p);
        let scores = lumia_list_f64_zeros(4);
        let scores = lumia_efe_action_scores(
            obs, pred, scores, 4, 2, 4, 11, 0.25, 1.2, 0.28, 3.2, 0.8, 2.0, 0.1,
        );
        for a in 0..4 {
            let v = f64::from_bits(lumia_list_get(scores, a) as u64);
            assert!(v.is_finite(), "score[{a}]={v}");
        }
        // Oracle (Python f64 twin of cogninucleus/efe.py): ~[1.392, -0.113, 1.322, 1.322]
        let s0 = f64::from_bits(lumia_list_get(scores, 0) as u64);
        let s1 = f64::from_bits(lumia_list_get(scores, 1) as u64);
        assert!((s0 - 1.3921016919911855).abs() < 1e-9, "s0={s0}");
        assert!((s1 - -0.11301180377457948).abs() < 1e-9, "s1={s1}");
        let _ = bits(0.0);
    }
}
