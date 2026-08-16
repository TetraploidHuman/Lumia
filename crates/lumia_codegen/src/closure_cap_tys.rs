//! Pre-collect capture types at `AllocClosure` sites so callees can type
//! `ClosureCap` even when the callee body is emitted before its callers.

use lumia_core::{
    infer_value_ty_ctx, Block, CodegenTypeTables, CoreModule, InferValueCtx, Op, Value,
};
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// `(lifted_fun → capture_index → ty)` from every `AllocClosure` in `core`.
pub(crate) fn collect_closure_cap_tys(
    core: &CoreModule,
) -> HashMap<String, HashMap<u32, Type>> {
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
    let fun_param0_identity: HashSet<String> = core
        .functions
        .iter()
        .filter(|f| crate::core_fun_is_param0_identity(f))
        .map(|f| f.name.clone())
        .collect();
    let mut out: HashMap<String, HashMap<u32, Type>> = HashMap::default();
    // Change-flag fixpoint (capped): outer AllocClosure may depend on inner
    // ClosureCap typing from a prior round.
    for _ in 0..lumia_abi::CLOSURE_CAP_TY_ROUNDS {
        let before = out.clone();
        for fun in &core.functions {
            let mut local_tys: HashMap<u32, Type> = HashMap::default();
            let mut slot_tys: HashMap<String, Type> = HashMap::default();
            let mut funref_locals: HashMap<u32, String> = HashMap::default();
            let local_int_consts: HashMap<u32, i64> = HashMap::default();
            for (i, ty) in fun.param_tys.iter().enumerate() {
                if let Some(p) = fun.params.get(i) {
                    local_tys.insert(p.0, ty.clone());
                }
            }
            walk_block(
                &fun.body,
                &fun.name,
                &mut local_tys,
                &mut slot_tys,
                &mut funref_locals,
                &fun_ret_tys,
                &fun_param_tys,
                &fun_param0_identity,
                &local_int_consts,
                &core.sum_max_arity,
                core.channel_elem_hint.as_ref(),
                &core.channel_elem_by_local,
                &mut out,
            );
        }
        if out == before {
            break;
        }
    }
    out
}

fn prefer_cap_ty(old: Type, new: Type) -> Type {
    match (&old, &new) {
        (Type::Fun(_, _, _), _) => old,
        (_, Type::Fun(_, _, _)) => new,
        (Type::Float, _) | (_, Type::Float) => Type::Float,
        (Type::Int | Type::Var(_), other) => other.clone(),
        (other, Type::Int | Type::Var(_)) => other.clone(),
        _ => new,
    }
}

fn walk_block(
    block: &Block,
    current_fun: &str,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    funref_locals: &mut HashMap<u32, String>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    fun_param0_identity: &HashSet<String>,
    local_int_consts: &HashMap<u32, i64>,
    sum_max_arity: &HashMap<String, usize>,
    channel_elem_hint: Option<&Type>,
    channel_elem_by_local: &HashMap<u32, Type>,
    out: &mut HashMap<String, HashMap<u32, Type>>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                walk_value_nested(
                    value,
                    current_fun,
                    local_tys,
                    slot_tys,
                    funref_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    fun_param0_identity,
                    local_int_consts,
                    sum_max_arity,
                    channel_elem_hint,
                    channel_elem_by_local,
                    out,
                );
                let mut ty = infer_value_ty_ctx(
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
                            channel_elem_hint,
                        },
                    ),
                    None,
                );
                if matches!(
                    value,
                    Value::Builtin {
                        name: lumia_hir::Builtin::ChannelNew,
                        ..
                    }
                ) {
                    if let Some(elem) = channel_elem_by_local.get(&local.0) {
                        ty = Type::Channel(Box::new(elem.clone()));
                    }
                }
                // `ClosureCap` defaults to Int in value_ty; prefer the type recorded
                // when this lam was allocated (Fun / Float / …). Otherwise nested
                // `AllocClosure` re-captures Int and `a(x)+1.0` sitofps IEEE bits.
                if let Value::ClosureCap {
                    index, as_float, ..
                } = value
                {
                    if *as_float {
                        ty = Type::Float;
                    } else if let Some(t) = out.get(current_fun).and_then(|m| m.get(index)) {
                        ty = t.clone();
                    }
                }
                if let Value::AllocClosure { fun, captures } = value {
                    let entry = out.entry(fun.clone()).or_default();
                    for (i, e) in captures.iter().enumerate() {
                        if let Some(t) = local_tys.get(&e.0).cloned() {
                            let slot = entry.entry(i as u32).or_insert_with(|| t.clone());
                            *slot = prefer_cap_ty(slot.clone(), t);
                        }
                    }
                }
                local_tys.insert(local.0, ty);
                crate::funref::note_funref_local(
                    funref_locals,
                    local.0,
                    value,
                    crate::funref::AllocClosureFunref::Track,
                );
            }
            Op::Assign { name, value } => {
                if let Some(ty) = local_tys.get(&value.0).cloned() {
                    slot_tys.insert(name.clone(), ty);
                }
            }
            Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
}

fn walk_value_nested(
    value: &Value,
    current_fun: &str,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    funref_locals: &mut HashMap<u32, String>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    fun_param0_identity: &HashSet<String>,
    local_int_consts: &HashMap<u32, i64>,
    sum_max_arity: &HashMap<String, usize>,
    channel_elem_hint: Option<&Type>,
    channel_elem_by_local: &HashMap<u32, Type>,
    out: &mut HashMap<String, HashMap<u32, Type>>,
) {
    lumia_core::for_each_nested_block(value, &mut |b| {
        walk_block(
            b,
            current_fun,
            local_tys,
            slot_tys,
            funref_locals,
            fun_ret_tys,
            fun_param_tys,
            fun_param0_identity,
            local_int_consts,
            sum_max_arity,
            channel_elem_hint,
            channel_elem_by_local,
            out,
        );
    });
}
