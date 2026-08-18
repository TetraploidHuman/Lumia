//! Representation selection: prove → specialize; else default (§7.1.1).

use crate::default_map_repr;
use lumia_abi::SMALL_CONTAINER_MAX;
use lumia_core::{
    AdtRepr, Block, CoreFun, CoreModule, ListRepr, Local, MapRepr, Op, SetRepr, Value,
};
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
    select_in_block(&mut f.body, escaping);
}

fn select_in_block(block: &mut Block, escaping: &HashSet<Local>) {
    for op in &mut block.ops {
        if let Op::Let { local, value, .. } = op {
            select_alloc_repr(value, *local, escaping);
            // Enter If/Loop only — Lambda skipped (same as prior `_ => {}`).
            match value {
                Value::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    select_in_block(then_block, escaping);
                    select_in_block(else_block, escaping);
                }
                Value::Loop {
                    header,
                    body,
                    latch,
                } => {
                    select_in_block(header, escaping);
                    select_in_block(body, escaping);
                    select_in_block(latch, escaping);
                }
                _ => {}
            }
        }
    }
}

fn select_alloc_repr(v: &mut Value, bound: Local, escaping: &HashSet<Local>) {
    let local_ok = !escaping.contains(&bound);
    let max = SMALL_CONTAINER_MAX;
    match v {
        Value::AllocList { elems, repr } => {
            // `ListRepr::Fused` is HIR-only (deforestation); never leave it for emit.
            // Runtime virtual lists are Iota (`lumia_range` / `TYPE_LIST_IOTA`).
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
            // Preserve Eq-only AssocList. PE `LitMap` and sized maps → HashOrdered
            // (emit always heap+finish; no distinct small-table layout). Empty →
            // null at emit regardless of repr.
            if !matches!(*repr, MapRepr::AssocList) {
                *repr = default_map_repr();
            }
            debug_assert!(
                !repr.is_pe_hint(),
                "ReprSelect must lower MapRepr::LitMap before codegen"
            );
            let _ = (flat_pairs, local_ok, max);
        }
        Value::AllocSet { elems, repr } => {
            // Empty → null at emit. PE `LitSet` and all non-empty sets → HeapSet.
            let _ = (elems, local_ok, max);
            *repr = SetRepr::HeapSet;
            debug_assert!(!repr.is_pe_hint());
        }
        Value::AllocAdt { fields, repr, .. } => {
            if local_ok && fields.len() <= max {
                *repr = AdtRepr::LitAdt;
            } else {
                *repr = AdtRepr::HeapAdt;
            }
        }
        Value::AllocClosure { .. } | Value::ClosureCap { .. } => {}
        _ => {}
    }
}

#[cfg(test)]
#[path = "repr_select_tests.rs"]
mod tests;
