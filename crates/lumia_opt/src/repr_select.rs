//! Representation selection: prove → specialize; else default (§7.1.1).

use crate::default_map_repr;
use lumia_abi::SMALL_CONTAINER_MAX;
use lumia_core::{AdtRepr, CoreFun, CoreModule, ListRepr, Local, MapRepr, Op, SetRepr, Value};
use rustc_hash::FxHashSet as HashSet;

/// Representation selection: prove → specialize; else default (§7.1.1).
pub(crate) struct ReprSelect;
impl ReprSelect {
    pub(crate) fn run(self, module: &mut CoreModule) {
        for f in &mut module.functions {
            // EscapePass fills `f.escaping` and must run in the same pipeline
            // pair immediately before ReprSelect (see DEBUG/RELEASE_PASSES).
            let escaping = f.escaping.clone();
            select_in_fun(f, &escaping);
        }
    }
}

fn select_in_fun(f: &mut CoreFun, escaping: &HashSet<Local>) {
    for op in &mut f.body.ops {
        if let Op::Let { local, value, .. } = op {
            select_value(value, *local, escaping);
        }
    }
}

fn select_value(v: &mut Value, bound: Local, escaping: &HashSet<Local>) {
    let local_ok = !escaping.contains(&bound);
    let max = SMALL_CONTAINER_MAX;
    match v {
        Value::AllocList { elems, repr } => {
            if elems.is_empty() {
                // Empty → immortal singleton (`lumia_list_empty`).
                *repr = ListRepr::LitList;
            } else if local_ok && elems.len() <= max {
                // Non-escaping small literal → stack layout in codegen (DESIGN §7).
                *repr = ListRepr::LitList;
            } else {
                *repr = ListRepr::HeapList;
            }
        }
        Value::AllocMap { flat_pairs, repr } => {
            let n_pairs = flat_pairs.len() / 2;
            // Preserve Eq-only AssocList; else stack LitMap when non-escaping ≤max.
            if matches!(*repr, MapRepr::AssocList) {
                // keep
            } else if local_ok && n_pairs > 0 && n_pairs <= max {
                *repr = MapRepr::LitMap;
            } else if n_pairs <= max {
                *repr = MapRepr::SmallMap;
            } else {
                *repr = default_map_repr();
            }
            let _ = flat_pairs;
        }
        Value::AllocSet { elems, repr } => {
            if local_ok && !elems.is_empty() && elems.len() <= max {
                *repr = SetRepr::LitSet;
            } else {
                *repr = SetRepr::HeapSet;
            }
        }
        Value::AllocAdt { fields, repr, .. } => {
            if local_ok && fields.len() <= max {
                *repr = AdtRepr::LitAdt;
            } else {
                *repr = AdtRepr::HeapAdt;
            }
        }
        Value::AllocClosure { .. } | Value::ClosureCap { .. } => {}
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            for op in then_block.ops.iter_mut().chain(else_block.ops.iter_mut()) {
                if let Op::Let { local, value, .. } = op {
                    select_value(value, *local, escaping);
                }
            }
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            for b in [&mut **header, &mut **body, &mut **latch] {
                for op in &mut b.ops {
                    if let Op::Let { local, value, .. } = op {
                        select_value(value, *local, escaping);
                    }
                }
            }
        }
        _ => {}
    }
}
