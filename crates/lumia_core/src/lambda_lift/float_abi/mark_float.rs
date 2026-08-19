//! Float use scanning and float/bool/channel result lattice.

use crate::ir::{Block, Local, Op, Value};
use crate::value_ty::{builtin_value_ty, InferValueCtx};
use crate::{
    block_result_is_bottom, find_top_level_local_def, for_each_top_level_op_in_block,
    peel_block_result, CoreBinOp as BinOp, CoreUnOp as UnOp,
};
use std::sync::Arc;
use lumia_syntax::Sym;
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use super::prefer_concrete_heap_ty;

pub(crate) fn params_used_as_float_seeded(
    block: &Block,
    params: &[Local],
    seed_float_locals: &HashSet<u32>,
) -> HashSet<u32> {
    let param_set: HashSet<u32> = params.iter().map(|p| p.0).collect();
    let mut float_locals = seed_float_locals.clone();
    let mut used: HashSet<u32> = HashSet::default();
    mark_float_uses(
        block,
        &param_set,
        &mut float_locals,
        &mut used,
        &HashMap::default(),
    );
    used
}

/// Float params via float `ClosureCap` indices, plus pre-seeded float locals
/// (e.g. float `ClosureCap` loads inserted while lifting — ABI is not on the IR node).
pub(crate) fn params_used_as_float_with_caps_seeded(
    block: &Block,
    params: &[Local],
    float_cap_idxs: &HashMap<Sym, HashSet<u32>>,
    seed_float_locals: &HashSet<u32>,
) -> HashSet<u32> {
    let param_set: HashSet<u32> = params.iter().map(|p| p.0).collect();
    let mut float_locals = seed_float_locals.clone();
    let mut used: HashSet<u32> = HashSet::default();
    mark_float_uses(
        block,
        &param_set,
        &mut float_locals,
        &mut used,
        float_cap_idxs,
    );
    used
}

pub(super) fn mark_float_uses(
    block: &Block,
    params: &HashSet<u32>,
    float_locals: &mut HashSet<u32>,
    used: &mut HashSet<u32>,
    float_cap_idxs: &HashMap<Sym, HashSet<u32>>,
) {
    let mut defs: HashMap<u32, Value> = HashMap::default();
    for_each_top_level_op_in_block(block, &mut |op| {
        if let Op::Let { local, value, .. } = op {
            defs.insert(local.0, value.clone());
            mark_float_in_value(value, params, float_locals, used, float_cap_idxs, &defs);
            if value_is_float_producing(value, float_locals) {
                float_locals.insert(local.0);
            }
        }
    });
}

pub(super) fn mark_float_in_value(
    v: &Value,
    params: &HashSet<u32>,
    float_locals: &mut HashSet<u32>,
    used: &mut HashSet<u32>,
    float_cap_idxs: &HashMap<Sym, HashSet<u32>>,
    defs: &HashMap<u32, Value>,
) {
    match v {
        Value::Binary { left, right, .. } => {
            let lf = float_locals.contains(&left.0);
            let rf = float_locals.contains(&right.0);
            if lf || rf {
                touch_param(left.0, params, used);
                touch_param(right.0, params, used);
                // `a(x) + 1.0`: chase IndirectCall/Call args so `x` becomes Float ABI.
                mark_float_through_def(left.0, params, used, defs, &mut HashSet::default());
                mark_float_through_def(right.0, params, used, defs, &mut HashSet::default());
            }
        }
        Value::Unary { operand, .. } => {
            if float_locals.contains(&operand.0) {
                touch_param(operand.0, params, used);
                mark_float_through_def(operand.0, params, used, defs, &mut HashSet::default());
            }
        }
        Value::AllocClosure { fun, captures } => {
            if let Some(idxs) = float_cap_idxs.get(fun.as_str()) {
                for i in idxs {
                    if let Some(cap) = captures.get(*i as usize) {
                        touch_param(cap.0, params, used);
                        float_locals.insert(cap.0);
                    }
                }
            }
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ListParFold,
            args,
            ..
        } if args.len() >= 2 => {
            // `{ y -> xs.fold(y, +) }` with Float xs / init: mark init param Float.
            if float_locals.contains(&args[1].0)
                || list_local_elems_float(args[0].0, float_locals, defs)
            {
                touch_param(args[1].0, params, used);
                mark_float_through_def(args[1].0, params, used, defs, &mut HashSet::default());
                float_locals.insert(args[1].0);
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            mark_float_uses(then_block, params, float_locals, used, float_cap_idxs);
            mark_float_uses(else_block, params, float_locals, used, float_cap_idxs);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            mark_float_uses(header, params, float_locals, used, float_cap_idxs);
            mark_float_uses(body, params, float_locals, used, float_cap_idxs);
            mark_float_uses(latch, params, float_locals, used, float_cap_idxs);
        }
        _ => {}
    }
}

pub(super) fn mark_float_through_def(
    id: u32,
    params: &HashSet<u32>,
    used: &mut HashSet<u32>,
    defs: &HashMap<u32, Value>,
    seen: &mut HashSet<u32>,
) {
    if !seen.insert(id) {
        return;
    }
    touch_param(id, params, used);
    match defs.get(&id) {
        Some(Value::Local(Local(src))) => {
            mark_float_through_def(*src, params, used, defs, seen);
        }
        Some(Value::Call { args, .. } | Value::IndirectCall { args, .. }) => {
            for a in args {
                mark_float_through_def(a.0, params, used, defs, seen);
            }
        }
        _ => {}
    }
}

pub(super) fn touch_param(id: u32, params: &HashSet<u32>, used: &mut HashSet<u32>) {
    if params.contains(&id) {
        used.insert(id);
    }
}

pub(crate) fn value_is_float_producing(v: &Value, float_locals: &HashSet<u32>) -> bool {
    value_is_float_producing_with_defs(v, float_locals, &HashMap::default(), &HashSet::default())
}

pub(super) fn value_is_float_producing_with_defs(
    v: &Value,
    float_locals: &HashSet<u32>,
    defs: &HashMap<u32, Value>,
    float_slots: &HashSet<Sym>,
) -> bool {
    match v {
        Value::Float(_) => true,
        Value::Local(Local(id)) => float_locals.contains(id),
        Value::Name(n) => float_slots.contains(n),
        Value::Binary { op, left, right } => binary_produces_float(*op, left, right, float_locals),
        Value::Unary { op, operand } => {
            matches!(op, UnOp::Neg) && float_locals.contains(&operand.0)
        }
        Value::Builtin {
            name: lumia_hir::Builtin::AdtField,
            args,
            result_ty,
        } => match result_ty {
            Some(Type::Float) => true,
            Some(_) => false,
            None => adt_field_is_float(args, float_locals, defs),
        },
        Value::Builtin {
            name: lumia_hir::Builtin::ListParFold,
            args,
            ..
        } if args.len() >= 2 => {
            // `xs.fold(init, +)` — Float init, or list elems already float-tracked.
            float_locals.contains(&args[1].0)
                || (args.len() >= 1 && list_local_elems_float(args[0].0, float_locals, defs))
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            // `match` desugars to nested `If` with a `MatchFail` default arm.
            // Inherit outer defs/floats so `Some(x) -> x` sees Float payload fields.
            let mut then_slots = float_slots.clone();
            let mut else_slots = float_slots.clone();
            let (tf, _) =
                compute_float_locals_from(then_block, float_locals, defs, &mut then_slots);
            let (ef, _) =
                compute_float_locals_from(else_block, float_locals, defs, &mut else_slots);
            let then_f = block_result_local_is_float(then_block, &tf);
            let else_f = block_result_local_is_float(else_block, &ef);
            let then_ok = then_f || block_result_is_bottom(then_block);
            let else_ok = else_f || block_result_is_bottom(else_block);
            then_ok && else_ok && (then_f || else_f)
        }
        _ => false,
    }
}

pub(super) fn binary_produces_float(
    op: BinOp,
    left: &Local,
    right: &Local,
    float_locals: &HashSet<u32>,
) -> bool {
    matches!(
        op,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
    ) && (float_locals.contains(&left.0) || float_locals.contains(&right.0))
}

/// Chase `Local` / `Name` / `AllocList` to see whether list elems are Float.
pub(super) fn list_local_elems_float(
    id: u32,
    float_locals: &HashSet<u32>,
    defs: &HashMap<u32, Value>,
) -> bool {
    let mut cur = id;
    let mut seen = HashSet::default();
    for _ in 0..lumia_abi::CHANGE_FLAG_ROUNDS {
        if !seen.insert(cur) {
            return false;
        }
        if float_locals.contains(&cur) {
            // List locals are not themselves in float_locals; elems are.
        }
        match defs.get(&cur) {
            Some(Value::Local(Local(src))) => cur = *src,
            Some(Value::AllocList { elems, .. }) => {
                return !elems.is_empty() && elems.iter().all(|e| float_locals.contains(&e.0));
            }
            _ => return false,
        }
    }
    false
}

pub(super) fn binary_produces_bool(op: BinOp) -> bool {
    match op {
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => true,
        BinOp::And | BinOp::Or => {
            debug_assert!(false, "ICE: BinOp::And|Or in Core; expected If desugar");
            true
        }
        _ => false,
    }
}

/// Body result is a Bool (comparison / `&&` / `||` / `!` / bool literal / mut bool).
pub(crate) fn block_result_is_bool(block: &Block) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut bool_slots = HashSet::default();
    let bool_locals = compute_bool_locals_from(block, &HashSet::default(), &mut bool_slots);
    bool_locals.contains(&r)
}

pub(super) fn compute_bool_locals_from(
    block: &Block,
    parent: &HashSet<u32>,
    bool_slots: &mut HashSet<Sym>,
) -> HashSet<u32> {
    let mut bool_locals = parent.clone();
    for_each_top_level_op_in_block(block, &mut |op| match op {
        Op::Assign { name, value } => {
            if bool_locals.contains(&value.0) {
                bool_slots.insert(name.clone());
            } else {
                bool_slots.remove(name);
            }
        }
        Op::Let { local, value, .. } => {
            if value_is_bool_producing(value, &bool_locals, bool_slots) {
                bool_locals.insert(local.0);
            }
            if let Value::Name(n) = value {
                if bool_slots.contains(n) {
                    bool_locals.insert(local.0);
                }
            }
            if let Value::Local(Local(src)) = value {
                if bool_locals.contains(src) {
                    bool_locals.insert(local.0);
                }
            }
            if let Value::If {
                cond,
                then_block,
                else_block,
                ..
            } = value
            {
                let mut then_slots = bool_slots.clone();
                let mut else_slots = bool_slots.clone();
                let tf = compute_bool_locals_from(then_block, &bool_locals, &mut then_slots);
                let ef = compute_bool_locals_from(else_block, &bool_locals, &mut else_slots);
                bool_locals.extend(tf.iter().copied());
                bool_locals.extend(ef.iter().copied());
                let then_b = then_block.result.is_some_and(|Local(r)| tf.contains(&r));
                let else_b = else_block.result.is_some_and(|Local(r)| ef.contains(&r));
                let then_ok = then_b || crate::block_result_is_bottom(then_block);
                let else_ok = else_b || crate::block_result_is_bottom(else_block);
                // `and`/`or` desugar to `if c then x else false` / `if c then true else x`.
                // The open arm may be `ListGet` of a Bool list (fold) and is not yet
                // in `bool_locals` — still a Bool result when `c` is Bool.
                let short_circuit = bool_locals.contains(&cond.0)
                    && (crate::block_result_is_bool_lit(else_block, false)
                        || crate::block_result_is_bool_lit(then_block, true));
                if (then_ok && else_ok && (then_b || else_b)) || short_circuit {
                    bool_locals.insert(local.0);
                }
                bool_slots.extend(then_slots);
                bool_slots.extend(else_slots);
            }
            if let Value::Loop {
                header,
                body,
                latch,
            } = value
            {
                bool_locals.extend(compute_bool_locals_from(header, &bool_locals, bool_slots));
                bool_locals.extend(compute_bool_locals_from(body, &bool_locals, bool_slots));
                bool_locals.extend(compute_bool_locals_from(latch, &bool_locals, bool_slots));
            }
        }
        _ => {}
    });
    bool_locals
}

pub(super) fn value_is_bool_producing(
    v: &Value,
    bool_locals: &HashSet<u32>,
    bool_slots: &HashSet<Sym>,
) -> bool {
    match v {
        Value::Bool(_) => true,
        Value::Local(Local(id)) => bool_locals.contains(id),
        Value::Name(n) => bool_slots.contains(n),
        Value::Binary { op, .. } => binary_produces_bool(*op),
        Value::Unary {
            op: UnOp::Not,
            operand,
        } => bool_locals.contains(&operand.0),
        Value::Builtin { name, .. } => {
            let empty = HashMap::default();
            matches!(
                builtin_value_ty(*name, &[], InferValueCtx::local_only(&empty)),
                Type::Bool
            )
        }
        _ => false,
    }
}

pub(super) fn block_result_local_is_float(block: &Block, float_locals: &HashSet<u32>) -> bool {
    block
        .result
        .is_some_and(|Local(r)| float_locals.contains(&r))
}

/// Body result is `Unit` (`send` / scope / println / …).
pub(crate) fn block_result_is_unit(block: &Block) -> bool {
    match peel_block_result(block) {
        Some(Value::Unit) => true,
        Some(Value::Builtin { name, args, .. }) => {
            if matches!(*name, lumia_hir::Builtin::MatchFail) {
                return false;
            }
            let empty = HashMap::default();
            matches!(
                builtin_value_ty(*name, args, InferValueCtx::local_only(&empty)),
                Type::Unit
            )
        }
        _ => false,
    }
}

/// `ChannelRecv` result typed from per-channel / module send hints.
pub(crate) fn block_result_channel_recv_ty(
    block: &Block,
    by_local: &HashMap<u32, Type>,
    module_hint: Option<&Type>,
    caps: Option<&[Local]>,
) -> Option<Type> {
    let Local(r) = block.result?;
    channel_recv_elem_ty(
        block,
        r,
        by_local,
        module_hint,
        caps,
        &mut HashSet::default(),
    )
}

/// Spawn/thunk returning a **channel value** (not recv): `Channel[T]` from send hints.
pub(crate) fn block_result_channel_ty(
    block: &Block,
    by_local: &HashMap<u32, Type>,
    module_hint: Option<&Type>,
    caps: Option<&[Local]>,
) -> Option<Type> {
    let Local(r) = block.result?;
    let root = channel_root_local(block, r, caps, &mut HashSet::default())?;
    // Only when the result *is* / aliases a `ChannelNew` (recv falls through elsewhere).
    match find_top_level_local_def(block, root) {
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ChannelNew,
            result_ty,
            ..
        }) => {
            let elem = by_local
                .get(&root)
                .cloned()
                .or_else(|| module_hint.cloned())
                .or_else(|| match result_ty {
                    Some(Type::Channel(e)) => Some((**e).clone()),
                    _ => None,
                })
                .unwrap_or(Type::Int);
            Some(Type::Channel(Arc::new(elem)))
        }
        _ => None,
    }
}

pub(crate) fn local_channel_recv_elem_ty(
    block: &Block,
    id: u32,
    by_local: &HashMap<u32, Type>,
    module_hint: Option<&Type>,
    caps: Option<&[Local]>,
) -> Option<Type> {
    channel_recv_elem_ty(
        block,
        id,
        by_local,
        module_hint,
        caps,
        &mut HashSet::default(),
    )
}

pub(super) fn channel_recv_elem_ty(
    block: &Block,
    id: u32,
    by_local: &HashMap<u32, Type>,
    module_hint: Option<&Type>,
    caps: Option<&[Local]>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    match find_top_level_local_def(block, id)? {
        Value::Local(Local(src)) => {
            channel_recv_elem_ty(block, *src, by_local, module_hint, caps, seen)
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ChannelRecv,
            args,
            ..
        } if !args.is_empty() => {
            let root = channel_root_local(block, args[0].0, caps, &mut HashSet::default())?;
            by_local
                .get(&root)
                .cloned()
                .or_else(|| module_hint.cloned())
        }
        _ => None,
    }
}

pub(super) fn channel_root_local(
    block: &Block,
    id: u32,
    caps: Option<&[Local]>,
    seen: &mut HashSet<u32>,
) -> Option<u32> {
    if !seen.insert(id) {
        return None;
    }
    match find_top_level_local_def(block, id) {
        Some(Value::Local(Local(src))) => channel_root_local(block, *src, caps, seen),
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ChannelNew,
            ..
        }) => Some(id),
        Some(Value::ClosureCap { index, .. }) => {
            caps.and_then(|c| c.get(*index as usize)).map(|l| l.0)
        }
        None => {
            // Env / param local with no let — treat as root id when caps resolved.
            Some(id)
        }
        _ => None,
    }
}

/// `AdtField(obj, idx)` yields Float when the ADT field payload is Float.
pub(super) fn adt_field_is_float(
    args: &[Local],
    float_locals: &HashSet<u32>,
    defs: &HashMap<u32, Value>,
) -> bool {
    if args.len() < 2 {
        return false;
    }
    let idx = match defs.get(&args[1].0) {
        Some(Value::Int(i)) if *i >= 0 => *i as usize,
        _ => return false,
    };
    let mut cur = args[0].0;
    let mut seen = HashSet::default();
    loop {
        if !seen.insert(cur) {
            return false;
        }
        match defs.get(&cur) {
            Some(Value::Local(Local(src))) => cur = *src,
            Some(Value::AllocAdt { fields, .. }) => {
                return fields.get(idx).is_some_and(|f| float_locals.contains(&f.0));
            }
            // `toMap` / for-in: `p.1` where `p = xs.get(i)` and xs holds tuples.
            Some(Value::Builtin {
                name: lumia_hir::Builtin::ListGet,
                args: la,
                ..
            }) if !la.is_empty() => {
                cur = la[0].0;
            }
            Some(Value::Builtin {
                name: lumia_hir::Builtin::Elems,
                args: la,
                ..
            }) if !la.is_empty() => {
                cur = la[0].0;
            }
            Some(Value::Builtin {
                name:
                    lumia_hir::Builtin::ListTake
                    | lumia_hir::Builtin::ListSlice
                    | lumia_hir::Builtin::ListReverse
                    | lumia_hir::Builtin::ListSort
                    | lumia_hir::Builtin::ListSortByKeys,
                args: la,
                ..
            }) if !la.is_empty() => {
                cur = la[0].0;
            }
            Some(Value::AllocList { elems, .. }) => {
                return elems.iter().any(|e| {
                    adt_local_field_is_float(e.0, idx, float_locals, defs, &mut HashSet::default())
                });
            }
            // `filter`/`map` acc then `toMap`: `p.1` where `p = acc.get(i)` and
            // `acc` is a Name-load of a list built from float-field tuples still
            // present as `AllocList` in the same function.
            Some(Value::Name(_)) => {
                return defs.values().any(|v| match v {
                    Value::AllocList { elems, .. } => elems.iter().any(|e| {
                        adt_local_field_is_float(
                            e.0,
                            idx,
                            float_locals,
                            defs,
                            &mut HashSet::default(),
                        )
                    }),
                    _ => false,
                });
            }
            _ => return false,
        }
    }
}

pub(super) fn adt_local_field_is_float(
    id: u32,
    idx: usize,
    float_locals: &HashSet<u32>,
    defs: &HashMap<u32, Value>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match defs.get(&id) {
        Some(Value::Local(Local(src))) => {
            adt_local_field_is_float(*src, idx, float_locals, defs, seen)
        }
        Some(Value::AllocAdt { fields, .. }) => {
            fields.get(idx).is_some_and(|f| float_locals.contains(&f.0))
        }
        _ => false,
    }
}

pub(crate) fn block_result_is_float(block: &Block, fun_ret_tys: &HashMap<Sym, Type>) -> bool {
    block_result_is_float_seeded(block, fun_ret_tys, &HashSet::default())
}

/// Like [`block_result_is_float`], with typed float-cap load locals pre-seeded
/// (lift / post-mono fixup — Float caps are not flagged on `ClosureCap`).
pub(crate) fn block_result_is_float_seeded(
    block: &Block,
    fun_ret_tys: &HashMap<Sym, Type>,
    seed_float_locals: &HashSet<u32>,
) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut float_slots = HashSet::default();
    let (float_locals, _) = compute_float_locals_from(
        block,
        seed_float_locals,
        &HashMap::default(),
        &mut float_slots,
    );
    if float_locals.contains(&r) {
        return true;
    }
    // `xs.map(f).get(i)` / `xs.fold` when Float payload.
    list_get_float_result(
        block,
        r,
        &float_locals,
        fun_ret_tys,
        &mut HashSet::default(),
    ) || list_fold_float_result(
        block,
        r,
        &float_locals,
        fun_ret_tys,
        &mut HashSet::default(),
    )
}

pub(super) fn list_get_float_result(
    block: &Block,
    id: u32,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<Sym, Type>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match find_top_level_local_def(block, id) {
        Some(Value::Local(Local(src))) => {
            list_get_float_result(block, *src, float_locals, fun_ret_tys, seen)
        }
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ListGet,
            args,
            ..
        }) if !args.is_empty() => {
            if list_elem_is_float(block, args[0].0, float_locals, fun_ret_tys, seen) {
                return true;
            }
            // Sequential `filter` keeps the acc in a mut slot (`Name` load); the
            // source `AllocList` of floats is still in the same block.
            local_is_name_load(block, args[0].0, &mut HashSet::default())
                && block_has_float_alloc_list(block, float_locals)
        }
        _ => false,
    }
}

pub(super) fn local_is_name_load(block: &Block, id: u32, seen: &mut HashSet<u32>) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match find_top_level_local_def(block, id) {
        Some(Value::Name(_)) => true,
        Some(Value::Local(Local(src))) => local_is_name_load(block, *src, seen),
        _ => false,
    }
}

/// Top-level `AllocList` of floats in `block` only — **intentionally shallow**.
///
/// Do not DFS / `for_each_let_value(_ctrl)`: nested If/Loop/Lambda lists are not
/// the filter-acc source shape this peep covers (Todo: visit 默认入口).
pub(super) fn block_has_float_alloc_list(block: &Block, float_locals: &HashSet<u32>) -> bool {
    let mut found = false;
    for_each_top_level_op_in_block(block, &mut |op| {
        if found {
            return;
        }
        if let Op::Let {
            value: Value::AllocList { elems, .. },
            ..
        } = op
        {
            if !elems.is_empty() && elems.iter().all(|e| float_locals.contains(&e.0)) {
                found = true;
            }
        }
    });
    found
}

pub(super) fn list_elem_is_float(
    block: &Block,
    id: u32,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<Sym, Type>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match find_top_level_local_def(block, id) {
        Some(Value::Local(Local(src))) => {
            list_elem_is_float(block, *src, float_locals, fun_ret_tys, seen)
        }
        Some(Value::AllocList { elems, .. }) => {
            !elems.is_empty() && elems.iter().all(|e| float_locals.contains(&e.0))
        }
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ListParMap,
            args,
            ..
        }) if args.len() >= 2 => funref_ret_is_float(block, args[1].0, fun_ret_tys, seen),
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ListAppend,
            args,
            ..
        }) if args.len() >= 2 => {
            list_elem_is_float(block, args[0].0, float_locals, fun_ret_tys, seen)
                || float_locals.contains(&args[1].0)
        }
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ListConcat,
            args,
            ..
        }) if args.len() >= 2 => {
            list_elem_is_float(block, args[0].0, float_locals, fun_ret_tys, seen)
                || list_elem_is_float(block, args[1].0, float_locals, fun_ret_tys, seen)
        }
        Some(Value::Builtin {
            name:
                lumia_hir::Builtin::ListTake
                | lumia_hir::Builtin::ListSlice
                | lumia_hir::Builtin::ListReverse
                | lumia_hir::Builtin::ListSort
                | lumia_hir::Builtin::ListSortByKeys,
            args,
            ..
        }) if !args.is_empty() => {
            list_elem_is_float(block, args[0].0, float_locals, fun_ret_tys, seen)
        }
        Some(Value::Builtin {
            name: lumia_hir::Builtin::MapValues,
            args,
            ..
        }) if !args.is_empty() => match find_top_level_local_def(block, args[0].0) {
            // Prefer heap typing when available; float map literals still in-block.
            _ => local_map_values_are_float(block, args[0].0, float_locals, seen),
        },
        Some(Value::Name(_)) => {
            // filter/map acc of floats: source AllocList still in the function.
            block_has_float_alloc_list(block, float_locals)
        }
        _ => false,
    }
}

pub(super) fn local_map_values_are_float(
    block: &Block,
    id: u32,
    float_locals: &HashSet<u32>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match find_top_level_local_def(block, id) {
        Some(Value::Local(Local(src))) => {
            local_map_values_are_float(block, *src, float_locals, seen)
        }
        Some(Value::AllocMap { flat_pairs, .. }) => {
            // flat: k0,v0,k1,v1,… — values at odd indices.
            flat_pairs
                .iter()
                .enumerate()
                .any(|(i, p)| i % 2 == 1 && float_locals.contains(&p.0))
        }
        _ => false,
    }
}

pub(super) fn list_fold_float_result(
    block: &Block,
    id: u32,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<Sym, Type>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match find_top_level_local_def(block, id) {
        Some(Value::Local(Local(src))) => {
            list_fold_float_result(block, *src, float_locals, fun_ret_tys, seen)
        }
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ListParFold,
            args,
            ..
        }) if args.len() >= 3 => {
            float_locals.contains(&args[1].0)
                || funref_ret_is_float(block, args[2].0, fun_ret_tys, seen)
        }
        _ => false,
    }
}

pub(super) fn funref_ret_is_float(
    block: &Block,
    id: u32,
    fun_ret_tys: &HashMap<Sym, Type>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match find_top_level_local_def(block, id) {
        Some(Value::Local(Local(src))) => funref_ret_is_float(block, *src, fun_ret_tys, seen),
        Some(Value::FunRef(name) | Value::AllocClosure { fun: name, .. }) => {
            matches!(fun_ret_tys.get(name.as_str()), Some(Type::Float))
        }
        _ => false,
    }
}

/// Locals that hold Float values in `block` (for closure-capture ABI).
pub(crate) fn compute_float_locals_in_block(block: &Block) -> HashSet<u32> {
    let mut float_slots = HashSet::default();
    compute_float_locals_from(
        block,
        &HashSet::default(),
        &HashMap::default(),
        &mut float_slots,
    )
    .0
}

pub(super) fn compute_float_locals_from(
    block: &Block,
    parent_floats: &HashSet<u32>,
    parent_defs: &HashMap<u32, Value>,
    float_slots: &mut HashSet<Sym>,
) -> (HashSet<u32>, HashMap<u32, Value>) {
    let mut float_locals = parent_floats.clone();
    let mut defs = parent_defs.clone();
    for_each_top_level_op_in_block(block, &mut |op| match op {
        Op::Assign { name, value } => {
            if float_locals.contains(&value.0) {
                float_slots.insert(name.clone());
            } else {
                float_slots.remove(name);
            }
        }
        Op::Let { local, value, .. } => {
            defs.insert(local.0, value.clone());
            if value_is_float_producing_with_defs(value, &float_locals, &defs, float_slots)
                || matches!(value, Value::Float(_))
            {
                float_locals.insert(local.0);
            }
            if let Value::Binary { op, left, right } = value {
                if binary_produces_float(*op, left, right, &float_locals) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::Local(Local(src)) = value {
                if float_locals.contains(src) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::Name(n) = value {
                if float_slots.contains(n) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::Unary { op, operand } = value {
                if matches!(op, UnOp::Neg) && float_locals.contains(&operand.0) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::Builtin {
                name: lumia_hir::Builtin::AdtField,
                args,
                result_ty,
            } = value
            {
                let is_f = match result_ty {
                    Some(Type::Float) => true,
                    Some(_) => false,
                    None => adt_field_is_float(args, &float_locals, &defs),
                };
                if is_f {
                    float_locals.insert(local.0);
                }
            }
            if let Value::If {
                then_block,
                else_block,
                ..
            } = value
            {
                let mut then_slots = float_slots.clone();
                let mut else_slots = float_slots.clone();
                let (tf, _) =
                    compute_float_locals_from(then_block, &float_locals, &defs, &mut then_slots);
                let (ef, _) =
                    compute_float_locals_from(else_block, &float_locals, &defs, &mut else_slots);
                float_locals.extend(tf);
                float_locals.extend(ef);
                // Prefer Float ABI if either arm keeps the slot as Float.
                float_slots.extend(then_slots);
                float_slots.extend(else_slots);
            }
            if let Value::Loop {
                header,
                body,
                latch,
            } = value
            {
                let (hf, _) = compute_float_locals_from(header, &float_locals, &defs, float_slots);
                float_locals.extend(hf);
                let (bf, _) = compute_float_locals_from(body, &float_locals, &defs, float_slots);
                float_locals.extend(bf);
                let (lf, _) = compute_float_locals_from(latch, &float_locals, &defs, float_slots);
                float_locals.extend(lf);
            }
        }
        _ => {}
    });
    (float_locals, defs)
}

/// Collect `ClosureCap` types from every `AllocClosure` site (capture SSA → heap ty).
///
/// Nested closures (`spawn { { s -> prefix.concat(s) } }`) allocate the inner
/// lambda inside an outer `__lam_*`; that site's captures are `ClosureCap`s of
/// the outer env. A single pass with empty outer caps misses those String/Float
/// types — iterate so outer AllocClosure sites populate `out` before (or for)
/// nested ones.
pub(crate) fn collect_fun_cap_tys(
    module: &crate::ir::CoreModule,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
) -> HashMap<Sym, HashMap<u32, Type>> {
    super::super::with_lifted_lambda_names(super::super::lifted_lambda_names(module), || {
        collect_fun_cap_tys_inner(module, fun_ret_tys, fun_param_tys)
    })
}

pub(super) fn collect_fun_cap_tys_inner(
    module: &crate::ir::CoreModule,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
) -> HashMap<Sym, HashMap<u32, Type>> {
    let by_local = &module.channel_elem_by_local;
    let module_hint = module.channel_elem_hint.as_ref();
    let mut out: HashMap<Sym, HashMap<u32, Type>> = HashMap::default();
    for _ in 0..16 {
        let before: usize = out.values().map(|m| m.len()).sum();
        for fun in &module.functions {
            let float_locals = compute_float_locals_in_block(&fun.body);
            let mut param_locals: HashMap<u32, Type> = HashMap::default();
            for (p, ty) in fun.params.iter().zip(fun.param_tys.iter()) {
                param_locals.insert(p.0, ty.clone());
            }
            // Caps already known for this fun (from outer AllocClosure sites).
            let outer = out
                .get(&fun.name)
                .cloned()
                .unwrap_or_default();
            collect_fun_cap_tys_in_block(
                &fun.body,
                &float_locals,
                fun_ret_tys,
                fun_param_tys,
                &outer,
                &param_locals,
                by_local,
                module_hint,
                None,
                &mut out,
            );
        }
        let after: usize = out.values().map(|m| m.len()).sum();
        if after == before {
            break;
        }
    }
    out
}

pub(super) fn collect_fun_cap_tys_in_block(
    block: &Block,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
    outer_caps: &HashMap<u32, Type>,
    param_locals: &HashMap<u32, Type>,
    channel_by_local: &HashMap<u32, Type>,
    channel_module_hint: Option<&Type>,
    outer_lam_caps: Option<&[crate::Local]>,
    out: &mut HashMap<Sym, HashMap<u32, Type>>,
) {
    // Prefer merges are order-tolerant for concrete lattice; DFS is safe.
    crate::visit::for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                value: Value::AllocClosure { fun, captures },
                ..
            } = op
            {
                let entry = out.entry(fun.name.clone()).or_default();
                for (i, c) in captures.iter().enumerate() {
                    let t = if float_locals.contains(&c.0) {
                        Some(Type::Float)
                    } else if let Some(t) = channel_recv_elem_ty(
                        b,
                        c.0,
                        channel_by_local,
                        channel_module_hint,
                        outer_lam_caps,
                        &mut HashSet::default(),
                    ) {
                        Some(t)
                    } else if let Some(t) = param_locals.get(&c.0) {
                        Some(t.clone())
                    } else {
                        super::local_heap::local_heap_ty(
                            b,
                            c.0,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            outer_caps,
                            &mut HashSet::default(),
                            &mut HashSet::default(),
                        )
                    };
                    if let Some(t) = t {
                        let e = entry.entry(i as u32).or_insert_with(|| t.clone());
                        *e = prefer_concrete_heap_ty(e.clone(), t);
                    }
                }
            }
        }
    });
}
