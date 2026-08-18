// Extracted from production module (Todo: RT 测例半迁).
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
    let scores = unsafe {
        lumia_efe_action_scores(
            obs, pred, scores, 4, 2, 4, 11, 0.25, 1.2, 0.28, 3.2, 0.8, 2.0, 0.1,
        )
    };
    for a in 0..4 {
        let v = f64::from_bits(unsafe { lumia_list_get(scores, a) } as u64);
        assert!(v.is_finite(), "score[{a}]={v}");
    }
    // Oracle (Python f64 twin of cogninucleus/efe.py): ~[1.392, -0.113, 1.322, 1.322]
    let s0 = f64::from_bits(unsafe { lumia_list_get(scores, 0) } as u64);
    let s1 = f64::from_bits(unsafe { lumia_list_get(scores, 1) } as u64);
    assert!((s0 - 1.3921016919911855).abs() < 1e-9, "s0={s0}");
    assert!((s1 - -0.11301180377457948).abs() < 1e-9, "s1={s1}");
    let _ = bits(0.0);
}
