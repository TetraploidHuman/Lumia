//! Representation selection: prove → specialize; else default (§7.1.1).
//!
//! Proof sources today:
//! - Escape analysis (`CoreFun::escaping`)
//! - Size thresholds (≤8 → Lit* / Small*)
//! - Key Hash capability from SSA (`use_summary::prove_*`)
//! - Use-pattern summary (`use_summary::summarize_fun`) — only hard facts
//!
//! Not selected here (runtime / HIR defaults): Iota, List COW, Map Overlay,
//! SortedTree (needs stronger Ord+scan proof — not yet).

use crate::use_summary::{
    collect_let_defs, prove_all_keys_no_hash, summarize_fun, LocalUse,
};
use crate::{default_map_repr, Pass};
use lumi_core::{AdtRepr, CoreFun, CoreModule, ListRepr, Local, MapRepr, Op, SetRepr, Value};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Representation selection: prove → specialize; else default (§7.1.1).
pub(crate) struct ReprSelect;
impl Pass for ReprSelect {
    fn name(&self) -> &str {
        "repr_select"
    }
    fn run(&self, module: &mut CoreModule) {
        let hash_adts = module.hash_adts.clone();
        for f in &mut module.functions {
            // EscapePass fills `f.escaping` and must run in the same pipeline
            // pair immediately before ReprSelect (see DEBUG/RELEASE_PASSES).
            let escaping = f.escaping.clone();
            let uses = summarize_fun(f);
            let defs = collect_let_defs(f);
            select_in_fun(f, &escaping, &uses, &defs, &hash_adts);
        }
    }
}

fn select_in_fun(
    f: &mut CoreFun,
    escaping: &HashSet<Local>,
    uses: &HashMap<Local, LocalUse>,
    defs: &HashMap<u32, Value>,
    hash_adts: &HashSet<String>,
) {
    for op in &mut f.body.ops {
        if let Op::Let { local, value, .. } = op {
            select_value(value, *local, escaping, uses, defs, hash_adts);
        }
    }
}

fn select_value(
    v: &mut Value,
    bound: Local,
    escaping: &HashSet<Local>,
    uses: &HashMap<Local, LocalUse>,
    defs: &HashMap<u32, Value>,
    hash_adts: &HashSet<String>,
) {
    let local_ok = !escaping.contains(&bound);
    let use_u = uses.get(&bound).copied().unwrap_or_default();
    match v {
        Value::AllocList { elems, repr } => {
            if elems.is_empty() {
                // Empty → immortal singleton (`lumi_list_empty`).
                *repr = ListRepr::LitList;
            } else if local_ok && elems.len() <= 8 {
                // Non-escaping small literal → stack layout in codegen (DESIGN §7).
                // Read-only proof is optional confirmation; mutation still OK via
                // promote-on-write at runtime.
                *repr = ListRepr::LitList;
                let _ = use_u.read_only_list();
            } else {
                *repr = ListRepr::HeapList;
            }
        }
        Value::AllocMap { flat_pairs, repr } => {
            let n_pairs = flat_pairs.len() / 2;
            // Proven: every key is an ADT without `instance Hash` → AssocList
            // forever (DESIGN §3.5.1). Prefer stack LitMap when non-escaping ≤8
            // (codegen still tags TID_ASSOC from key types).
            let no_hash_keys = prove_all_keys_no_hash(flat_pairs, defs, hash_adts);
            if matches!(*repr, MapRepr::AssocList) {
                // keep
            } else if local_ok && n_pairs > 0 && n_pairs <= 8 {
                *repr = MapRepr::LitMap;
            } else if no_hash_keys {
                // Escaping or large Eq-only — never HashOrdered / finish.
                *repr = MapRepr::AssocList;
            } else if n_pairs <= 8 {
                // Escaping but small + Hash-capable keys: stay linear until growth.
                *repr = MapRepr::SmallMap;
            } else if local_ok && use_u.lookup_only_map() {
                // Proven read-only (get/contains only): skip HashOrdered finish
                // at codegen — linear scan is enough (DESIGN §7.1.1 BuildFused-lite).
                *repr = MapRepr::SmallMap;
            } else {
                *repr = default_map_repr();
            }
        }
        Value::AllocSet { elems, repr } => {
            if local_ok && !elems.is_empty() && elems.len() <= 8 {
                *repr = SetRepr::LitSet;
            } else {
                *repr = SetRepr::HeapSet;
            }
        }
        Value::AllocAdt { fields, repr, .. } => {
            if local_ok && fields.len() <= 8 {
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
                    select_value(value, *local, escaping, uses, defs, hash_adts);
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
                        select_value(value, *local, escaping, uses, defs, hash_adts);
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
    use crate::Pass;
    use lumi_core::{Block, CoreModule};
    use lumi_ty::{Effect, Type};

    fn module_with(fun: CoreFun, hash_adts: HashSet<String>) -> CoreModule {
        let mut m = CoreModule::with_functions("t", vec![fun]);
        m.hash_adts = hash_adts;
        m
    }

    fn base_fun(body: Block) -> CoreFun {
        CoreFun {
            name: "main".into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            body,
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: true,
            memo: None,
            external: None,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
        }
    }

    #[test]
    fn no_hash_adt_keys_select_assoc_even_when_large() {
        // 9 pairs of Point keys (no Hash) → AssocList, not HashOrdered.
        let mut ops = Vec::new();
        let mut pairs = Vec::new();
        for i in 0..9u32 {
            let k = Local(i * 2);
            let v = Local(i * 2 + 1);
            ops.push(Op::Let {
                local: k,
                value: Value::AllocAdt {
                    adt_name: "Point".into(),
                    tag: 0,
                    fields: vec![],
                    repr: AdtRepr::HeapAdt,
                },
                pure_region: true,
            });
            ops.push(Op::Let {
                local: v,
                value: Value::Int(i as i64),
                pure_region: true,
            });
            pairs.push(k);
            pairs.push(v);
        }
        let map_l = Local(20);
        ops.push(Op::Let {
            local: map_l,
            value: Value::AllocMap {
                flat_pairs: pairs,
                repr: MapRepr::HashOrdered,
            },
            pure_region: true,
        });
        let mut fun = base_fun(Block {
            params: vec![],
            ops,
            result: Some(map_l),
        });
        fun.escaping.insert(map_l);
        let mut module = module_with(fun, HashSet::default());
        ReprSelect.run(&mut module);
        let found = module.functions[0].body.ops.iter().find_map(|op| match op {
            Op::Let {
                value: Value::AllocMap { repr, .. },
                ..
            } => Some(*repr),
            _ => None,
        });
        assert_eq!(found, Some(MapRepr::AssocList));
    }

    #[test]
    fn lookup_only_large_map_stays_small_not_hash() {
        let mut ops = Vec::new();
        let mut pairs = Vec::new();
        for i in 0..9u32 {
            let k = Local(i * 2);
            let v = Local(i * 2 + 1);
            ops.push(Op::Let {
                local: k,
                value: Value::Int(i as i64),
                pure_region: true,
            });
            ops.push(Op::Let {
                local: v,
                value: Value::Int(i as i64 * 10),
                pure_region: true,
            });
            pairs.push(k);
            pairs.push(v);
        }
        let map_l = Local(20);
        let key_l = Local(21);
        let got_l = Local(22);
        ops.push(Op::Let {
            local: map_l,
            value: Value::AllocMap {
                flat_pairs: pairs,
                repr: MapRepr::HashOrdered,
            },
            pure_region: true,
        });
        ops.push(Op::Let {
            local: key_l,
            value: Value::Int(2),
            pure_region: true,
        });
        ops.push(Op::Let {
            local: got_l,
            value: Value::Builtin {
                name: lumi_hir::Builtin::ListGet,
                args: vec![map_l, key_l],
            },
            pure_region: true,
        });
        let fun = base_fun(Block {
            params: vec![],
            ops,
            result: Some(got_l),
        });
        let mut module = module_with(fun, HashSet::default());
        ReprSelect.run(&mut module);
        let found = module.functions[0].body.ops.iter().find_map(|op| match op {
            Op::Let {
                value: Value::AllocMap { repr, .. },
                ..
            } => Some(*repr),
            _ => None,
        });
        assert_eq!(found, Some(MapRepr::SmallMap));
    }

    #[test]
    fn hash_adt_keys_large_stay_hash_ordered() {
        let mut ops = Vec::new();
        let mut pairs = Vec::new();
        for i in 0..9u32 {
            let k = Local(i * 2);
            let v = Local(i * 2 + 1);
            ops.push(Op::Let {
                local: k,
                value: Value::AllocAdt {
                    adt_name: "Point".into(),
                    tag: 0,
                    fields: vec![],
                    repr: AdtRepr::HeapAdt,
                },
                pure_region: true,
            });
            ops.push(Op::Let {
                local: v,
                value: Value::Int(i as i64),
                pure_region: true,
            });
            pairs.push(k);
            pairs.push(v);
        }
        let map_l = Local(20);
        ops.push(Op::Let {
            local: map_l,
            value: Value::AllocMap {
                flat_pairs: pairs,
                repr: MapRepr::HashOrdered,
            },
            pure_region: true,
        });
        let mut fun = base_fun(Block {
            params: vec![],
            ops,
            result: Some(map_l),
        });
        fun.escaping.insert(map_l);
        let mut hash_adts = HashSet::default();
        hash_adts.insert("Point".into());
        let mut module = module_with(fun, hash_adts);
        ReprSelect.run(&mut module);
        let found = module.functions[0].body.ops.iter().find_map(|op| match op {
            Op::Let {
                value: Value::AllocMap { repr, .. },
                ..
            } => Some(*repr),
            _ => None,
        });
        assert_eq!(found, Some(MapRepr::HashOrdered));
    }
}
