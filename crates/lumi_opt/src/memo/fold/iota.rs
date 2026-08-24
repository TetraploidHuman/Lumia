//! Virtual iota PE from `range` / `range_inclusive` / take / slice.

use lumi_core::{Local, Value};
use lumi_hir::Builtin;

use super::FoldEnv;

pub(super) fn fold(
    env: &mut FoldEnv,
    name: Builtin,
    args: &[Local],
    local: u32,
    _value: &mut Value,
) -> bool {
    match (name, args) {
        (Builtin::Range, [lo, hi]) => {
            if let (Some(s), Some(e)) = (env.known_int.get(lo.0), env.known_int.get(hi.0)) {
                if e >= s {
                    env.known_iota.insert(local, (s, e));
                }
            }
            true
        }
        (Builtin::RangeInclusive, [lo, hi]) => {
            if let (Some(s), Some(e)) = (env.known_int.get(lo.0), env.known_int.get(hi.0)) {
                if let Some(end) = e.checked_add(1) {
                    if end >= s {
                        env.known_iota.insert(local, (s, end));
                    }
                }
            }
            true
        }
        _ => false,
    }
}

/// Track iota after take/slice without forcing (keep Builtin).
pub(super) fn track_iota_take(env: &mut FoldEnv, local: u32, xs: Local, n: Local) -> bool {
    if let (Some(&(s, e)), Some(k)) = (env.known_iota.get(&xs.0), env.known_int.get(n.0)) {
        if k >= 0 {
            let take_end = s.saturating_add(k).min(e);
            env.known_iota.insert(local, (s, take_end));
            return true;
        }
    }
    false
}

pub(super) fn track_iota_slice(env: &mut FoldEnv, local: u32, xs: Local, n: Local) -> bool {
    if let (Some(&(s, e)), Some(k)) = (env.known_iota.get(&xs.0), env.known_int.get(n.0)) {
        if k >= 0 {
            let start = s.saturating_add(k).min(e);
            env.known_iota.insert(local, (start, e));
            return true;
        }
    }
    false
}

pub(super) fn fold_iota_len(env: &mut FoldEnv, local: u32, xs: Local, value: &mut Value) -> bool {
    if let Some(&(s, e)) = env.known_iota.get(&xs.0) {
        let n = e.saturating_sub(s);
        *value = Value::Int(n);
        env.known_int.insert(local, n);
        true
    } else {
        false
    }
}

pub(super) fn fold_iota_get(
    env: &mut FoldEnv,
    local: u32,
    xs: Local,
    idx: Local,
    value: &mut Value,
) -> bool {
    if let (Some(&(s, e)), Some(i)) = (env.known_iota.get(&xs.0), env.known_int.get(idx.0)) {
        let n = e.saturating_sub(s);
        if i >= 0 && i < n {
            if let Some(v) = s.checked_add(i) {
                *value = Value::Int(v);
                env.known_int.insert(local, v);
                return true;
            }
        }
    }
    false
}
