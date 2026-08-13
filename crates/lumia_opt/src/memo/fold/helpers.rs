//! Shared helpers for const-fold PE of literal collections.

use lumia_core::{AdtRepr, ListRepr, Local, Value};

use super::FoldEnv;

pub(super) fn all_int_keys(env: &FoldEnv, keys: &[Local]) -> bool {
    keys.iter().all(|k| env.known_int.contains(k.0))
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
