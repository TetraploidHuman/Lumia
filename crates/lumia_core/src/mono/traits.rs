use super::fun_index::FunIndex;
use super::specialize::mono_value_ty;
use crate::ir::{Block, CoreFun, CoreModule, ForeignAbi, FunKind, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::BinOp;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet};

pub(crate) fn resolve_trait_method_calls(module: &mut CoreModule) {
    if module.trait_methods.is_empty() {
        return;
    }
    let trait_methods = module.trait_methods.clone();
    let method_names: FxHashSet<String> = trait_methods.keys().map(|(_, m)| m.clone()).collect();
    // Take bodies out so FunIndex can borrow the signature table immutably.
    let mut functions = std::mem::take(&mut module.functions);
    let empty = Block {
        ops: Vec::new(),
        result: None,
    };
    let mut bodies: Vec<Block> = functions
        .iter_mut()
        .map(|f| std::mem::replace(&mut f.body, empty.clone()))
        .collect();
    {
        let index = FunIndex::new(&functions, &module.sum_max_arity, &module.trait_methods, module.channel_elem_hint.as_ref());
        for i in 0..functions.len() {
            let mut local_tys: HashMap<u32, Type> = HashMap::default();
            for (j, p) in functions[i].params.iter().enumerate() {
                local_tys.insert(
                    p.0,
                    functions[i].param_tys.get(j).cloned().unwrap_or(Type::Int),
                );
            }
            let mut slot_tys: HashMap<String, Type> = HashMap::default();
            let mut int_consts: HashMap<u32, i64> = HashMap::default();
            resolve_trait_block(
                &mut bodies[i],
                &mut local_tys,
                &mut slot_tys,
                &mut int_consts,
                &trait_methods,
                &method_names,
                &index,
            );
        }
    }
    for (fun, body) in functions.iter_mut().zip(bodies) {
        fun.body = body;
    }
    module.functions = functions;
}

fn resolve_trait_block(
    block: &mut Block,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    int_consts: &mut HashMap<u32, i64>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    method_names: &FxHashSet<String>,
    index: &FunIndex<'_>,
) {
    for op in &mut block.ops {
        match op {
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
        }
    }
}

fn resolve_trait_value(
    value: &mut Value,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
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
                        if let Some(cands) = trait_methods.get(&(name.clone(), fun.clone())) {
                            if let [mangled] = cands.as_slice() {
                                *fun = mangled.clone();
                            }
                        }
                    }
                }
            }
        }
        // `a + b` / `a * b` on ADTs with `instance Num` stay as Binary through
        // lower; rewrite to `__Num_T_add`/`mul` Call so mono can specialize
        // Float fields (codegen override alone hits the unspecialized body).
        Value::Binary { op, left, right }
            if matches!(op, BinOp::Add | BinOp::Mul) =>
        {
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
                                fun: mangled.clone(),
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
        collect_trait_method_refs(&fun.body, &method_names, &mut referenced);
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
        let param_names: Vec<String> = (0..nparams).map(|i| format!("p{i}")).collect();
        let param_tys = vec![Type::Int; nparams];
        let fail_local = Local(nparams as u32);
        stubs.push(CoreFun {
            name,
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
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        });
    }
    module.functions.append(&mut stubs);
}

fn collect_trait_method_refs(
    block: &Block,
    methods: &FxHashSet<String>,
    out: &mut FxHashSet<String>,
) {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } => {
                collect_trait_method_refs_value(value, methods, out);
            }
            _ => {}
        }
    }
}

fn collect_trait_method_refs_value(
    value: &Value,
    methods: &FxHashSet<String>,
    out: &mut FxHashSet<String>,
) {
    match value {
        Value::Call { fun, .. } if methods.contains(fun.as_str()) => {
            out.insert(fun.clone());
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            collect_trait_method_refs(then_block, methods, out);
            collect_trait_method_refs(else_block, methods, out);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            collect_trait_method_refs(header, methods, out);
            collect_trait_method_refs(body, methods, out);
            collect_trait_method_refs(latch, methods, out);
        }
        _ => {}
    }
}
