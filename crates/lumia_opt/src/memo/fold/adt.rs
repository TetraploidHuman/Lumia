//! Literal ADT field/tag PE.

use lumia_core::{Local, Value};
use lumia_hir::Builtin;

use super::FoldEnv;

pub(super) fn fold(
    env: &mut FoldEnv,
    name: Builtin,
    args: &[Local],
    local: u32,
    value: &mut Value,
) -> bool {
    match (name, args) {
        (Builtin::AdtField, [adt, idx, ..]) => {
            if let (Some(fields), Some(i)) = (env.known_adt.get(&adt.0), env.known_int.get(idx.0)) {
                if i >= 0 && (i as usize) < fields.len() {
                    let el = fields[i as usize];
                    *value = Value::Local(el);
                    if let Some(n) = env.known_int.get(el.0) {
                        env.known_int.insert(local, n);
                    }
                    if let Some(inner) = env.known_list.get(&el.0).cloned() {
                        env.known_list.insert(local, inner);
                    }
                    if let Some(inner) = env.known_adt.get(&el.0).cloned() {
                        env.known_adt.insert(local, inner);
                    }
                    if let Some(&tag) = env.known_adt_tag.get(&el.0) {
                        env.known_adt_tag.insert(local, tag);
                    }
                }
            }
            true
        }
        (Builtin::AdtTag, [adt]) => {
            if let Some(&tag) = env.known_adt_tag.get(&adt.0) {
                *value = Value::Int(tag);
                env.known_int.insert(local, tag);
            }
            true
        }
        _ => false,
    }
}
