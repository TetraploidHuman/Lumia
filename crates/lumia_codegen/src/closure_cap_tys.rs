//! Pre-collect capture types at `AllocClosure` sites so callees can type
//! `ClosureCap` even when the callee body is emitted before its callers.
#![allow(clippy::too_many_arguments)]

use lumia_core::{
    infer_value_ty_ctx, Block, CodegenTypeTables, CoreModule, FunRefAliases, InferValueCtx, Op,
    Value,
};
use lumia_hir::Sym;
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::sync::Arc;

/// `(lifted_fun → capture_index → ty)` from every `AllocClosure` in `core`.
pub(crate) fn collect_closure_cap_tys(core: &CoreModule) -> HashMap<Sym, HashMap<u32, Type>> {
    let tables = lumia_core::ModuleTables::from_module(core);
    let fun_ret_tys = &tables.fun_ret_tys;
    let fun_param_tys = &tables.fun_param_tys;
    let fun_param0_identity = &tables.fun_param0_identity;
    let mut out: HashMap<Sym, HashMap<u32, Type>> = HashMap::default();
    // Change-flag fixpoint (capped): outer AllocClosure may depend on inner
    // ClosureCap typing from a prior round.
    for _ in 0..lumia_abi::CLOSURE_CAP_TY_ROUNDS {
        let before = out.clone();
        for fun in &core.functions {
            let mut local_tys: HashMap<u32, Type> = HashMap::default();
            let mut slot_tys: HashMap<Sym, Type> = HashMap::default();
            let mut funref = FunRefAliases::default();
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
                &mut funref,
                fun_ret_tys,
                fun_param_tys,
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

fn walk_block(
    block: &Block,
    current_fun: &Sym,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<Sym, Type>,
    funref: &mut FunRefAliases,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
    fun_param0_identity: &HashSet<Sym>,
    local_int_consts: &HashMap<u32, i64>,
    sum_max_arity: &HashMap<Sym, usize>,
    channel_elem_hint: Option<&Type>,
    channel_elem_by_local: &HashMap<u32, Type>,
    out: &mut HashMap<Sym, HashMap<u32, Type>>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                walk_value_nested(
                    value,
                    current_fun,
                    local_tys,
                    slot_tys,
                    funref,
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
                            funref_locals: &funref.locals,
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
                        ty = Type::Channel(Arc::new(elem.clone()));
                    }
                }
                // `ClosureCap`: typed table from AllocClosure sites.
                if let Value::ClosureCap { index, .. } = value {
                    if let Some(t) = out.get(current_fun).and_then(|m| m.get(index)) {
                        ty = t.clone();
                    }
                }
                if let Value::AllocClosure { fun, captures } = value {
                    let entry = out.entry(fun.name.clone()).or_default();
                    for (i, e) in captures.iter().enumerate() {
                        if let Some(t) = local_tys.get(&e.0).cloned() {
                            let slot = entry.entry(i as u32).or_insert_with(|| t.clone());
                            *slot = lumia_core::prefer_concrete_heap_ty(slot.clone(), t);
                        }
                    }
                }
                local_tys.insert(local.0, ty);
                crate::funref::note_funref_let(
                    funref,
                    local.0,
                    value,
                    crate::funref::AllocClosureFunref::Track,
                );
            }
            Op::Assign { name, value } => {
                if let Some(ty) = local_tys.get(&value.0).cloned() {
                    slot_tys.insert(name.clone(), ty);
                }
                crate::funref::note_funref_assign(funref, name, *value);
            }
            Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
}

fn walk_value_nested(
    value: &Value,
    current_fun: &Sym,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<Sym, Type>,
    funref: &mut FunRefAliases,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
    fun_param0_identity: &HashSet<Sym>,
    local_int_consts: &HashMap<u32, i64>,
    sum_max_arity: &HashMap<Sym, usize>,
    channel_elem_hint: Option<&Type>,
    channel_elem_by_local: &HashMap<u32, Type>,
    out: &mut HashMap<Sym, HashMap<u32, Type>>,
) {
    lumia_core::for_each_nested_block(value, &mut |b| {
        walk_block(
            b,
            current_fun,
            local_tys,
            slot_tys,
            funref,
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
