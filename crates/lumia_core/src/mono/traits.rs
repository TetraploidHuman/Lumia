use super::fun_index::FunIndex;
use super::specialize::mono_value_ty;
use crate::ir::{Block, CoreFun, CoreModule, ForeignAbi, FunKind, Local, Op, Value};
use crate::visit::for_each_top_level_op_in_block_mut;
use crate::CoreBinOp as BinOp;
use lumia_hir::Builtin;
use lumia_syntax::Sym;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet};

pub(crate) fn resolve_trait_method_calls(module: &mut CoreModule) {
    if module.trait_methods.is_empty() {
        return;
    }
    let tables = crate::ModuleTables::from_module(module);
    let trait_methods = tables.trait_methods;
    let method_names: FxHashSet<String> = trait_methods.keys().map(|(_, m)| m.clone()).collect();
    let shadow = super::fun_index::SigShadow::from_module(module);
    let index = shadow.index();
    for fun in &mut module.functions {
        let mut local_tys: HashMap<u32, Type> = HashMap::default();
        for (j, p) in fun.params.iter().enumerate() {
            local_tys.insert(p.0, fun.param_tys.get(j).cloned().unwrap_or(Type::Int));
        }
        let mut slot_tys: HashMap<Sym, Type> = HashMap::default();
        let mut int_consts: HashMap<u32, i64> = HashMap::default();
        resolve_trait_block(
            &mut fun.body,
            &mut local_tys,
            &mut slot_tys,
            &mut int_consts,
            &trait_methods,
            &method_names,
            &index,
        );
    }
}

fn resolve_trait_block(
    block: &mut Block,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<Sym, Type>,
    int_consts: &mut HashMap<u32, i64>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    method_names: &FxHashSet<String>,
    index: &FunIndex<'_>,
) {
    for_each_top_level_op_in_block_mut(block, &mut |op| match op {
        Op::Let { local, value, .. } => {
            resolve_trait_value(
                value,
                local_tys,
                slot_tys,
                int_consts,
                trait_methods,
                method_names,
                index,
            );
            let ty = mono_value_ty(value, local_tys, slot_tys, int_consts, index);
            local_tys.insert(local.0, ty);
            if let Value::Int(n) = value {
                int_consts.insert(local.0, *n);
            } else {
                int_consts.remove(&local.0);
            }
        }
        Op::Assign { name, value } => {
            if let Some(ty) = local_tys.get(&value.0).cloned() {
                slot_tys.insert(name.clone(), ty);
            }
        }
        _ => {}
    });
}

fn resolve_trait_value(
    value: &mut Value,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<Sym, Type>,
    int_consts: &mut HashMap<u32, i64>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    method_names: &FxHashSet<String>,
    index: &FunIndex<'_>,
) {
    match value {
        Value::Call { fun, args } => {
            if method_names.contains(fun.as_str()) {
                if let Some(recv) = args.first() {
                    if let Some(Type::Adt { name, .. }) = local_tys.get(&recv.0) {
                        if let Some(cands) = trait_methods.get(&(name.clone(), fun.name.to_string())) {
                            if let [mangled] = cands.as_slice() {
                                *fun = mangled.clone().into();
                            }
                        }
                    }
                }
            }
        }
        // `a + b` / `a * b` on ADTs with `instance Num` stay as Binary through
        // lower; rewrite to `__Num_T_add`/`mul` Call so mono can specialize
        // Float fields (codegen override alone hits the unspecialized body).
        Value::Binary { op, left, right } if matches!(op, BinOp::Add | BinOp::Mul) => {
            let method = if matches!(op, BinOp::Add) {
                "add"
            } else {
                "mul"
            };
            if let (Some(Type::Adt { name: n1, .. }), Some(Type::Adt { name: n2, .. })) =
                (local_tys.get(&left.0), local_tys.get(&right.0))
            {
                if n1 == n2 {
                    if let Some(cands) = trait_methods.get(&(n1.clone(), method.to_string())) {
                        if let [mangled] = cands.as_slice() {
                            *value = Value::Call {
                                fun: mangled.clone().into(),
                                args: vec![*left, *right],
                            };
                        }
                    }
                }
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            resolve_trait_block(
                then_block,
                local_tys,
                slot_tys,
                int_consts,
                trait_methods,
                method_names,
                index,
            );
            resolve_trait_block(
                else_block,
                local_tys,
                slot_tys,
                int_consts,
                trait_methods,
                method_names,
                index,
            );
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            resolve_trait_block(
                header,
                local_tys,
                slot_tys,
                int_consts,
                trait_methods,
                method_names,
                index,
            );
            resolve_trait_block(
                body,
                local_tys,
                slot_tys,
                int_consts,
                trait_methods,
                method_names,
                index,
            );
            resolve_trait_block(
                latch,
                local_tys,
                slot_tys,
                int_consts,
                trait_methods,
                method_names,
                index,
            );
        }
        _ => {}
    }
}

/// Generic poly bodies may still mention short method names; emit trap stubs so
/// codegen can link (specialized clones call mangled impls).
pub(crate) fn ensure_trait_method_stubs(module: &mut CoreModule) {
    let method_names: FxHashSet<String> = module
        .trait_methods
        .keys()
        .map(|(_, m)| m.clone())
        .collect();
    if method_names.is_empty() {
        return;
    }
    let mut referenced: FxHashSet<String> = FxHashSet::default();
    for fun in &module.functions {
        crate::collect_call_names_in(&fun.body, &method_names, &mut referenced);
    }
    let index = FunIndex::new(
        &module.functions,
        &module.sum_max_arity,
        &module.trait_methods,
        module.channel_elem_hint.as_ref(),
    );
    let mut stubs = Vec::new();
    for name in referenced {
        if index.contains(&name) {
            continue;
        }
        // Sample arity / ret from any mangled impl.
        let sample = module
            .trait_methods
            .iter()
            .find(|((_, m), _)| *m == name)
            .and_then(|(_, mangled)| mangled.first())
            .and_then(|m| index.get(m));
        let (nparams, ret_ty) = match sample {
            Some(f) => (f.params.len().max(1), f.ret_ty.clone()),
            None => (1, Type::Int),
        };
        let params: Vec<Local> = (0..nparams as u32).map(Local).collect();
        let param_names: Vec<Sym> = (0..nparams).map(|i| Sym::from(format!("p{i}"))).collect();
        let param_tys = vec![Type::Int; nparams];
        let fail_local = Local(nparams as u32);
        stubs.push(CoreFun {
            name: name.into(),
            params,
            param_names,
            param_tys,
            body: Block {
                ops: vec![Op::Let {
                    local: fail_local,
                    value: Value::Builtin {
                        name: Builtin::MatchFail,
                        args: vec![],
                        result_ty: None,
                    },
                    pure_region: false,
                }],
                result: Some(fail_local),
            },
            ret_ty,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: ForeignAbi::C,
            escaping: Default::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        });
    }
    module.functions.append(&mut stubs);
}
