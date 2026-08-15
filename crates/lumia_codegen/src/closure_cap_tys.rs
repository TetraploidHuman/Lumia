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
    let fun_param0_identity: HashSet<String> = HashSet::default();
    let mut out: HashMap<String, HashMap<u32, Type>> = HashMap::default();
    for fun in &core.functions {
        let mut local_tys: HashMap<u32, Type> = HashMap::default();
        let mut slot_tys: HashMap<String, Type> = HashMap::default();
        let mut funref_locals: HashMap<u32, String> = HashMap::default();
        let local_int_consts: HashMap<u32, i64> = HashMap::default();
        let sum_max_arity: HashMap<String, usize> = HashMap::default();
        for (i, ty) in fun.param_tys.iter().enumerate() {
            if let Some(p) = fun.params.get(i) {
                local_tys.insert(p.0, ty.clone());
            }
        }
        walk_block(
            &fun.body,
            &mut local_tys,
            &mut slot_tys,
            &mut funref_locals,
            &fun_ret_tys,
            &fun_param_tys,
            &fun_param0_identity,
            &local_int_consts,
            &sum_max_arity,
            core.channel_elem_hint.as_ref(),
            &core.channel_elem_by_local,
            &mut out,
        );
    }
    out
}

fn walk_block(
    block: &Block,
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
                if let Value::AllocClosure { fun, captures } = value {
                    let mut cap_tys = HashMap::default();
                    for (i, e) in captures.iter().enumerate() {
                        if let Some(t) = local_tys.get(&e.0).cloned() {
                            cap_tys.insert(i as u32, t);
                        }
                    }
                    if !cap_tys.is_empty() {
                        out.insert(fun.clone(), cap_tys);
                    }
                }
                local_tys.insert(local.0, ty);
                match value {
                    Value::FunRef(name) => {
                        funref_locals.insert(local.0, name.clone());
                    }
                    Value::AllocClosure { fun, .. } => {
                        funref_locals.insert(local.0, fun.clone());
                    }
                    Value::Local(src) => {
                        if let Some(n) = funref_locals.get(&src.0).cloned() {
                            funref_locals.insert(local.0, n);
                        } else {
                            funref_locals.remove(&local.0);
                        }
                    }
                    _ => {
                        funref_locals.remove(&local.0);
                    }
                }
            }
            Op::Effect { value } => {
                walk_value_nested(
                    value,
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
