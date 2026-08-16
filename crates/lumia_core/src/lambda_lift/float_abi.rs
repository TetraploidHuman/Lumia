//! Float ABI inference for lifted lambdas.

use crate::ir::{Block, Local, Op, Value};
use crate::{CoreBinOp as BinOp, CoreUnOp as UnOp};
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Infer which lambda parameters are used in float contexts.
pub(super) fn params_used_as_float(block: &Block, params: &[Local]) -> HashSet<u32> {
    params_used_as_float_with_caps(block, params, &HashMap::default())
}

/// Like [`params_used_as_float`], also treating locals captured into a callee
/// at a float `ClosureCap` index as float (e.g. `{ x -> spawn { x * 2.0 } }`).
pub(super) fn params_used_as_float_with_caps(
    block: &Block,
    params: &[Local],
    float_cap_idxs: &HashMap<String, HashSet<u32>>,
) -> HashSet<u32> {
    let param_set: HashSet<u32> = params.iter().map(|p| p.0).collect();
    let mut float_locals: HashSet<u32> = HashSet::default();
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

/// Capture indices loaded with `as_float` in `body`.
pub(super) fn float_closure_cap_indices(block: &Block) -> HashSet<u32> {
    let mut idxs = HashSet::default();
    collect_float_cap_indices(block, &mut idxs);
    idxs
}

fn collect_float_cap_indices(block: &Block, idxs: &mut HashSet<u32>) {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } => {
                if let Value::ClosureCap {
                    index,
                    as_float: true,
                    ..
                } = value
                {
                    idxs.insert(*index);
                }
                crate::visit::for_each_nested_block(value, &mut |b| {
                    collect_float_cap_indices(b, idxs);
                });
            }
            _ => {}
        }
    }
}

fn mark_float_uses(
    block: &Block,
    params: &HashSet<u32>,
    float_locals: &mut HashSet<u32>,
    used: &mut HashSet<u32>,
    float_cap_idxs: &HashMap<String, HashSet<u32>>,
) {
    let mut defs: HashMap<u32, &Value> = HashMap::default();
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                defs.insert(local.0, value);
                mark_float_in_value(value, params, float_locals, used, float_cap_idxs, &defs);
                if value_is_float_producing(value, float_locals) {
                    float_locals.insert(local.0);
                }
            }
            _ => {}
        }
    }
}

fn mark_float_in_value(
    v: &Value,
    params: &HashSet<u32>,
    float_locals: &mut HashSet<u32>,
    used: &mut HashSet<u32>,
    float_cap_idxs: &HashMap<String, HashSet<u32>>,
    defs: &HashMap<u32, &Value>,
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
            if let Some(idxs) = float_cap_idxs.get(fun) {
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
            args, .. } if args.len() >= 2 => {
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

fn mark_float_through_def(
    id: u32,
    params: &HashSet<u32>,
    used: &mut HashSet<u32>,
    defs: &HashMap<u32, &Value>,
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

fn touch_param(id: u32, params: &HashSet<u32>, used: &mut HashSet<u32>) {
    if params.contains(&id) {
        used.insert(id);
    }
}

pub(super) fn value_is_float_producing(v: &Value, float_locals: &HashSet<u32>) -> bool {
    value_is_float_producing_with_defs(v, float_locals, &HashMap::default(), &HashSet::default())
}

fn value_is_float_producing_with_defs(
    v: &Value,
    float_locals: &HashSet<u32>,
    defs: &HashMap<u32, &Value>,
    float_slots: &HashSet<String>,
) -> bool {
    match v {
        Value::Float(_) => true,
        Value::Local(Local(id)) => float_locals.contains(id),
        Value::Name(n) => float_slots.contains(n),
        Value::ClosureCap { as_float: true, .. } => true,
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
            args, .. } if args.len() >= 2 => {
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

fn binary_produces_float(
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
fn list_local_elems_float(
    id: u32,
    float_locals: &HashSet<u32>,
    defs: &HashMap<u32, &Value>,
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

fn binary_produces_bool(op: BinOp) -> bool {
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
pub(super) fn block_result_is_bool(block: &Block) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut bool_slots = HashSet::default();
    let bool_locals = compute_bool_locals_from(block, &HashSet::default(), &mut bool_slots);
    bool_locals.contains(&r)
}

fn compute_bool_locals_from(
    block: &Block,
    parent: &HashSet<u32>,
    bool_slots: &mut HashSet<String>,
) -> HashSet<u32> {
    let mut bool_locals = parent.clone();
    for op in &block.ops {
        match op {
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
                    let then_b = then_block
                        .result
                        .is_some_and(|Local(r)| tf.contains(&r));
                    let else_b = else_block
                        .result
                        .is_some_and(|Local(r)| ef.contains(&r));
                    let then_ok = then_b || block_result_is_bottom(then_block);
                    let else_ok = else_b || block_result_is_bottom(else_block);
                    // `and`/`or` desugar to `if c then x else false` / `if c then true else x`.
                    // The open arm may be `ListGet` of a Bool list (fold) and is not yet
                    // in `bool_locals` — still a Bool result when `c` is Bool.
                    let short_circuit = bool_locals.contains(&cond.0)
                        && (block_result_is_bool_lit(else_block, false)
                            || block_result_is_bool_lit(then_block, true));
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
                    bool_locals.extend(compute_bool_locals_from(
                        header,
                        &bool_locals,
                        bool_slots,
                    ));
                    bool_locals
                        .extend(compute_bool_locals_from(body, &bool_locals, bool_slots));
                    bool_locals.extend(compute_bool_locals_from(
                        latch,
                        &bool_locals,
                        bool_slots,
                    ));
                }
            }
            _ => {}
        }
    }
    bool_locals
}

fn value_is_bool_producing(
    v: &Value,
    bool_locals: &HashSet<u32>,
    bool_slots: &HashSet<String>,
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
        Value::Builtin { name, .. } => matches!(
            name,
            lumia_hir::Builtin::Contains
                | lumia_hir::Builtin::StrStartsWith
                | lumia_hir::Builtin::StrEndsWith
        ),
        _ => false,
    }
}

fn block_result_local_is_float(block: &Block, float_locals: &HashSet<u32>) -> bool {
    block
        .result
        .is_some_and(|Local(r)| float_locals.contains(&r))
}

/// Unreachable / exhaustiveness arm (`MatchFail`) — compatible with any result ty.
fn block_result_is_bottom(block: &Block) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut seen = HashSet::default();
    let mut cur = r;
    loop {
        if !seen.insert(cur) {
            return false;
        }
        match let_value(block, cur) {
            Some(Value::Local(Local(src))) => cur = *src,
            Some(Value::Builtin {
                name: lumia_hir::Builtin::MatchFail,
                ..
            }) => return true,
            _ => return false,
        }
    }
}

/// `if` arm that is the Bool literal from `and`/`or` desugaring.
fn block_result_is_bool_lit(block: &Block, expect: bool) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut seen = HashSet::default();
    let mut cur = r;
    loop {
        if !seen.insert(cur) {
            return false;
        }
        match let_value(block, cur) {
            Some(Value::Local(Local(src))) => cur = *src,
            Some(Value::Bool(b)) => return *b == expect,
            _ => return false,
        }
    }
}

/// Body result is `Unit` (`send` / scope / println / …).
pub(super) fn block_result_is_unit(block: &Block) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut seen = HashSet::default();
    let mut cur = r;
    loop {
        if !seen.insert(cur) {
            return false;
        }
        match let_value(block, cur) {
            Some(Value::Local(Local(src))) => cur = *src,
            Some(Value::Unit) => return true,
            Some(Value::Builtin { name, .. }) => {
                return matches!(
                    name,
                    lumia_hir::Builtin::ChannelSend
                        | lumia_hir::Builtin::ChannelClose
                        | lumia_hir::Builtin::ScopeEnter
                        | lumia_hir::Builtin::ScopeLeave
                        | lumia_hir::Builtin::ScopeCancel
                        | lumia_hir::Builtin::Println
                        | lumia_hir::Builtin::Assert
                );
            }
            _ => return false,
        }
    }
}

/// `ChannelRecv` result typed from per-channel / module send hints.
pub(super) fn block_result_channel_recv_ty(
    block: &Block,
    by_local: &HashMap<u32, Type>,
    module_hint: Option<&Type>,
    caps: Option<&[Local]>,
) -> Option<Type> {
    let Local(r) = block.result?;
    channel_recv_elem_ty(block, r, by_local, module_hint, caps, &mut HashSet::default())
}

pub(super) fn local_channel_recv_elem_ty(
    block: &Block,
    id: u32,
    by_local: &HashMap<u32, Type>,
    module_hint: Option<&Type>,
    caps: Option<&[Local]>,
) -> Option<Type> {
    channel_recv_elem_ty(block, id, by_local, module_hint, caps, &mut HashSet::default())
}

fn channel_recv_elem_ty(
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
    match let_value(block, id)? {
        Value::Local(Local(src)) => {
            channel_recv_elem_ty(block, *src, by_local, module_hint, caps, seen)
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ChannelRecv,
            args, .. } if !args.is_empty() => {
            let root = channel_root_local(block, args[0].0, caps, &mut HashSet::default())?;
            by_local
                .get(&root)
                .cloned()
                .or_else(|| module_hint.cloned())
        }
        _ => None,
    }
}

fn channel_root_local(
    block: &Block,
    id: u32,
    caps: Option<&[Local]>,
    seen: &mut HashSet<u32>,
) -> Option<u32> {
    if !seen.insert(id) {
        return None;
    }
    match let_value(block, id) {
        Some(Value::Local(Local(src))) => channel_root_local(block, *src, caps, seen),
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ChannelNew,
            ..
        }) => Some(id),
        Some(Value::ClosureCap { index, .. }) => caps
            .and_then(|c| c.get(*index as usize))
            .map(|l| l.0),
        None => {
            // Env / param local with no let — treat as root id when caps resolved.
            Some(id)
        }
        _ => None,
    }
}

/// `AdtField(obj, idx)` yields Float when the ADT field payload is Float.
fn adt_field_is_float(
    args: &[Local],
    float_locals: &HashSet<u32>,
    defs: &HashMap<u32, &Value>,
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
                args: la, .. }) if !la.is_empty() => {
                cur = la[0].0;
            }
            Some(Value::Builtin {
                name: lumia_hir::Builtin::Elems,
                args: la, .. }) if !la.is_empty() => {
                cur = la[0].0;
            }
            Some(Value::Builtin {
                name: lumia_hir::Builtin::ListTake
                    | lumia_hir::Builtin::ListSlice
                    | lumia_hir::Builtin::ListReverse,
                args: la, .. }) if !la.is_empty() => {
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

fn adt_local_field_is_float(
    id: u32,
    idx: usize,
    float_locals: &HashSet<u32>,
    defs: &HashMap<u32, &Value>,
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

pub(super) fn block_result_is_float(
    block: &Block,
    fun_ret_tys: &HashMap<String, Type>,
) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let float_locals = compute_float_locals_in_block(block);
    if float_locals.contains(&r) {
        return true;
    }
    // `xs.map(f).get(i)` / `xs.fold` when Float payload.
    list_get_float_result(block, r, &float_locals, fun_ret_tys, &mut HashSet::default())
        || list_fold_float_result(block, r, &float_locals, fun_ret_tys, &mut HashSet::default())
}

fn list_get_float_result(
    block: &Block,
    id: u32,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match let_value(block, id) {
        Some(Value::Local(Local(src))) => {
            list_get_float_result(block, *src, float_locals, fun_ret_tys, seen)
        }
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ListGet,
            args, .. }) if !args.is_empty() => {
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

fn local_is_name_load(block: &Block, id: u32, seen: &mut HashSet<u32>) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match let_value(block, id) {
        Some(Value::Name(_)) => true,
        Some(Value::Local(Local(src))) => local_is_name_load(block, *src, seen),
        _ => false,
    }
}

fn block_has_float_alloc_list(block: &Block, float_locals: &HashSet<u32>) -> bool {
    for op in &block.ops {
        if let Op::Let {
            value: Value::AllocList { elems, .. },
            ..
        } = op
        {
            if !elems.is_empty() && elems.iter().all(|e| float_locals.contains(&e.0)) {
                return true;
            }
        }
    }
    false
}

fn list_elem_is_float(
    block: &Block,
    id: u32,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match let_value(block, id) {
        Some(Value::Local(Local(src))) => {
            list_elem_is_float(block, *src, float_locals, fun_ret_tys, seen)
        }
        Some(Value::AllocList { elems, .. }) => {
            !elems.is_empty() && elems.iter().all(|e| float_locals.contains(&e.0))
        }
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ListParMap,
            args, .. }) if args.len() >= 2 => funref_ret_is_float(block, args[1].0, fun_ret_tys, seen),
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ListAppend,
            args, .. }) if args.len() >= 2 => {
            list_elem_is_float(block, args[0].0, float_locals, fun_ret_tys, seen)
                || float_locals.contains(&args[1].0)
        }
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ListConcat,
            args, .. }) if args.len() >= 2 => {
            list_elem_is_float(block, args[0].0, float_locals, fun_ret_tys, seen)
                || list_elem_is_float(block, args[1].0, float_locals, fun_ret_tys, seen)
        }
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ListTake
                | lumia_hir::Builtin::ListSlice
                | lumia_hir::Builtin::ListReverse,
            args, .. }) if !args.is_empty() => {
            list_elem_is_float(block, args[0].0, float_locals, fun_ret_tys, seen)
        }
        Some(Value::Builtin {
            name: lumia_hir::Builtin::MapValues,
            args, .. }) if !args.is_empty() => match let_value(block, args[0].0) {
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

fn local_map_values_are_float(
    block: &Block,
    id: u32,
    float_locals: &HashSet<u32>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match let_value(block, id) {
        Some(Value::Local(Local(src))) => local_map_values_are_float(block, *src, float_locals, seen),
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

fn list_fold_float_result(
    block: &Block,
    id: u32,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match let_value(block, id) {
        Some(Value::Local(Local(src))) => {
            list_fold_float_result(block, *src, float_locals, fun_ret_tys, seen)
        }
        Some(Value::Builtin {
            name: lumia_hir::Builtin::ListParFold,
            args, .. }) if args.len() >= 3 => {
            float_locals.contains(&args[1].0)
                || funref_ret_is_float(block, args[2].0, fun_ret_tys, seen)
        }
        _ => false,
    }
}

fn funref_ret_is_float(
    block: &Block,
    id: u32,
    fun_ret_tys: &HashMap<String, Type>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match let_value(block, id) {
        Some(Value::Local(Local(src))) => funref_ret_is_float(block, *src, fun_ret_tys, seen),
        Some(Value::FunRef(name) | Value::AllocClosure { fun: name, .. }) => {
            matches!(fun_ret_tys.get(name), Some(Type::Float))
        }
        _ => false,
    }
}

/// Locals that hold Float values in `block` (for closure-capture ABI).
pub(super) fn compute_float_locals_in_block(block: &Block) -> HashSet<u32> {
    let mut float_slots = HashSet::default();
    compute_float_locals_from(
        block,
        &HashSet::default(),
        &HashMap::default(),
        &mut float_slots,
    )
    .0
}

fn compute_float_locals_from<'a>(
    block: &'a Block,
    parent_floats: &HashSet<u32>,
    parent_defs: &HashMap<u32, &'a Value>,
    float_slots: &mut HashSet<String>,
) -> (HashSet<u32>, HashMap<u32, &'a Value>) {
    let mut float_locals = parent_floats.clone();
    let mut defs = parent_defs.clone();
    for op in &block.ops {
        match op {
            Op::Assign { name, value } => {
                if float_locals.contains(&value.0) {
                    float_slots.insert(name.clone());
                } else {
                    float_slots.remove(name);
                }
            }
            Op::Let { local, value, .. } => {
                defs.insert(local.0, value);
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
                if let Value::ClosureCap { as_float: true, .. } = value {
                    float_locals.insert(local.0);
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
                    let (hf, _) =
                        compute_float_locals_from(header, &float_locals, &defs, float_slots);
                    float_locals.extend(hf);
                    let (bf, _) = compute_float_locals_from(body, &float_locals, &defs, float_slots);
                    float_locals.extend(bf);
                    let (lf, _) =
                        compute_float_locals_from(latch, &float_locals, &defs, float_slots);
                    float_locals.extend(lf);
                }
            }
            _ => {}
        }
    }
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
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
) -> HashMap<String, HashMap<u32, Type>> {
    super::with_lifted_lambda_names(super::lifted_lambda_names(module), || {
        collect_fun_cap_tys_inner(module, fun_ret_tys, fun_param_tys)
    })
}

fn collect_fun_cap_tys_inner(
    module: &crate::ir::CoreModule,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
) -> HashMap<String, HashMap<u32, Type>> {
    let by_local = &module.channel_elem_by_local;
    let module_hint = module.channel_elem_hint.as_ref();
    let mut out: HashMap<String, HashMap<u32, Type>> = HashMap::default();
    for _ in 0..16 {
        let before: usize = out.values().map(|m| m.len()).sum();
        for fun in &module.functions {
            let float_locals = compute_float_locals_in_block(&fun.body);
            let mut param_locals: HashMap<u32, Type> = HashMap::default();
            for (p, ty) in fun.params.iter().zip(fun.param_tys.iter()) {
                param_locals.insert(p.0, ty.clone());
            }
            // Caps already known for this fun (from outer AllocClosure sites).
            let outer = out.get(&fun.name).cloned().unwrap_or_default();
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

fn collect_fun_cap_tys_in_block(
    block: &Block,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    outer_caps: &HashMap<u32, Type>,
    param_locals: &HashMap<u32, Type>,
    channel_by_local: &HashMap<u32, Type>,
    channel_module_hint: Option<&Type>,
    outer_lam_caps: Option<&[crate::Local]>,
    out: &mut HashMap<String, HashMap<u32, Type>>,
) {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } => {
                if let Value::AllocClosure { fun, captures } = value {
                    let entry = out.entry(fun.clone()).or_default();
                    for (i, c) in captures.iter().enumerate() {
                        let t = if float_locals.contains(&c.0) {
                            Some(Type::Float)
                        } else if let Some(t) = channel_recv_elem_ty(
                            block,
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
                            local_heap_ty(
                                block,
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
                match value {
                    Value::If {
                        then_block,
                        else_block,
                        ..
                    } => {
                        collect_fun_cap_tys_in_block(
                            then_block,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            outer_caps,
                            param_locals,
                            channel_by_local,
                            channel_module_hint,
                            outer_lam_caps,
                            out,
                        );
                        collect_fun_cap_tys_in_block(
                            else_block,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            outer_caps,
                            param_locals,
                            channel_by_local,
                            channel_module_hint,
                            outer_lam_caps,
                            out,
                        );
                    }
                    Value::Loop {
                        header,
                        body,
                        latch,
                    } => {
                        collect_fun_cap_tys_in_block(
                            header,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            outer_caps,
                            param_locals,
                            channel_by_local,
                            channel_module_hint,
                            outer_lam_caps,
                            out,
                        );
                        collect_fun_cap_tys_in_block(
                            body,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            outer_caps,
                            param_locals,
                            channel_by_local,
                            channel_module_hint,
                            outer_lam_caps,
                            out,
                        );
                        collect_fun_cap_tys_in_block(
                            latch,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            outer_caps,
                            param_locals,
                            channel_by_local,
                            channel_module_hint,
                            outer_lam_caps,
                            out,
                        );
                    }
                    Value::Lambda { body, .. } => {
                        debug_assert!(
                            false,
                            "ICE: Value::Lambda after lift; expected FunRef/AllocClosure"
                        );
                        collect_fun_cap_tys_in_block(
                            body,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            outer_caps,
                            param_locals,
                            channel_by_local,
                            channel_module_hint,
                            outer_lam_caps,
                            out,
                        );
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Concrete heap return for a lifted lambda body (`List[Float]` / float-field ADT /
/// `Task[…]` / `Fun`), so `spawn { listOf(1.5) }.join().get(0)` / nested spawn keep ABI.
pub(crate) fn block_result_heap_ty(
    block: &Block,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
) -> Option<Type> {
    block_result_heap_ty_caps(block, fun_ret_tys, fun_param_tys, &HashMap::default())
}

/// Like [`block_result_heap_ty`], with known `ClosureCap` types from AllocClosure sites.
///
/// Callers must install FunKind lifted names via [`super::with_lifted_lambda_names`]
/// (or go through [`collect_fun_cap_tys`] / lift / fixup entry points).
pub(crate) fn block_result_heap_ty_caps(
    block: &Block,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
) -> Option<Type> {
    let Local(r) = block.result?;
    let float_locals = compute_float_locals_in_block(block);
    local_heap_ty(
        block,
        r,
        &float_locals,
        fun_ret_tys,
        fun_param_tys,
        cap_tys,
        &mut HashSet::default(),
        &mut HashSet::default(),
    )
}

/// Defining `Let` for `id`, including nested If/Loop bodies (map-acc slots, etc.).
fn let_value_dfs<'a>(block: &'a Block, id: u32) -> Option<&'a Value> {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                if local.0 == id {
                    return Some(value);
                }
                if let Some(v) = let_value_in_nested(value, id) {
                    return Some(v);
                }
            }
            _ => {}
        }
    }
    None
}

fn let_value_in_nested<'a>(value: &'a Value, id: u32) -> Option<&'a Value> {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => let_value_dfs(then_block, id).or_else(|| let_value_dfs(else_block, id)),
        Value::Loop {
            header,
            body,
            latch,
        } => let_value_dfs(header, id)
            .or_else(|| let_value_dfs(body, id))
            .or_else(|| let_value_dfs(latch, id)),
        Value::Lambda { body, .. } => {
            debug_assert!(false, "ICE: Value::Lambda after lift");
            let_value_dfs(body, id)
        }
        _ => None,
    }
}

/// Heap type of a mutable/immutable slot (`Name` / `Assign`), joining all writes.
fn slot_heap_ty(
    block: &Block,
    name: &str,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    seen_slots: &mut HashSet<String>,
) -> Option<Type> {
    if !seen_slots.insert(name.to_string()) {
        return None;
    }
    let mut acc: Option<Type> = None;
    collect_slot_assigns(
        block,
        block,
        name,
        float_locals,
        fun_ret_tys,
        fun_param_tys,
        cap_tys,
        seen,
        seen_slots,
        &mut acc,
    );
    acc
}

fn collect_slot_assigns(
    walk: &Block,
    defs_root: &Block,
    name: &str,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    seen_slots: &mut HashSet<String>,
    acc: &mut Option<Type>,
) {
    for op in &walk.ops {
        match op {
            Op::Assign {
                name: n,
                value: Local(src),
            } if n == name => {
                if let Some(t) = local_heap_ty(
                    defs_root,
                    *src,
                    float_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    cap_tys,
                    seen,
                    seen_slots,
                ) {
                    *acc = Some(match acc.take() {
                        None => t,
                        Some(prev) => prefer_concrete_heap_ty(prev, t),
                    });
                }
            }
            Op::Let { value, .. } => {
                match value {
                    Value::If {
                        then_block,
                        else_block,
                        ..
                    } => {
                        collect_slot_assigns(
                            then_block,
                            defs_root,
                            name,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            cap_tys,
                            seen,
                            seen_slots,
                            acc,
                        );
                        collect_slot_assigns(
                            else_block,
                            defs_root,
                            name,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            cap_tys,
                            seen,
                            seen_slots,
                            acc,
                        );
                    }
                    Value::Loop {
                        header,
                        body,
                        latch,
                    } => {
                        collect_slot_assigns(
                            header,
                            defs_root,
                            name,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            cap_tys,
                            seen,
                            seen_slots,
                            acc,
                        );
                        collect_slot_assigns(
                            body,
                            defs_root,
                            name,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            cap_tys,
                            seen,
                            seen_slots,
                            acc,
                        );
                        collect_slot_assigns(
                            latch,
                            defs_root,
                            name,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            cap_tys,
                            seen,
                            seen_slots,
                            acc,
                        );
                    }
                    Value::Lambda { body, .. } => {
                        debug_assert!(false, "ICE: Value::Lambda after lift");
                        collect_slot_assigns(
                            body,
                            defs_root,
                            name,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            cap_tys,
                            seen,
                            seen_slots,
                            acc,
                        );
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn local_heap_ty(
    block: &Block,
    id: u32,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    seen_slots: &mut HashSet<String>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    let value = let_value_dfs(block, id)?;
    match value {
        Value::Local(Local(src)) => local_heap_ty(
            block,
            *src,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        ),
        Value::Name(n) => slot_heap_ty(
            block,
            n,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        ),
        Value::FunRef(name) | Value::AllocClosure { fun: name, .. } => {
            fun_ty_from_tables(name, fun_ret_tys, fun_param_tys)
        }
        Value::String(_) => Some(Type::String),
        Value::Char(_) => Some(Type::Char),
        Value::Float(_) => Some(Type::Float),
        Value::Bool(_) => Some(Type::Bool),
        Value::ClosureCap {
            index,
            as_float,
            ..
        } => {
            if *as_float {
                Some(Type::Float)
            } else {
                cap_tys.get(index).cloned()
            }
        }
        Value::Call { fun, .. } => fun_ret_tys.get(fun).cloned(),
        Value::Binary { op, left, right }
            if !binary_produces_bool(*op)
                && matches!(
                    *op,
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
                ) =>
        {
            let lt = local_heap_ty(
                block,
                left.0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            let rt = local_heap_ty(
                block,
                right.0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            if matches!(lt, Some(Type::Float)) || matches!(rt, Some(Type::Float)) {
                Some(Type::Float)
            } else {
                None
            }
        }
        Value::Unary {
            op: UnOp::Neg,
            operand,
        } => match local_heap_ty(
            block,
            operand.0,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        ) {
            Some(Type::Float) => Some(Type::Float),
            _ => None,
        },
        Value::IndirectCall { callee, .. } => {
            match local_heap_ty(
                block,
                callee.0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::Fun(_, ret, _)) => Some(*ret),
                _ => fun_ret_of_local(block, callee.0, fun_ret_tys, seen),
            }
        }
        Value::AllocList { elems, .. } => Some(Type::List(Box::new(alloc_elems_ty(
            block,
            elems,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        )))),
        Value::AllocSet { elems, .. } => Some(Type::Set(Box::new(alloc_elems_ty(
            block,
            elems,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        )))),
        Value::AllocMap { flat_pairs, .. } => {
            let (k, v) = if flat_pairs.len() >= 2 {
                (
                    if float_locals.contains(&flat_pairs[0].0) {
                        Type::Float
                    } else {
                        local_heap_ty(
                            block,
                            flat_pairs[0].0,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            cap_tys,
                            seen,
                            seen_slots,
                        )
                        .unwrap_or(Type::Int)
                    },
                    if float_locals.contains(&flat_pairs[1].0) {
                        Type::Float
                    } else {
                        local_heap_ty(
                            block,
                            flat_pairs[1].0,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            cap_tys,
                            seen,
                            seen_slots,
                        )
                        .unwrap_or(Type::Int)
                    },
                )
            } else {
                (Type::Int, Type::Int)
            };
            Some(Type::Map(Box::new(k), Box::new(v)))
        }
        Value::AllocAdt {
            adt_name,
            fields,
            ..
        } => {
            let params: Vec<Type> = fields
                .iter()
                .map(|f| {
                    if float_locals.contains(&f.0) {
                        Type::Float
                    } else {
                        local_heap_ty(
                            block,
                            f.0,
                            float_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            cap_tys,
                            seen,
                            seen_slots,
                        )
                        .unwrap_or(Type::Int)
                    }
                })
                .collect();
            Some(Type::Adt {
                name: adt_name.clone(),
                params,
            })
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ListGet,
            args, .. } if !args.is_empty() => {
            match local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::List(e) | Type::Set(e)) => Some(*e),
                Some(Type::Map(_, v)) => Some(Type::Adt {
                    name: lumia_hir::OPTION.name.into(),
                    params: vec![*v],
                }),
                _ => None,
            }
        }
        Value::Builtin {
            name: lumia_hir::Builtin::Elems,
            args, .. } if !args.is_empty() => {
            match local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::List(e) | Type::Set(e)) => Some(Type::List(e)),
                Some(Type::Map(k, _)) => Some(Type::List(k)),
                _ => None,
            }
        }
        Value::Builtin {
            name: lumia_hir::Builtin::MapValues,
            args, .. } if !args.is_empty() => {
            match local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::Map(_, v)) => Some(Type::List(v)),
                _ => None,
            }
        }
        Value::Builtin {
            name: lumia_hir::Builtin::MapKeys,
            args, .. } if !args.is_empty() => {
            match local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::Map(k, _)) => Some(Type::List(k)),
                _ => None,
            }
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ListTake
                | lumia_hir::Builtin::ListSlice
                | lumia_hir::Builtin::ListReverse,
            args, .. } if !args.is_empty() => {
            match local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::List(e)) => Some(Type::List(e)),
                other => other,
            }
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ListConcat,
            args, .. } if args.len() >= 2 => {
            let a = local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            let b = local_heap_ty(
                block,
                args[1].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            match (a, b) {
                // `.concat` is shared by List and String; do not fall through to the
                // lift `List[Int]` placeholder (spawn { s.concat(…) }.join().len()).
                (Some(Type::String), _) | (_, Some(Type::String)) => Some(Type::String),
                (Some(Type::List(e1)), Some(Type::List(e2))) => Some(Type::List(Box::new(
                    prefer_concrete_heap_ty(*e1, *e2),
                ))),
                (Some(Type::List(e)), _) | (_, Some(Type::List(e))) => {
                    Some(Type::List(e))
                }
                _ => None,
            }
        }
        Value::Builtin {
            name: lumia_hir::Builtin::Show
                | lumia_hir::Builtin::ReadStdin
                | lumia_hir::Builtin::StrTrim
                | lumia_hir::Builtin::StrSplit
                | lumia_hir::Builtin::StrSubstring
                | lumia_hir::Builtin::StrToLower
                | lumia_hir::Builtin::StrToUpper
                | lumia_hir::Builtin::ListJoin,
            ..
        } => Some(Type::String),
        Value::Builtin {
            name: lumia_hir::Builtin::AdtField,
            args, .. } if args.len() >= 2 => {
            let idx = match let_value_dfs(block, args[1].0) {
                Some(Value::Int(i)) if *i >= 0 => *i as usize,
                _ => {
                    // Index not a literal in this search root — do not abort the
                    // whole query (nested match arms share outer locals).
                    return if float_locals.contains(&id) {
                        Some(Type::Float)
                    } else {
                        None
                    };
                }
            };
            let parent = local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            match parent {
                Some(Type::Adt { params, .. }) => params.get(idx).cloned(),
                Some(Type::Tuple(ts) | Type::TuplePrefix(ts)) => ts.get(idx).cloned(),
                _ if float_locals.contains(&id) => Some(Type::Float),
                _ => None,
            }
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ListAppend,
            args, .. } if args.len() >= 2 => {
            let elem = if float_locals.contains(&args[1].0) {
                Type::Float
            } else {
                local_heap_ty(
                    block,
                    args[1].0,
                    float_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    cap_tys,
                    seen,
                    seen_slots,
                )
                .unwrap_or(Type::Int)
            };
            let list = local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            Some(match list {
                Some(Type::List(e))
                    if matches!(e.as_ref(), Type::Int | Type::Var(_))
                        && !matches!(elem, Type::Int | Type::Var(_)) =>
                {
                    Type::List(Box::new(elem))
                }
                Some(Type::List(e)) => Type::List(Box::new(prefer_concrete_heap_ty(
                    e.as_ref().clone(),
                    elem,
                ))),
                Some(other) => other,
                None if !matches!(elem, Type::Int | Type::Var(_)) => {
                    Type::List(Box::new(elem))
                }
                None => Type::List(Box::new(Type::Int)),
            })
        }
        Value::Builtin {
            name: lumia_hir::Builtin::MapSet,
            args, .. } if args.len() >= 3 => {
            // `m.set(k,v)` / `xs.set(i,v)` — float values must upgrade Map/List ABI
            // (`toMap` acc loop, `mapOf(…).set(…)`).
            let key_ty = if float_locals.contains(&args[1].0) {
                Type::Float
            } else {
                local_heap_ty(
                    block,
                    args[1].0,
                    float_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    cap_tys,
                    seen,
                    seen_slots,
                )
                .unwrap_or(Type::Int)
            };
            let val_ty = if float_locals.contains(&args[2].0) {
                Type::Float
            } else {
                local_heap_ty(
                    block,
                    args[2].0,
                    float_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    cap_tys,
                    seen,
                    seen_slots,
                )
                .unwrap_or(Type::Int)
            };
            match local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::List(e)) => Some(Type::List(Box::new(prefer_concrete_heap_ty(
                    e.as_ref().clone(),
                    val_ty,
                )))),
                Some(Type::Map(k, v)) => Some(Type::Map(
                    Box::new(prefer_concrete_heap_ty(k.as_ref().clone(), key_ty)),
                    Box::new(prefer_concrete_heap_ty(v.as_ref().clone(), val_ty)),
                )),
                // Do **not** mirror value_ty's Int-key→List guess: empty/`mapOf`
                // receivers that still look open become Map once `.set` is seen.
                _ => Some(Type::Map(Box::new(key_ty), Box::new(val_ty))),
            }
        }
        Value::Builtin {
            name: lumia_hir::Builtin::MapRemove,
            args, .. } if args.len() >= 2 => {
            let key_ty = if float_locals.contains(&args[1].0) {
                Type::Float
            } else {
                local_heap_ty(
                    block,
                    args[1].0,
                    float_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    cap_tys,
                    seen,
                    seen_slots,
                )
                .unwrap_or(Type::Int)
            };
            match local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::List(e)) => Some(Type::List(e)),
                Some(Type::Map(k, v)) => Some(Type::Map(
                    Box::new(prefer_concrete_heap_ty(k.as_ref().clone(), key_ty)),
                    v,
                )),
                _ => Some(Type::Map(Box::new(key_ty), Box::new(Type::Int))),
            }
        }
        Value::Builtin {
            name: lumia_hir::Builtin::TaskSpawn,
            args, .. } if !args.is_empty() => {
            let elem = fun_ret_of_local(block, args[0].0, fun_ret_tys, seen).unwrap_or(Type::Int);
            Some(Type::Task(Box::new(elem)))
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ListParFold,
            args, .. } if args.len() >= 2 => {
            // Acc type: Float init / Float callback / List[Float] elems → Float.
            if float_locals.contains(&args[1].0) {
                return Some(Type::Float);
            }
            if let Some(Type::List(e)) = local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                if matches!(e.as_ref(), Type::Float) {
                    return Some(Type::Float);
                }
            }
            local_heap_ty(
                block,
                args[1].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            )
            .or_else(|| {
                fun_ret_of_local(block, args[2].0, fun_ret_tys, seen).filter(|t| {
                    matches!(t, Type::Float | Type::Bool | Type::String | Type::Char)
                })
            })
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ListParMap,
            args, .. } if args.len() >= 2 => {
            // Result elem follows the callback Fun ret (`map { x -> x + 1.0 }`).
            // Without this, `spawn { join().map(…) }` falls through to List[Int]
            // and later `+` does integer add on IEEE bits (overflow).
            let from_cb = fun_ret_of_local(block, args[1].0, fun_ret_tys, seen).or_else(|| {
                match local_heap_ty(
                    block,
                    args[1].0,
                    float_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    cap_tys,
                    seen,
                    seen_slots,
                ) {
                    Some(Type::Fun(_, r, _)) => Some(*r),
                    _ => None,
                }
            });
            let from_list = match local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::List(e)) => Some(*e),
                _ => None,
            };
            let elem = match (from_cb, from_list) {
                (Some(Type::Float), _) => Type::Float,
                (Some(Type::Int | Type::Var(_)), Some(Type::Float)) | (None, Some(Type::Float)) => {
                    Type::Float
                }
                (Some(e), _) => e,
                (None, Some(e)) => e,
                (None, None) => Type::Int,
            };
            Some(Type::List(Box::new(elem)))
        }
        Value::Builtin {
            name: lumia_hir::Builtin::TaskJoin,
            args, .. } if !args.is_empty() => {
            match local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::Task(e)) => Some(*e),
                _ => None,
            }
        }
        Value::Builtin {
            name: lumia_hir::Builtin::TaskJoinOpt,
            args, .. } if !args.is_empty() => {
            // `joinOpt()` → Option[T] from Task[T] (needed for `alt listOf()` cap typing).
            match local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::Task(e)) => Some(Type::Adt {
                    name: lumia_hir::OPTION.name.into(),
                    params: vec![*e],
                }),
                _ => None,
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            // Resolve arm results via `block` (search root), not the nested
            // arm block alone — arm bodies reference outer locals (flatMap
            // `AdtField` of a `ListGet` defined in the loop body).
            let then_ty = then_block.result.and_then(|Local(r)| {
                local_heap_ty(
                    block,
                    r,
                    float_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    cap_tys,
                    seen,
                    seen_slots,
                )
            });
            let else_ty = else_block.result.and_then(|Local(r)| {
                local_heap_ty(
                    block,
                    r,
                    float_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    cap_tys,
                    seen,
                    seen_slots,
                )
            });
            join_match_heap_tys(
                then_ty,
                else_ty,
                block_result_is_bottom(then_block),
                block_result_is_bottom(else_block),
            )
        }
        _ => None,
    }
}

fn fun_ty_from_tables(
    name: &str,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
) -> Option<Type> {
    super::fun_ty_from_tables_tls(name, fun_ret_tys, fun_param_tys)
}

fn fun_ret_of_local(
    block: &Block,
    id: u32,
    fun_ret_tys: &HashMap<String, Type>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    match let_value_dfs(block, id)? {
        Value::Local(Local(src)) => fun_ret_of_local(block, *src, fun_ret_tys, seen),
        Value::FunRef(name) | Value::AllocClosure { fun: name, .. } => {
            fun_ret_tys.get(name).cloned()
        }
        _ => None,
    }
}

fn join_match_heap_tys(
    then_ty: Option<Type>,
    else_ty: Option<Type>,
    then_bottom: bool,
    else_bottom: bool,
) -> Option<Type> {
    if then_bottom {
        return else_ty;
    }
    if else_bottom {
        return then_ty;
    }
    match (then_ty, else_ty) {
        (Some(a), Some(b)) => join_heap_tys(&a, &b),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

fn join_heap_tys(a: &Type, b: &Type) -> Option<Type> {
    if a == b {
        return Some(a.clone());
    }
    match (a, b) {
        // MatchFail / empty arm: Unit is bottom.
        (Type::Unit, other) | (other, Type::Unit) => Some(other.clone()),
        // `Err("e") alt 9.5`: then=AdtField(String) from Err-only AllocAdt params,
        // else=Float — prefer Float (same as `join_value_tys` for println ABI).
        (Type::Float, other) | (other, Type::Float)
            if matches!(
                other,
                Type::Int
                    | Type::Var(_)
                    | Type::Bool
                    | Type::String
                    | Type::Char
                    | Type::Float
            ) =>
        {
            Some(Type::Float)
        }
        (Type::Int | Type::Var(_), other) => Some(other.clone()),
        (other, Type::Int | Type::Var(_)) => Some(other.clone()),
        (
            Type::Adt {
                name: n1,
                params: p1,
            },
            Type::Adt {
                name: n2,
                params: p2,
            },
        ) if n1 == n2 => {
            let n = p1.len().max(p2.len());
            let mut params = Vec::with_capacity(n);
            for i in 0..n {
                let x = p1.get(i).cloned().unwrap_or(Type::Int);
                let y = p2.get(i).cloned().unwrap_or(Type::Int);
                params.push(prefer_concrete_heap_ty(x, y));
            }
            Some(Type::Adt {
                name: n1.clone(),
                params,
            })
        }
        (Type::List(e1), Type::List(e2)) => {
            Some(Type::List(Box::new(prefer_concrete_heap_ty(
                e1.as_ref().clone(),
                e2.as_ref().clone(),
            ))))
        }
        (Type::Set(e1), Type::Set(e2)) => {
            Some(Type::Set(Box::new(prefer_concrete_heap_ty(
                e1.as_ref().clone(),
                e2.as_ref().clone(),
            ))))
        }
        (Type::Task(e1), Type::Task(e2)) => {
            Some(Type::Task(Box::new(prefer_concrete_heap_ty(
                e1.as_ref().clone(),
                e2.as_ref().clone(),
            ))))
        }
        (Type::Channel(e1), Type::Channel(e2)) => {
            Some(Type::Channel(Box::new(prefer_concrete_heap_ty(
                e1.as_ref().clone(),
                e2.as_ref().clone(),
            ))))
        }
        (Type::Map(k1, v1), Type::Map(k2, v2)) => Some(Type::Map(
            Box::new(prefer_concrete_heap_ty(k1.as_ref().clone(), k2.as_ref().clone())),
            Box::new(prefer_concrete_heap_ty(v1.as_ref().clone(), v2.as_ref().clone())),
        )),
        _ => None,
    }
}

pub fn prefer_concrete_heap_ty(x: Type, y: Type) -> Type {
    if x == y {
        return x;
    }
    match (&x, &y) {
        // Fun ABI must not collapse to Float (curried compose / make(k) rets).
        (Type::Fun(p1, r1, e1), Type::Fun(p2, r2, e2)) => {
            let n = p1.len().max(p2.len());
            let mut params = Vec::with_capacity(n);
            for i in 0..n {
                let a = p1.get(i).cloned().unwrap_or(Type::Int);
                let b = p2.get(i).cloned().unwrap_or(Type::Int);
                params.push(prefer_concrete_heap_ty(a, b));
            }
            Type::Fun(
                params,
                Box::new(prefer_concrete_heap_ty(r1.as_ref().clone(), r2.as_ref().clone())),
                if e1.has_io() || e2.has_io() {
                    Effect::io()
                } else {
                    Effect::pure()
                },
            )
        }
        (Type::Fun(_, _, _), _) => x.clone(),
        (_, Type::Fun(_, _, _)) => y.clone(),
        // Lift may-heap placeholder `List(Int)` must yield to Map/Set/Task/String/…
        // (`mapOf(…).set` was stuck as List → `.get` used list indexing;
        // spawn String was stuck as List → `.len()` used `lumia_list_len`).
        (
            Type::List(e),
            other @ (Type::Map(_, _)
            | Type::Set(_)
            | Type::Task(_)
            | Type::Channel(_)
            | Type::Adt { .. }
            | Type::String
            | Type::Char),
        ) if matches!(e.as_ref(), Type::Int | Type::Var(_)) => other.clone(),
        (
            other @ (Type::Map(_, _)
            | Type::Set(_)
            | Type::Task(_)
            | Type::Channel(_)
            | Type::Adt { .. }
            | Type::String
            | Type::Char),
            Type::List(e),
        ) if matches!(e.as_ref(), Type::Int | Type::Var(_)) => other.clone(),
        (Type::Float, _) | (_, Type::Float) => Type::Float,
        (Type::Int | Type::Var(_), other) => other.clone(),
        (other, Type::Int | Type::Var(_)) => other.clone(),
        (
            Type::Adt {
                name: n1,
                params: p1,
            },
            Type::Adt {
                name: n2,
                params: p2,
            },
        ) if n1 == n2 => join_heap_tys(&x, &y).unwrap_or(x),
        (Type::List(_), Type::List(_))
        | (Type::Set(_), Type::Set(_))
        | (Type::Task(_), Type::Task(_))
        | (Type::Channel(_), Type::Channel(_))
        | (Type::Map(_, _), Type::Map(_, _)) => join_heap_tys(&x, &y).unwrap_or(x),
        _ => x,
    }
}

fn alloc_elems_ty(
    block: &Block,
    elems: &[Local],
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    seen_slots: &mut HashSet<String>,
) -> Type {
    if elems.is_empty() {
        return Type::Int;
    }
    if elems.iter().all(|e| float_locals.contains(&e.0)) {
        return Type::Float;
    }
    let mut acc: Option<Type> = None;
    for e in elems {
        let t = if float_locals.contains(&e.0) {
            Type::Float
        } else {
            local_heap_ty(
                block,
                e.0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            )
            .unwrap_or(Type::Int)
        };
        acc = Some(match acc {
            None => t,
            Some(prev) => prefer_concrete_heap_ty(prev, t),
        });
    }
    acc.unwrap_or(Type::Int)
}

/// Concrete return type from a body's result `Call` / alias chain (post-lower /
/// post-mono callee tables), so `spawn { dbl(1.5) }.join()` keeps Float ABI.
pub(crate) fn block_result_callee_ty(
    block: &Block,
    fun_ret_tys: &HashMap<String, Type>,
) -> Option<Type> {
    let Local(r) = block.result?;
    local_callee_ty(block, r, fun_ret_tys, &mut HashSet::default())
}

/// `icall` of a `ClosureCap` whose outer capture is a known FunRef/closure.
/// Covers `spawn { dbl(1.5) }` when `dbl` is a local lambda (env capture).
pub(super) fn block_result_icall_cap_ty(
    block: &Block,
    cap_srcs: &[Local],
    funref_locals: &HashMap<u32, String>,
    fun_ret_tys: &HashMap<String, Type>,
) -> Option<Type> {
    let Local(r) = block.result?;
    local_icall_cap_ty(
        block,
        r,
        cap_srcs,
        funref_locals,
        fun_ret_tys,
        &mut HashSet::default(),
    )
}

/// Resolve `IndirectCall` → `ClosureCap(index)` → captured fun name → ret.
pub(super) fn block_result_icall_cap_ty_by_index(
    block: &Block,
    cap_funs: &HashMap<u32, String>,
    fun_ret_tys: &HashMap<String, Type>,
) -> Option<Type> {
    let Local(r) = block.result?;
    local_icall_cap_ty_by_index(block, r, cap_funs, fun_ret_tys, &mut HashSet::default())
}

/// Body result is `FunRef` / `AllocClosure` — keep a `Fun` ret so
/// `spawn { { x -> x * 2.0 } }.join()(1.5)` uses Float icall ABI.
pub(super) fn block_result_fun_ty(
    block: &Block,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
) -> Option<Type> {
    let Local(r) = block.result?;
    local_fun_ty(block, r, fun_ret_tys, fun_param_tys, &mut HashSet::default())
}

/// Known HOF shapes for spawn/icall Float ABI (apply / compose / id).
#[derive(Default, Clone)]
pub(super) struct HofSets {
    pub apply: HashSet<String>,
    pub compose: HashSet<String>,
    pub id: HashSet<String>,
}

impl HofSets {
    pub(super) fn note(&mut self, name: &str, params: &[Local], body: &Block) {
        if is_apply_hof(params, body) {
            self.apply.insert(name.to_string());
        }
        if is_compose_hof(params, body) {
            self.compose.insert(name.to_string());
        }
        if is_id_hof(params, body) {
            self.id.insert(name.to_string());
        }
    }

    pub(super) fn from_module_funs<'a, I>(funs: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a [Local], &'a Block)>,
    {
        let mut h = Self::default();
        for (name, params, body) in funs {
            h.note(name, params, body);
        }
        h
    }
}

/// `{ f, x -> f(x) }` / `{ f, g, x -> g(f(x)) }` pipeline HOFs.
pub(super) fn block_result_known_hof_ty(
    block: &Block,
    hof: &HofSets,
    fun_ret_tys: &HashMap<String, Type>,
    cap_funs: Option<&HashMap<u32, String>>,
) -> Option<Type> {
    let Local(r) = block.result?;
    local_known_hof_ty(block, r, hof, fun_ret_tys, cap_funs, &mut HashSet::default())
}

/// True when the body is exactly `icall f(args…)` with `f`/`args` = formals
/// (optional leading env param for lifted closures).
pub(super) fn is_apply_hof(params: &[Local], body: &Block) -> bool {
    if params.len() < 2 {
        return false;
    }
    let Some(Local(r)) = body.result else {
        return false;
    };
    let Some((callee, args)) = resolve_icall(body, r) else {
        return false;
    };
    // Nullary-env: params[0] is the fun, params[1..] are args.
    if local_aliases(body, callee, params[0].0)
        && args.len() == params.len() - 1
        && args
            .iter()
            .zip(params[1..].iter())
            .all(|(a, p)| local_aliases(body, *a, p.0))
    {
        return true;
    }
    // Env closure: params[0]=env, params[1]=fun, params[2..]=args.
    if params.len() >= 3
        && local_aliases(body, callee, params[1].0)
        && args.len() == params.len() - 2
        && args
            .iter()
            .zip(params[2..].iter())
            .all(|(a, p)| local_aliases(body, *a, p.0))
    {
        return true;
    }
    false
}

/// `{ f, g, x -> g(f(x)) }` (optional leading env).
pub(super) fn is_compose_hof(params: &[Local], body: &Block) -> bool {
    let (f, g, x) = match params.len() {
        3 => (params[0], params[1], params[2]),
        4 => (params[1], params[2], params[3]),
        _ => return false,
    };
    let Some(Local(r)) = body.result else {
        return false;
    };
    let Some((g_cal, g_args)) = resolve_icall(body, r) else {
        return false;
    };
    if g_args.len() != 1 || !local_aliases(body, g_cal, g.0) {
        return false;
    }
    let Some((f_cal, f_args)) = resolve_icall(body, g_args[0]) else {
        return false;
    };
    f_args.len() == 1
        && local_aliases(body, f_cal, f.0)
        && local_aliases(body, f_args[0], x.0)
}

/// `{ f -> f }` identity (optional leading env).
pub(super) fn is_id_hof(params: &[Local], body: &Block) -> bool {
    let p = match params.len() {
        1 => params[0],
        2 => params[1],
        _ => return false,
    };
    let Some(Local(r)) = body.result else {
        return false;
    };
    local_aliases(body, r, p.0)
}

fn resolve_icall(block: &Block, id: u32) -> Option<(u32, Vec<u32>)> {
    let mut seen = HashSet::default();
    let mut cur = id;
    loop {
        if !seen.insert(cur) {
            return None;
        }
        match let_value(block, cur)? {
            Value::Local(Local(src)) => cur = *src,
            Value::IndirectCall { callee, args } => {
                return Some((callee.0, args.iter().map(|a| a.0).collect()));
            }
            _ => return None,
        }
    }
}

fn ret_ty_from_callee_table(t: &Type) -> Option<Type> {
    match t {
        // `List[Int]` is the lift may-heap placeholder — not a real payload type.
        Type::List(e) if matches!(e.as_ref(), Type::Int) => None,
        Type::Float
        | Type::String
        | Type::Char
        | Type::List(_)
        | Type::Map(_, _)
        | Type::Set(_)
        | Type::Adt { .. }
        | Type::Task(_)
        | Type::Channel(_)
        | Type::Fun(_, _, _) => Some(t.clone()),
        _ => None,
    }
}

fn let_value(block: &Block, id: u32) -> Option<&Value> {
    for op in &block.ops {
        if let Op::Let { local, value, .. } = op {
            if local.0 == id {
                return Some(value);
            }
        }
    }
    None
}

fn local_aliases(block: &Block, id: u32, target: u32) -> bool {
    let mut seen = HashSet::default();
    let mut cur = id;
    loop {
        if cur == target {
            return true;
        }
        if !seen.insert(cur) {
            return false;
        }
        match let_value(block, cur) {
            Some(Value::Local(Local(src))) => cur = *src,
            _ => return false,
        }
    }
}

fn local_fun_ty(
    block: &Block,
    id: u32,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    match let_value(block, id)? {
        Value::Local(Local(src)) => local_fun_ty(block, *src, fun_ret_tys, fun_param_tys, seen),
        Value::FunRef(name) => {
            let ret = fun_ret_tys.get(name)?.clone();
            let params = fun_param_tys.get(name).cloned().unwrap_or_default();
            Some(Type::Fun(params, Box::new(ret), Effect::pure()))
        }
        Value::AllocClosure { fun, .. } => {
            let ret = fun_ret_tys.get(fun)?.clone();
            // Drop env pointer param for the user-facing Fun type.
            let params = fun_param_tys.get(fun).cloned().unwrap_or_default();
            let params = if params.len() > 1 {
                params[1..].to_vec()
            } else {
                Vec::new()
            };
            Some(Type::Fun(params, Box::new(ret), Effect::pure()))
        }
        _ => None,
    }
}

fn local_known_hof_ty(
    block: &Block,
    id: u32,
    hof: &HofSets,
    fun_ret_tys: &HashMap<String, Type>,
    cap_funs: Option<&HashMap<u32, String>>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    match let_value(block, id)? {
        Value::Local(Local(src)) => {
            local_known_hof_ty(block, *src, hof, fun_ret_tys, cap_funs, seen)
        }
        Value::IndirectCall { callee, args } => {
            let cal = resolve_fun_name(block, callee.0, cap_funs, hof)?;
            if hof.apply.contains(&cal) {
                let farg = args.first()?;
                let fname = resolve_fun_name(block, farg.0, cap_funs, hof)?;
                return fun_ret_tys.get(&fname).and_then(ret_ty_from_callee_table);
            }
            if hof.compose.contains(&cal) && args.len() >= 2 {
                // andThen(f, g, x): result type is g's return.
                let g_arg = &args[args.len() - 2];
                let fname = resolve_fun_name(block, g_arg.0, cap_funs, hof)?;
                return fun_ret_tys.get(&fname).and_then(ret_ty_from_callee_table);
            }
            None
        }
        _ => None,
    }
}

fn resolve_fun_name(
    block: &Block,
    id: u32,
    cap_funs: Option<&HashMap<u32, String>>,
    hof: &HofSets,
) -> Option<String> {
    let mut seen = HashSet::default();
    let mut cur = id;
    loop {
        if !seen.insert(cur) {
            return None;
        }
        match let_value(block, cur)? {
            Value::Local(Local(src)) => cur = *src,
            Value::FunRef(name) | Value::AllocClosure { fun: name, .. } => {
                return Some(name.clone());
            }
            Value::ClosureCap { index, .. } => {
                return cap_funs.and_then(|m| m.get(index).cloned());
            }
            Value::IndirectCall { callee, args } => {
                let cal = resolve_fun_name(block, callee.0, cap_funs, hof)?;
                // id(f) → f; apply returning a Fun is uncommon — treat first arg.
                if hof.id.contains(&cal) {
                    let farg = args.first()?;
                    cur = farg.0;
                    continue;
                }
                if hof.apply.contains(&cal) {
                    let farg = args.first()?;
                    cur = farg.0;
                    continue;
                }
                return None;
            }
            _ => return None,
        }
    }
}

fn local_callee_ty(
    block: &Block,
    id: u32,
    fun_ret_tys: &HashMap<String, Type>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    for op in &block.ops {
        let Op::Let { local, value, .. } = op else {
            continue;
        };
        if local.0 != id {
            continue;
        }
        return match value {
            Value::Local(Local(src)) => local_callee_ty(block, *src, fun_ret_tys, seen),
            Value::Call { fun, .. } => fun_ret_tys.get(fun).and_then(ret_ty_from_callee_table),
            _ => None,
        };
    }
    None
}

fn local_icall_cap_ty(
    block: &Block,
    id: u32,
    cap_srcs: &[Local],
    funref_locals: &HashMap<u32, String>,
    fun_ret_tys: &HashMap<String, Type>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    for op in &block.ops {
        let Op::Let { local, value, .. } = op else {
            continue;
        };
        if local.0 != id {
            continue;
        }
        return match value {
            Value::Local(Local(src)) => {
                local_icall_cap_ty(block, *src, cap_srcs, funref_locals, fun_ret_tys, seen)
            }
            Value::IndirectCall { callee, .. } => {
                let idx = closure_cap_index(block, callee.0, &mut HashSet::default())?;
                let src = cap_srcs.get(idx as usize)?;
                let name = funref_locals.get(&src.0)?;
                fun_ret_tys.get(name).and_then(ret_ty_from_callee_table)
            }
            _ => None,
        };
    }
    None
}

fn local_icall_cap_ty_by_index(
    block: &Block,
    id: u32,
    cap_funs: &HashMap<u32, String>,
    fun_ret_tys: &HashMap<String, Type>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    for op in &block.ops {
        let Op::Let { local, value, .. } = op else {
            continue;
        };
        if local.0 != id {
            continue;
        }
        return match value {
            Value::Local(Local(src)) => {
                local_icall_cap_ty_by_index(block, *src, cap_funs, fun_ret_tys, seen)
            }
            Value::IndirectCall { callee, .. } => {
                let idx = closure_cap_index(block, callee.0, &mut HashSet::default())?;
                let name = cap_funs.get(&idx)?;
                fun_ret_tys.get(name).and_then(ret_ty_from_callee_table)
            }
            _ => None,
        };
    }
    None
}

fn closure_cap_index(block: &Block, id: u32, seen: &mut HashSet<u32>) -> Option<u32> {
    if !seen.insert(id) {
        return None;
    }
    for op in &block.ops {
        let Op::Let { local, value, .. } = op else {
            continue;
        };
        if local.0 != id {
            continue;
        }
        return match value {
            Value::Local(Local(src)) => closure_cap_index(block, *src, seen),
            Value::ClosureCap { index, .. } => Some(*index),
            _ => None,
        };
    }
    None
}



#[cfg(test)]
mod tests {
    use super::is_apply_hof;
    use crate::compile_source_to_core;

    #[test]
    fn spawn_string_cap_closure_ret_is_string() {
        let str2 = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val prefix = "pre"
        val f = spawn { { s -> prefix.concat(s) } }.join()
        println(f("x").len())
    }
}
"#,
        )
        .expect("str2");
        let lam0 = str2
            .functions
            .iter()
            .find(|f| f.name == "__lam_0")
            .expect("__lam_0");
        assert!(
            matches!(lam0.ret_ty, lumia_ty::Type::String),
            "spawned concat lam ret should be String, got {:?}",
            lam0.ret_ty
        );
    }


    #[test]
    fn spawn_some_true_option_bool() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val o = spawn { Some(true) }.join()
        println(o)
    }
}
"#,
        )
        .expect("core");
        let lam = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn lam");
        assert!(
            matches!(
                &lam.ret_ty,
                lumia_ty::Type::Adt { name, params }
                    if lumia_hir::is_option(name) && params.first().is_some_and(|p| matches!(p, lumia_ty::Type::Bool))
            ),
            "spawn Some(true) ret should be Option[Bool], got {:?}",
            lam.ret_ty
        );
    }

    #[test]
    fn spawn_two_float_folds_sum_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val xs = listOf(1.0, 2.0, 3.0)
        val ys = listOf(1.0, 2.0)
        val s = spawn { xs.fold(0.0, { a, b -> a + b }) + ys.fold(0.0, { a, b -> a + b }) }.join()
        println(s)
    }
}
"#,
        )
        .expect("core");
        assert!(
            core.functions.iter().any(|f| f.name.starts_with("__lam_")
                && matches!(f.ret_ty, lumia_ty::Type::Float)
                && f.params.len() <= 1),
            "spawn body should return Float"
        );
    }

    #[test]
    fn detect_apply_hof_shape() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val apply = { f, x -> f(x) }
        println(spawn { apply({ y -> y * 2.0 }, 1.5) }.join())
    }
}
"#,
        )
        .expect("core");
        let apply = core
            .functions
            .iter()
            .find(|f| f.name == "__lam_0")
            .expect("apply");
        assert!(is_apply_hof(&apply.params, &apply.body));
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name == "__lam_2")
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Float),
            "got {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn detect_compose_hof_float() {
        use super::is_compose_hof;
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val andThen = { f, g, x -> g(f(x)) }
        println(spawn { andThen({ a -> a + 1.0 }, { b -> b * 2.0 }, 1.5) }.join())
    }
}
"#,
        )
        .expect("core");
        let compose = core
            .functions
            .iter()
            .find(|f| is_compose_hof(&f.params, &f.body))
            .expect("compose");
        let spawn = core
            .functions
            .iter()
            .find(|f| {
                f.name.starts_with("__lam_")
                    && f.params.len() == 1
                    && f.body.ops.iter().any(|op| {
                        matches!(
                            op,
                            crate::ir::Op::Let {
                                value: crate::ir::Value::IndirectCall { args, .. },
                                ..
                            } if args.len() == 3
                        ) || matches!(
                            op,
                            crate::ir::Op::Let {
                                value: crate::ir::Value::Call { args, .. },
                                ..
                            } if args.len() == 3
                        )
                    })
            })
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Float),
            "compose spawn ret {:?}, compose={}",
            spawn.ret_ty,
            compose.name
        );
    }

    #[test]
    fn detect_id_through_apply_float() {
        use super::is_id_hof;
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val apply = { f, x -> f(x) }
        val id = { f -> f }
        println(spawn { apply(id({ y -> y * 2.0 }), 1.5) }.join())
    }
}
"#,
        )
        .expect("core");
        assert!(core.functions.iter().any(|f| is_id_hof(&f.params, &f.body)));
        let spawn = core
            .functions
            .iter()
            .find(|f| {
                f.name.starts_with("__lam_")
                    && f.params.len() == 1
                    && f.body.ops.iter().any(|op| {
                        matches!(
                            op,
                            crate::ir::Op::Let {
                                value: crate::ir::Value::IndirectCall { args, .. },
                                ..
                            } if args.len() == 2
                        ) || matches!(
                            op,
                            crate::ir::Op::Let {
                                value: crate::ir::Value::Call { args, .. },
                                ..
                            } if args.len() == 2
                        )
                    })
            })
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Float),
            "id∘apply spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_map_get_float_keeps_float_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn { listOf(1.5, 2.5).map({ x -> x * 2.0 }).get(0) }.join())
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Float),
            "map.get spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_if_float_keeps_float_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val t = spawn { if true { 1.5 } else { 2.5 } }
        println(t.join() * 2.0)
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Float),
            "if-float spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_fold_float_keeps_float_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn { listOf(1.5, 2.5).fold(0.0, { a, b -> a + b }) }.join())
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Float),
            "fold-float spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_filter_get_float_keeps_float_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn { listOf(1.5, 2.5).filter({ x -> x > 2.0 }).get(0) }.join())
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Float),
            "filter.get spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_match_float_keeps_float_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn {
            Some(1.5) match {
                Some(x) -> x * 2.0
                None -> 0.0
            }
        }.join())
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Float),
            "match-float spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_match_id_float_keeps_float_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn {
            Some(1.5) match {
                Some(x) -> x
                None -> 0.0
            }
        }.join())
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Float),
            "match-id-float spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_match_option_float_keeps_adt_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val o = spawn {
            Some(1.5) match {
                Some(x) -> Some(x * 2.0)
                None -> None
            }
        }.join()
        o match {
            Some(v) -> println(v)
            None -> println(0)
        }
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(
                &spawn.ret_ty,
                lumia_ty::Type::Adt { name, params }
                    if lumia_hir::is_option(name)
                        && params.first().is_some_and(|p| matches!(p, lumia_ty::Type::Float))
            ),
            "match-option-float spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_list_option_float_keeps_elem_adt() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val xs = spawn { listOf(Some(1.5)) }.join()
        xs.get(0) match {
            Some(v) -> println(v * 2.0)
            None -> println(0)
        }
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(
                &spawn.ret_ty,
                lumia_ty::Type::List(e)
                    if matches!(
                        e.as_ref(),
                        lumia_ty::Type::Adt { name, params }
                            if lumia_hir::is_option(name)
                                && params.first().is_some_and(|p| matches!(p, lumia_ty::Type::Float))
                    )
            ),
            "list-option-float spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_for_float_acc_keeps_float_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn {
            var s = 0.0
            for x in listOf(1.0, 2.0, 3.0) { s = s + x }
            s
        }.join())
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Float),
            "for-float-acc spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_mut_fun_call_float_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn {
            var f = { x -> x * 2.0 }
            f(1.5)
        }.join())
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Float),
            "mut-fun-call spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_bool_cmp_keeps_int_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn { 1.5 > 1.0 }.join())
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Bool),
            "bool-cmp spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_mut_bool_keeps_bool_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn {
            var b = false
            b = 1.5 > 1.0
            b
        }.join())
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn");
        assert!(
            matches!(spawn.ret_ty, lumia_ty::Type::Bool),
            "mut-bool spawn ret {:?}",
            spawn.ret_ty
        );
    }

    #[test]
    fn spawn_nested_task_keeps_task_float_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val outer = spawn { spawn { 1.5 * 2.0 } }
        println(outer.join().join())
    }
}
"#,
        )
        .expect("core");
        let outer = core
            .functions
            .iter()
            .find(|f| {
                f.name.starts_with("__lam_")
                    && f.params.is_empty()
                    && f.body.ops.iter().any(|op| {
                        matches!(
                            op,
                            crate::ir::Op::Let {
                                value: crate::ir::Value::Builtin {
                                    name: lumia_hir::Builtin::TaskSpawn,
                                    ..
                                },
                                ..
                            }
                        )
                    })
            })
            .expect("outer spawn");
        assert!(
            matches!(
                &outer.ret_ty,
                lumia_ty::Type::Task(e) if matches!(e.as_ref(), lumia_ty::Type::Float)
            ),
            "nested spawn ret should be Task[Float], got {:?}",
            outer.ret_ty
        );
    }

    #[test]
    fn map_spawn_join_list_append_task_float() {
        use crate::value_ty::{infer_value_ty_ctx, CodegenTypeTables, InferValueCtx};
        use crate::Op;
        use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
        use lumia_ty::Type;
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val xs = listOf(1.5, 2.5).map({ x -> spawn { x * 2.0 } })
        println(xs.get(0).join())
    }
}
"#,
        )
        .expect("core");
        let fun_ret_tys: HashMap<_, _> = core
            .functions
            .iter()
            .map(|f| (f.name.clone(), f.ret_ty.clone()))
            .collect();
        let fun_param_tys: HashMap<_, _> = core
            .functions
            .iter()
            .map(|f| (f.name.clone(), f.param_tys.clone()))
            .collect();
        let main = core.functions.iter().find(|f| f.name == "main").unwrap();
        let mut local_tys: HashMap<u32, Type> = HashMap::default();
        let mut slot_tys: HashMap<String, Type> = HashMap::default();
        let fun_param0_identity = HashSet::default();
        let mut funref_locals: HashMap<u32, String> = HashMap::default();
        let local_int_consts: HashMap<u32, i64> = HashMap::default();
        let sum_max_arity: HashMap<String, usize> = HashMap::default();
        fn walk(
            ops: &[Op],
            local_tys: &mut HashMap<u32, Type>,
            slot_tys: &mut HashMap<String, Type>,
            funref_locals: &mut HashMap<u32, String>,
            fun_ret_tys: &HashMap<String, Type>,
            fun_param_tys: &HashMap<String, Vec<Type>>,
            fun_param0_identity: &HashSet<String>,
            local_int_consts: &HashMap<u32, i64>,
            sum_max_arity: &HashMap<String, usize>,
            hint: Option<&Type>,
        ) {
            for op in ops {
                match op {
                    Op::Let { local, value, .. } => {
                        crate::for_each_nested_block(value, &mut |b| {
                            walk(
                                &b.ops,
                                local_tys,
                                slot_tys,
                                funref_locals,
                                fun_ret_tys,
                                fun_param_tys,
                                fun_param0_identity,
                                local_int_consts,
                                sum_max_arity,
                                hint,
                            );
                        });
                        let ty = infer_value_ty_ctx(
                            value,
                            InferValueCtx::full(
                                local_tys,
                                CodegenTypeTables {
                                    slot_tys,
                                    fun_ret_tys,
                                    fun_param_tys,
                                    fun_param0_identity,
                                    funref_locals,
                                    local_int_consts,
                                    sum_max_arity,
                                    channel_elem_hint: hint,
                                },
                            ),
                            None,
                        );
                        local_tys.insert(local.0, ty);
                    }
                    Op::Assign { name, value } => {
                        if let Some(ty) = local_tys.get(&value.0).cloned() {
                            slot_tys.insert(name.clone(), ty);
                        }
                    }
                    _ => {}
                }
            }
        }
        walk(
            &main.body.ops,
            &mut local_tys,
            &mut slot_tys,
            &mut funref_locals,
            &fun_ret_tys,
            &fun_param_tys,
            &fun_param0_identity,
            &local_int_consts,
            &sum_max_arity,
            core.channel_elem_hint.as_ref(),
        );
        let join_ty = local_tys
            .values()
            .find(|t| matches!(t, Type::Float))
            .cloned();
        assert!(
            join_ty.is_some(),
            "expected Float join result in local_tys, got {:?}",
            local_tys
                .iter()
                .filter(|(_, t)| matches!(t, Type::Task(_) | Type::List(_) | Type::Float))
                .collect::<Vec<_>>()
        );
        assert!(
            local_tys.values().any(|t| matches!(t, Type::List(e) if matches!(e.as_ref(), Type::Task(_)))),
            "map acc should become List[Task[_]], slot_tys={slot_tys:?}"
        );
    }

    #[test]
    fn flatmap_list_fun_println_ty() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    val xs = listOf(1.0, 2.0)
    val fs = xs.flatMap({ x -> listOf({ y -> x + y }) })
    println(fs.get(0)(1.0))
}
"#,
        )
        .expect("core");
        for f in &core.functions {
            if f.name == "main" || f.name.starts_with("__lam") {
                eprintln!("{} ret={:?} params={:?}", f.name, f.ret_ty, f.param_tys);
            }
        }
        // Check main has Call to $Float or Float println path via funref
        let main = core.functions.iter().find(|f| f.name == "main").unwrap();
        for op in &main.body.ops {
            if let crate::Op::Let { local, value, .. } = op {
                match value {
                    crate::Value::Call { fun, args } => eprintln!("  %{} Call {} {:?}", local.0, fun, args),
                    crate::Value::IndirectCall { callee, args } => eprintln!("  %{} ICall %{} {:?}", local.0, callee.0, args),
                    crate::Value::Builtin { name: lumia_hir::Builtin::ListGet, args, .. } => eprintln!("  %{} ListGet {:?}", local.0, args),
                    crate::Value::Builtin { name: lumia_hir::Builtin::ListConcat, args, .. } => eprintln!("  %{} ListConcat {:?}", local.0, args),
                    crate::Value::FunRef(n) => eprintln!("  %{} FunRef {}", local.0, n),
                    crate::Value::AllocClosure { fun, .. } => eprintln!("  %{} AllocClosure {}", local.0, fun),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn spawn_list_of_float_fun_ret() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        val fs = spawn { listOf({ x -> x * 2.0 }) }.join()
        println(fs.get(0)(1.5))
    }
}
"#,
        )
        .expect("core");
        let spawn = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
            .expect("spawn_lam");
        assert!(
            matches!(
                &spawn.ret_ty,
                lumia_ty::Type::List(e) if matches!(e.as_ref(), lumia_ty::Type::Fun(ps, r, _) if ps.len() == 1 && matches!(ps[0], lumia_ty::Type::Float) && matches!(r.as_ref(), lumia_ty::Type::Float))
            ),
            "got {:?}",
            spawn.ret_ty
        );
    }
}
