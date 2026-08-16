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
            // Preserve Eq-only AssocList. Codegen never stacks LitMap (finish+heap
            // only); small maps use SmallMap, large → default HashOrdered.
            if matches!(*repr, MapRepr::AssocList) {
                // keep
            } else if n_pairs > 0 && n_pairs <= max {
                *repr = MapRepr::SmallMap;
            } else if n_pairs == 0 {
                // Empty → null at emit; SmallMap keeps the “tiny” hint.
                *repr = MapRepr::SmallMap;
            } else {
                *repr = default_map_repr();
            }
            let _ = (flat_pairs, local_ok);
        }
        Value::AllocSet { elems, repr } => {
            // Empty → null at emit. Non-empty always HeapSet+finish (no LitSet stack).
            let _ = (elems, local_ok, max);
            *repr = SetRepr::HeapSet;
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

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_core::{Block, CoreFun, FunKind, Local, Op, Value};
    use lumia_ty::{Effect, Type};
    use rustc_hash::FxHashSet as HashSet;

    fn fun(body: Block, escaping: HashSet<Local>) -> CoreFun {
        CoreFun {
            name: "f".into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            body,
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: lumia_core::ForeignAbi::C,
            escaping,
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        }
    }

    #[test]
    fn empty_list_is_lit_nonempty_set_is_heap() {
        let body = Block {
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::AllocList {
                        elems: vec![],
                        repr: ListRepr::HeapList,
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::AllocSet {
                        elems: vec![Local(1)],
                        repr: SetRepr::HeapSet,
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(0)),
        };
        let mut module = CoreModule::empty("m");
        module.functions.push(fun(body, HashSet::default()));
        ReprSelect.run(&mut module);
        let ops = &module.functions[0].body.ops;
        match &ops[0] {
            Op::Let {
                value: Value::AllocList { repr, .. },
                ..
            } => assert_eq!(*repr, ListRepr::LitList),
            other => panic!("expected AllocList, got {other:?}"),
        }
        match &ops[2] {
            Op::Let {
                value: Value::AllocSet { repr, .. },
                ..
            } => assert_eq!(*repr, SetRepr::HeapSet),
            other => panic!("expected AllocSet, got {other:?}"),
        }
    }

    #[test]
    fn escaping_small_list_stays_heap() {
        let body = Block {
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Int(2),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::AllocList {
                        elems: vec![Local(0), Local(1)],
                        repr: ListRepr::HeapList,
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(2)),
        };
        let mut escaping = HashSet::default();
        escaping.insert(Local(2));
        let mut module = CoreModule::empty("m");
        module.functions.push(fun(body, escaping));
        ReprSelect.run(&mut module);
        match &module.functions[0].body.ops[2] {
            Op::Let {
                value: Value::AllocList { repr, .. },
                ..
            } => assert_eq!(*repr, ListRepr::HeapList),
            other => panic!("expected AllocList, got {other:?}"),
        }
    }
}
