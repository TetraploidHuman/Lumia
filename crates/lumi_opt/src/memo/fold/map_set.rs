//! Literal Map/Set PE (contains, set, insert).

use lumi_core::{Local, MapRepr, SetRepr, Value};
use lumi_hir::Builtin;

use super::list;
use super::FoldEnv;

pub(super) fn fold(
    env: &mut FoldEnv,
    name: Builtin,
    args: &[Local],
    local: u32,
    value: &mut Value,
) -> bool {
    match (name, args) {
        (Builtin::Contains, [col, key]) => {
            // Only fold when every key/elem is a known Int constant.
            // A non-constant key that happens to equal `k` at runtime
            // must not be folded to `false` (false negative).
            if let Some(k) = env.known_int.get(key.0) {
                if let Some(pairs) = env.known_map.get(&col.0) {
                    let keys: Vec<_> = pairs.as_chunks::<2>().0.iter().map(|kv| kv[0]).collect();
                    if keys.iter().all(|kk| env.known_int.contains(kk.0)) {
                        let found = keys.iter().any(|kk| env.known_int.get(kk.0) == Some(k));
                        *value = Value::Bool(found);
                        env.known_int.insert(local, if found { 1 } else { 0 });
                    }
                } else if let Some(elems) = env.known_set.get(&col.0) {
                    if elems.iter().all(|e| env.known_int.contains(e.0)) {
                        let found = elems.iter().any(|e| env.known_int.get(e.0) == Some(k));
                        *value = Value::Bool(found);
                        env.known_int.insert(local, if found { 1 } else { 0 });
                    }
                }
            }
            true
        }
        (Builtin::MapSet, [col, k, v]) => {
            if let Some(pairs) = env.known_map.get(&col.0).cloned() {
                let keys_known = pairs
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .all(|kv| env.known_int.contains(kv[0].0))
                    && env.known_int.contains(k.0);
                let mut out = Vec::with_capacity(pairs.len() + 2);
                let mut replaced = false;
                for kv in pairs.as_chunks::<2>().0 {
                    let same = kv[0] == *k
                        || (keys_known && env.known_int.get(kv[0].0) == env.known_int.get(k.0));
                    if same && !replaced {
                        out.push(*k);
                        out.push(*v);
                        replaced = true;
                    } else {
                        out.push(kv[0]);
                        out.push(kv[1]);
                    }
                }
                // Inserting a "new" key is only safe when every existing
                // key is a known Int (else a non-const key may collide).
                // Empty-map insert also requires a known Int key so we
                // do not PE Float/ADT keys into LitMap incorrectly.
                if replaced {
                    *value = Value::AllocMap {
                        flat_pairs: out.clone(),
                        repr: MapRepr::LitMap,
                    };
                    env.known_map.insert(local, out);
                } else if keys_known || (pairs.is_empty() && env.known_int.contains(k.0)) {
                    out.push(*k);
                    out.push(*v);
                    *value = Value::AllocMap {
                        flat_pairs: out.clone(),
                        repr: MapRepr::LitMap,
                    };
                    env.known_map.insert(local, out);
                }
                return true;
            }
            let _ = list::fold_list_set(env, local, *col, *k, *v, value);
            true
        }
        (Builtin::SetInsert, [set, elem]) => {
            if let Some(elems) = env.known_set.get(&set.0).cloned() {
                let elems_known = elems.iter().all(|e| env.known_int.contains(e.0))
                    && env.known_int.contains(elem.0);
                let already = elems.iter().any(|e| {
                    *e == *elem
                        || (elems_known && env.known_int.get(e.0) == env.known_int.get(elem.0))
                });
                if already || (elems.is_empty() && env.known_int.contains(elem.0)) || elems_known {
                    let mut neu = elems;
                    if !already {
                        neu.push(*elem);
                    }
                    *value = Value::AllocSet {
                        elems: neu.clone(),
                        repr: SetRepr::LitSet,
                    };
                    env.known_set.insert(local, neu);
                }
            }
            true
        }
        _ => false,
    }
}
