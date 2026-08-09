use super::specialize::mono_value_ty;
use crate::ir::{Block, CoreFun, CoreModule, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_ty::{Effect, Type};
use std::collections::{HashMap, HashSet};

pub(crate) fn resolve_trait_method_calls(module: &mut CoreModule) {
    if module.trait_methods.is_empty() {
        return;
    }
    let trait_methods = module.trait_methods.clone();
    let method_names: HashSet<String> = trait_methods.keys().map(|(_, m)| m.clone()).collect();
    // Snapshot signatures for `mono_value_ty` (names/ret only; bodies unused).
    let fun_sigs: Vec<CoreFun> = module.functions.clone();
    for fun in &mut module.functions {
        let mut local_tys: HashMap<u32, Type> = HashMap::new();
        for (i, p) in fun.params.iter().enumerate() {
            local_tys.insert(p.0, fun.param_tys.get(i).cloned().unwrap_or(Type::Int));
        }
        resolve_trait_block(
            &mut fun.body,
            &mut local_tys,
            &trait_methods,
            &method_names,
            &fun_sigs,
        );
    }
}

fn resolve_trait_block(
    block: &mut Block,
    local_tys: &mut HashMap<u32, Type>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    method_names: &HashSet<String>,
    functions: &[CoreFun],
) {
    for op in &mut block.ops {
        match op {
            Op::Let { local, value, .. } => {
                resolve_trait_value(value, local_tys, trait_methods, method_names, functions);
                let ty = mono_value_ty(value, local_tys, functions);
                local_tys.insert(local.0, ty);
            }
            Op::Effect { value } => {
                resolve_trait_value(value, local_tys, trait_methods, method_names, functions);
            }
            _ => {}
        }
    }
}

fn resolve_trait_value(
    value: &mut Value,
    local_tys: &mut HashMap<u32, Type>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    method_names: &HashSet<String>,
    functions: &[CoreFun],
) {
    match value {
        Value::Call { fun, args } => {
            if method_names.contains(fun.as_str()) {
                if let Some(recv) = args.first() {
                    if let Some(Type::Adt { name, .. }) = local_tys.get(&recv.0).cloned() {
                        if let Some(cands) = trait_methods.get(&(name, fun.clone())) {
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
                trait_methods,
                method_names,
                functions,
            );
            resolve_trait_block(
                else_block,
                local_tys,
                trait_methods,
                method_names,
                functions,
            );
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            resolve_trait_block(header, local_tys, trait_methods, method_names, functions);
            resolve_trait_block(body, local_tys, trait_methods, method_names, functions);
            resolve_trait_block(latch, local_tys, trait_methods, method_names, functions);
        }
        _ => {}
    }
}

/// Generic poly bodies may still mention short method names; emit trap stubs so
/// codegen can link (specialized clones call mangled impls).
pub(crate) fn ensure_trait_method_stubs(module: &mut CoreModule) {
    let method_names: HashSet<String> = module
        .trait_methods
        .keys()
        .map(|(_, m)| m.clone())
        .collect();
    if method_names.is_empty() {
        return;
    }
    let mut referenced: HashSet<String> = HashSet::new();
    for fun in &module.functions {
        collect_trait_method_refs(&fun.body, &method_names, &mut referenced);
    }
    for name in referenced {
        if module.functions.iter().any(|f| f.name == name) {
            continue;
        }
        // Sample arity / ret from any mangled impl.
        let sample = module
            .trait_methods
            .iter()
            .find(|((_, m), _)| *m == name)
            .and_then(|(_, mangled)| mangled.first())
            .and_then(|m| module.functions.iter().find(|f| f.name == *m));
        let (nparams, ret_ty) = match sample {
            Some(f) => (f.params.len().max(1), f.ret_ty.clone()),
            None => (1, Type::Int),
        };
        let params: Vec<Local> = (0..nparams as u32).map(Local).collect();
        let param_names: Vec<String> = (0..nparams).map(|i| format!("p{i}")).collect();
        let param_tys = vec![Type::Int; nparams];
        let fail_local = Local(nparams as u32);
        module.functions.push(CoreFun {
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
            escaping: HashSet::new(),
            scheme_poly: false,
        });
    }
}

fn collect_trait_method_refs(block: &Block, methods: &HashSet<String>, out: &mut HashSet<String>) {
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
    methods: &HashSet<String>,
    out: &mut HashSet<String>,
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
    let empty = HashMap::new();
    for fun in &mut module.functions {
        directize_block(&mut fun.body, &empty);
    }
}

pub(crate) fn directize_block(block: &mut Block, parent_funrefs: &HashMap<u32, String>) {
    // Inherit FunRef bindings from the enclosing block so `val f = g; if … { f(x) }`
    // inside nested If/Loop still becomes a direct `Call`.
    let mut funref_of = parent_funrefs.clone();
    for op in &mut block.ops {
        match op {
            Op::Let { local, value, .. } => {
                directize_value(value, &funref_of);
                walk_nested_blocks_directize(value, &funref_of);
                if let Value::FunRef(name) = value {
                    funref_of.insert(local.0, name.clone());
                } else if let Value::Local(Local(src)) = value {
                    if let Some(n) = funref_of.get(src).cloned() {
                        funref_of.insert(local.0, n);
                    } else {
                        funref_of.remove(&local.0);
                    }
                } else {
                    funref_of.remove(&local.0);
                }
            }
            Op::Effect { value } => {
                directize_value(value, &funref_of);
                walk_nested_blocks_directize(value, &funref_of);
            }
            Op::Assign { .. } | Op::Break | Op::Continue => {}
        }
    }
}

fn walk_nested_blocks_directize(value: &mut Value, funref_of: &HashMap<u32, String>) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            directize_block(then_block, funref_of);
            directize_block(else_block, funref_of);
        }
        Value::Loop {
            header,
            body,
            latch,
            ..
        } => {
            directize_block(header, funref_of);
            directize_block(body, funref_of);
            directize_block(latch, funref_of);
        }
        // Fresh scope: lifted lambda body should not see outer SSA FunRef locals.
        Value::Lambda { body, .. } => directize_block(body, &HashMap::new()),
        _ => {}
    }
}

fn directize_value(value: &mut Value, funref_of: &HashMap<u32, String>) {
    if let Value::IndirectCall { callee, args } = value {
        if let Some(name) = funref_of.get(&callee.0) {
            *value = Value::Call {
                fun: name.clone(),
                args: args.clone(),
            };
        }
    }
}
