//! Literal list PE (len/get/concat/append/take/slice/reverse/set).

use lumi_core::{Local, Value};
use lumi_hir::Builtin;

use super::helpers::{all_int_keys, alloc_option_none, alloc_option_some, rewrite_lit_list};
use super::{iota, FoldEnv};

pub(super) fn fold(
    env: &mut FoldEnv,
    name: Builtin,
    args: &[Local],
    local: u32,
    value: &mut Value,
) -> bool {
    match (name, args) {
        (Builtin::ListLen, [xs]) => {
            if let Some(elems) = env.known_list.get(&xs.0) {
                let n = elems.len() as i64;
                *value = Value::Int(n);
                env.known_int.insert(local, n);
                return true;
            }
            if iota::fold_iota_len(env, local, *xs, value) {
                return true;
            }
            if let Some(pairs) = env.known_map.get(&xs.0) {
                let n = (pairs.len() / 2) as i64;
                *value = Value::Int(n);
                env.known_int.insert(local, n);
                return true;
            }
            if let Some(elems) = env.known_set.get(&xs.0) {
                let n = elems.len() as i64;
                *value = Value::Int(n);
                env.known_int.insert(local, n);
                return true;
            }
            true
        }
        (Builtin::ListGet, [xs, idx]) => {
            if let (Some(elems), Some(i)) = (env.known_list.get(&xs.0), env.known_int.get(idx.0)) {
                if i >= 0 && (i as usize) < elems.len() {
                    let el = elems[i as usize];
                    *value = Value::Local(el);
                    if let Some(n) = env.known_int.get(el.0) {
                        env.known_int.insert(local, n);
                    }
                    if let Some(inner) = env.known_list.get(&el.0).cloned() {
                        env.known_list.insert(local, inner);
                    }
                    return true;
                }
            }
            if iota::fold_iota_get(env, local, *xs, *idx, value) {
                return true;
            }
            // Map.get → Option (ListGet is also used for Map.get).
            if let (Some(pairs), Some(k)) =
                (env.known_map.get(&xs.0).cloned(), env.known_int.get(idx.0))
            {
                let keys: Vec<_> = pairs.as_chunks::<2>().0.iter().map(|kv| kv[0]).collect();
                if all_int_keys(env, &keys) {
                    let found = keys.iter().enumerate().find_map(|(i, kk)| {
                        if env.known_int.get(kk.0) == Some(k) {
                            Some(pairs[i * 2 + 1])
                        } else {
                            None
                        }
                    });
                    match found {
                        Some(v) => {
                            *value = alloc_option_some(v);
                            env.known_adt.insert(local, vec![v]);
                        }
                        None => {
                            *value = alloc_option_none();
                            env.known_adt.insert(local, vec![]);
                        }
                    }
                }
            }
            true
        }
        (Builtin::ListConcat, [a, b]) => {
            if let (Some(la), Some(lb)) = (env.known_list.get(&a.0), env.known_list.get(&b.0)) {
                let mut merged = la.clone();
                merged.extend_from_slice(lb);
                rewrite_lit_list(env, local, merged, value);
            }
            true
        }
        (Builtin::ListAppend, [xs, x]) => {
            if let Some(elems) = env.known_list.get(&xs.0) {
                let mut merged = elems.clone();
                merged.push(*x);
                rewrite_lit_list(env, local, merged, value);
            }
            true
        }
        (Builtin::ListTake, [xs, n]) => {
            if let (Some(elems), Some(k)) = (env.known_list.get(&xs.0), env.known_int.get(n.0)) {
                if k >= 0 {
                    let take: Vec<_> = elems.iter().take(k as usize).copied().collect();
                    rewrite_lit_list(env, local, take, value);
                    return true;
                }
            }
            let _ = iota::track_iota_take(env, local, *xs, *n);
            true
        }
        (Builtin::ListSlice, [xs, n]) => {
            // `slice`/`drop`: drop the first `n` elements.
            if let (Some(elems), Some(k)) = (env.known_list.get(&xs.0), env.known_int.get(n.0)) {
                if k >= 0 {
                    let drop_n = (k as usize).min(elems.len());
                    let rest: Vec<_> = elems[drop_n..].to_vec();
                    rewrite_lit_list(env, local, rest, value);
                    return true;
                }
            }
            let _ = iota::track_iota_slice(env, local, *xs, *n);
            true
        }
        (Builtin::ListReverse, [xs]) => {
            if let Some(elems) = env.known_list.get(&xs.0) {
                let mut rev = elems.clone();
                rev.reverse();
                rewrite_lit_list(env, local, rev, value);
            }
            true
        }
        _ => false,
    }
}

/// List.set(index, elem) arm of MapSet.
pub(super) fn fold_list_set(
    env: &mut FoldEnv,
    local: u32,
    col: Local,
    k: Local,
    v: Local,
    value: &mut Value,
) -> bool {
    if let (Some(elems), Some(i)) = (env.known_list.get(&col.0), env.known_int.get(k.0)) {
        if i >= 0 && (i as usize) < elems.len() {
            let mut neu = elems.clone();
            neu[i as usize] = v;
            rewrite_lit_list(env, local, neu, value);
            return true;
        }
    }
    false
}
