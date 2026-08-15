//! Shared helpers for const-fold PE of literal collections.

use lumia_core::{AdtRepr, ListRepr, Local, Value};

use super::FoldEnv;

pub(super) fn all_int_keys(env: &FoldEnv, keys: &[Local]) -> bool {
    keys.iter().all(|k| env.known_int.contains(k.0))
}

/// Float keys collide under RT `float_key_eq` (±0 equal; NaN never equals).
fn float_key_eq_bits(a: u64, b: u64) -> bool {
    if a == b {
        return true;
    }
    let fa = f64::from_bits(a);
    let fb = f64::from_bits(b);
    fa == 0.0 && fb == 0.0
}

fn locals_struct_eq(env: &FoldEnv, a: Local, b: Local) -> bool {
    if a.0 == b.0 {
        return true;
    }
    if let (Some(x), Some(y)) = (env.known_int.get(a.0), env.known_int.get(b.0)) {
        return x == y;
    }
    if let (Some(&x), Some(&y)) = (env.known_float.get(&a.0), env.known_float.get(&b.0)) {
        return float_key_eq_bits(x, y);
    }
    if let (Some(x), Some(y)) = (env.known_string.get(&a.0), env.known_string.get(&b.0)) {
        return x == y;
    }
    if let (Some(&ta), Some(&tb)) = (env.known_adt_tag.get(&a.0), env.known_adt_tag.get(&b.0)) {
        if ta != tb {
            return false;
        }
        let fa = env.known_adt.get(&a.0).map(|v| v.as_slice()).unwrap_or(&[]);
        let fb = env.known_adt.get(&b.0).map(|v| v.as_slice()).unwrap_or(&[]);
        if fa.len() != fb.len() {
            return false;
        }
        return fa
            .iter()
            .zip(fb.iter())
            .all(|(x, y)| locals_struct_eq(env, *x, *y));
    }
    false
}

pub(super) fn compact_float_map_pairs(flat_pairs: &mut Vec<Local>, env: &FoldEnv) {
    if flat_pairs.len() < 2 || flat_pairs.len() % 2 != 0 {
        return;
    }
    let keys: Vec<_> = flat_pairs.iter().step_by(2).copied().collect();
    if !keys.iter().all(|k| env.known_float.contains_key(&k.0)) {
        return;
    }
    let mut out: Vec<Local> = Vec::with_capacity(flat_pairs.len());
    for chunk in flat_pairs.chunks_exact(2) {
        let (k, v) = (chunk[0], chunk[1]);
        let bits = env.known_float[&k.0];
        if let Some(i) = (0..out.len() / 2).find(|&i| {
            float_key_eq_bits(env.known_float[&out[i * 2].0], bits)
        }) {
            out[i * 2 + 1] = v;
        } else {
            out.push(k);
            out.push(v);
        }
    }
    *flat_pairs = out;
}

pub(super) fn compact_float_set_elems(elems: &mut Vec<Local>, env: &FoldEnv) {
    if elems.len() < 2 {
        return;
    }
    if !elems.iter().all(|e| env.known_float.contains_key(&e.0)) {
        return;
    }
    let mut out: Vec<Local> = Vec::with_capacity(elems.len());
    for e in elems.iter().copied() {
        let bits = env.known_float[&e.0];
        if !out
            .iter()
            .any(|k| float_key_eq_bits(env.known_float[&k.0], bits))
        {
            out.push(e);
        }
    }
    *elems = out;
}

/// Int/Bool/Char literal keys (same i64 bits).
pub(super) fn compact_int_map_pairs(flat_pairs: &mut Vec<Local>, env: &FoldEnv) {
    if flat_pairs.len() < 2 || flat_pairs.len() % 2 != 0 {
        return;
    }
    let keys: Vec<_> = flat_pairs.iter().step_by(2).copied().collect();
    if !all_int_keys(env, &keys) {
        return;
    }
    let mut out: Vec<Local> = Vec::with_capacity(flat_pairs.len());
    for chunk in flat_pairs.chunks_exact(2) {
        let (k, v) = (chunk[0], chunk[1]);
        let bits = env.known_int.get(k.0).expect("all_int_keys");
        if let Some(i) =
            (0..out.len() / 2).find(|&i| env.known_int.get(out[i * 2].0) == Some(bits))
        {
            out[i * 2 + 1] = v;
        } else {
            out.push(k);
            out.push(v);
        }
    }
    *flat_pairs = out;
}

pub(super) fn compact_int_set_elems(elems: &mut Vec<Local>, env: &FoldEnv) {
    if elems.len() < 2 {
        return;
    }
    if !elems.iter().all(|e| env.known_int.contains(e.0)) {
        return;
    }
    let mut out: Vec<Local> = Vec::with_capacity(elems.len());
    for e in elems.iter().copied() {
        let bits = env.known_int.get(e.0).expect("known_int");
        if !out.iter().any(|k| env.known_int.get(k.0) == Some(bits)) {
            out.push(e);
        }
    }
    *elems = out;
}

pub(super) fn compact_string_map_pairs(flat_pairs: &mut Vec<Local>, env: &FoldEnv) {
    if flat_pairs.len() < 2 || flat_pairs.len() % 2 != 0 {
        return;
    }
    let keys: Vec<_> = flat_pairs.iter().step_by(2).copied().collect();
    if !keys.iter().all(|k| env.known_string.contains_key(&k.0)) {
        return;
    }
    let mut out: Vec<Local> = Vec::with_capacity(flat_pairs.len());
    for chunk in flat_pairs.chunks_exact(2) {
        let (k, v) = (chunk[0], chunk[1]);
        let s = &env.known_string[&k.0];
        if let Some(i) = (0..out.len() / 2).find(|&i| env.known_string[&out[i * 2].0] == *s) {
            out[i * 2 + 1] = v;
        } else {
            out.push(k);
            out.push(v);
        }
    }
    *flat_pairs = out;
}

pub(super) fn compact_string_set_elems(elems: &mut Vec<Local>, env: &FoldEnv) {
    if elems.len() < 2 {
        return;
    }
    if !elems.iter().all(|e| env.known_string.contains_key(&e.0)) {
        return;
    }
    let mut out: Vec<Local> = Vec::with_capacity(elems.len());
    for e in elems.iter().copied() {
        let s = &env.known_string[&e.0];
        if !out.iter().any(|k| env.known_string[&k.0] == *s) {
            out.push(e);
        }
    }
    *elems = out;
}

/// User ADT literals with known tags/fields (e.g. unit `P`).
pub(super) fn compact_adt_map_pairs(flat_pairs: &mut Vec<Local>, env: &FoldEnv) {
    if flat_pairs.len() < 2 || flat_pairs.len() % 2 != 0 {
        return;
    }
    let keys: Vec<_> = flat_pairs.iter().step_by(2).copied().collect();
    if !keys.iter().all(|k| env.known_adt_tag.contains_key(&k.0)) {
        return;
    }
    let mut out: Vec<Local> = Vec::with_capacity(flat_pairs.len());
    for chunk in flat_pairs.chunks_exact(2) {
        let (k, v) = (chunk[0], chunk[1]);
        if let Some(i) = (0..out.len() / 2).find(|&i| locals_struct_eq(env, out[i * 2], k)) {
            out[i * 2 + 1] = v;
        } else {
            out.push(k);
            out.push(v);
        }
    }
    *flat_pairs = out;
}

pub(super) fn compact_adt_set_elems(elems: &mut Vec<Local>, env: &FoldEnv) {
    if elems.len() < 2 {
        return;
    }
    if !elems.iter().all(|e| env.known_adt_tag.contains_key(&e.0)) {
        return;
    }
    let mut out: Vec<Local> = Vec::with_capacity(elems.len());
    for e in elems.iter().copied() {
        if !out.iter().any(|k| locals_struct_eq(env, *k, e)) {
            out.push(e);
        }
    }
    *elems = out;
}

/// Compact literal map/set keys before `.len()` / get PE (matches RT finish).
pub(super) fn compact_map_pairs(flat_pairs: &mut Vec<Local>, env: &FoldEnv) {
    compact_float_map_pairs(flat_pairs, env);
    compact_int_map_pairs(flat_pairs, env);
    compact_string_map_pairs(flat_pairs, env);
    compact_adt_map_pairs(flat_pairs, env);
}

pub(super) fn compact_set_elems(elems: &mut Vec<Local>, env: &FoldEnv) {
    compact_float_set_elems(elems, env);
    compact_int_set_elems(elems, env);
    compact_string_set_elems(elems, env);
    compact_adt_set_elems(elems, env);
}

pub(super) fn alloc_option_some(v: Local) -> Value {
    Value::AllocAdt {
        adt_name: "Option".into(),
        tag: 0,
        fields: vec![v],
        repr: AdtRepr::LitAdt,
    }
}

pub(super) fn alloc_option_none() -> Value {
    Value::AllocAdt {
        adt_name: "Option".into(),
        tag: 1,
        fields: vec![],
        repr: AdtRepr::LitAdt,
    }
}

pub(super) fn rewrite_lit_list(
    env: &mut FoldEnv,
    local: u32,
    elems: Vec<Local>,
    value: &mut Value,
) {
    *value = Value::AllocList {
        elems: elems.clone(),
        repr: ListRepr::LitList,
    };
    env.known_list.insert(local, elems);
}
