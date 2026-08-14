use super::fun_index::FunIndex;
use super::specialize::mono_value_ty;
use crate::ir::{Block, CoreFun, CoreModule, Local, Op, Value};
use lumia_hir::Builtin;
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
        params: Vec::new(),
        ops: Vec::new(),
        result: None,
    };
    let mut bodies: Vec<Block> = functions
        .iter_mut()
        .map(|f| std::mem::replace(&mut f.body, empty.clone()))
        .collect();
    {
        let index = FunIndex::new(&functions, &module.sum_max_arity);
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
            Op::Effect { value } => {
                resolve_trait_value(
                    value,
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
    let index = FunIndex::new(&module.functions, &module.sum_max_arity);
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
                params: vec![],
                ops: vec![Op::Let {
                    local: fail_local,
                    value: Value::Builtin {
                        name: Builtin::MatchFail,
                        args: vec![],
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
            escaping: Default::default(),
            scheme_poly: false,
            mono_of: None,
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
            Op::Let { value, .. } | Op::Effect { value } => {
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
pub(crate) fn directize_funref_calls(module: &mut CoreModule) {
    let empty = HashMap::default();
    for fun in &mut module.functions {
        directize_block(&mut fun.body, &empty);
    }
}

pub(crate) fn directize_block(block: &mut Block, parent_funrefs: &HashMap<u32, String>) {
    directize_block_with_slots(block, parent_funrefs, &HashMap::default());
}

fn directize_block_with_slots(
    block: &mut Block,
    parent_funrefs: &HashMap<u32, String>,
    parent_slot_funrefs: &HashMap<String, String>,
) {
    // Inherit FunRef bindings from the enclosing block so `val f = g; if … { f(x) }`
    // inside nested If/Loop still becomes a direct `Call`.
    let mut funref_of = parent_funrefs.clone();
    let mut slot_funrefs = parent_slot_funrefs.clone();
    for op in &mut block.ops {
        match op {
            Op::Let { local, value, .. } => {
                directize_value(value, &funref_of);
                walk_nested_blocks_directize(value, &funref_of, &slot_funrefs);
                if let Value::FunRef(name) = value {
                    funref_of.insert(local.0, name.clone());
                } else if let Value::Local(Local(src)) = value {
                    if let Some(n) = funref_of.get(src).cloned() {
                        funref_of.insert(local.0, n);
                    } else {
                        funref_of.remove(&local.0);
                    }
                } else if let Value::Name(n) = value {
                    if let Some(fr) = slot_funrefs.get(n).cloned() {
                        funref_of.insert(local.0, fr);
                    } else {
                        funref_of.remove(&local.0);
                    }
                } else {
                    funref_of.remove(&local.0);
                }
            }
            Op::Effect { value } => {
                directize_value(value, &funref_of);
                walk_nested_blocks_directize(value, &funref_of, &slot_funrefs);
            }
            Op::Assign { name, value } => {
                if let Some(fr) = funref_of.get(&value.0).cloned() {
                    slot_funrefs.insert(name.clone(), fr);
                } else {
                    slot_funrefs.remove(name);
                }
            }
            Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
}

fn walk_nested_blocks_directize(
    value: &mut Value,
    funref_of: &HashMap<u32, String>,
    slot_funrefs: &HashMap<String, String>,
) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            directize_block_with_slots(then_block, funref_of, slot_funrefs);
            directize_block_with_slots(else_block, funref_of, slot_funrefs);
        }
        Value::Loop {
            header,
            body,
            latch,
            ..
        } => {
            directize_block_with_slots(header, funref_of, slot_funrefs);
            directize_block_with_slots(body, funref_of, slot_funrefs);
            directize_block_with_slots(latch, funref_of, slot_funrefs);
        }
        // Fresh scope: lifted lambda body should not see outer SSA FunRef locals.
        Value::Lambda { body, .. } => {
            directize_block_with_slots(body, &HashMap::default(), &HashMap::default())
        }
        _ => {}
    }
}

fn directize_value(value: &mut Value, funref_of: &HashMap<u32, String>) {
    let Value::IndirectCall { callee, args } = value else {
        return;
    };
    let Some(name) = funref_of.get(&callee.0) else {
        return;
    };
    let args = std::mem::take(args);
    *value = Value::Call {
        fun: name.clone(),
        args,
    };
}
