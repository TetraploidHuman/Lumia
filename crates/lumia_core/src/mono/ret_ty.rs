use crate::ir::{Block, CoreFun, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::BinOp;
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(crate) fn param_ty_map(fun: &CoreFun) -> HashMap<u32, Type> {
    fun.params
        .iter()
        .zip(fun.param_tys.iter())
        .map(|(p, t)| (p.0, t.clone()))
        .collect()
}

/// Keep ADT/List/Map/Set shape; refine only `Var` slots (never blast Int
/// placeholders — those are often literal field types like `Ok(7)`).
pub(crate) fn refine_mono_container_ret(orig: &Type, inferred: &Type) -> Type {
    match orig {
        Type::Adt { name, params } => {
            let mut ps = params.clone();
            match inferred {
                Type::Adt {
                    name: iname,
                    params: ips,
                } if iname == name => {
                    for (p, ip) in ps.iter_mut().zip(ips.iter()) {
                        if matches!(p, Type::Var(_)) && !matches!(ip, Type::Var(_)) {
                            *p = ip.clone();
                        }
                    }
                }
                Type::List(_) | Type::Map(_, _) | Type::Set(_) => {
                    if let Some(p) = ps.first_mut() {
                        if matches!(p, Type::Var(_)) {
                            *p = inferred.clone();
                        }
                    }
                }
                _ => {}
            }
            Type::Adt {
                name: name.clone(),
                params: ps,
            }
        }
        Type::List(e) => match inferred {
            Type::List(_) => inferred.clone(),
            Type::Float | Type::Bool | Type::Int | Type::String | Type::Char
                if matches!(e.as_ref(), Type::Var(_)) =>
            {
                Type::List(Box::new(inferred.clone()))
            }
            _ => orig.clone(),
        },
        Type::Set(e) => match inferred {
            Type::Set(_) => inferred.clone(),
            Type::Float | Type::Bool | Type::Int | Type::String | Type::Char
                if matches!(e.as_ref(), Type::Var(_)) =>
            {
                Type::Set(Box::new(inferred.clone()))
            }
            _ => orig.clone(),
        },
        Type::Map(k, v) => match inferred {
            Type::Map(_, _) => inferred.clone(),
            Type::Float | Type::Bool | Type::Int | Type::String | Type::Char
                if matches!(k.as_ref(), Type::Var(_)) =>
            {
                Type::Map(Box::new(inferred.clone()), v.clone())
            }
            _ => orig.clone(),
        },
        Type::Tuple(ts) => match inferred {
            Type::Tuple(its) if its.len() == ts.len() => inferred.clone(),
            Type::TuplePrefix(its) if its.len() <= ts.len() => {
                // Prefix refinement is weaker than a fixed tuple; keep orig.
                let _ = its;
                orig.clone()
            }
            _ => orig.clone(),
        },
        Type::TuplePrefix(ts) => match inferred {
            Type::Tuple(its) if its.len() >= ts.len() => inferred.clone(),
            Type::TuplePrefix(its) if its.len() >= ts.len() => inferred.clone(),
            _ => orig.clone(),
        },
        other => other.clone(),
    }
}

pub(crate) fn block_result_fixed_ty(
    block: &Block,
    functions: &[CoreFun],
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
) -> Option<Type> {
    let Local(r) = block.result?;
    let mut seen = HashSet::default();
    local_fixed_ty(block, r, functions, trait_methods, param_tys, &mut seen)
}

fn local_fixed_ty(
    block: &Block,
    id: u32,
    functions: &[CoreFun],
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    if let Some(t) = param_tys.get(&id) {
        return Some(t.clone());
    }
    for op in &block.ops {
        if let Op::Let { local, value, .. } = op {
            if local.0 == id {
                return value_fixed_ty(block, value, functions, trait_methods, param_tys, seen);
            }
        }
    }
    None
}

fn value_fixed_ty(
    block: &Block,
    value: &Value,
    functions: &[CoreFun],
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    match value {
        Value::Local(Local(id)) => {
            local_fixed_ty(block, *id, functions, trait_methods, param_tys, seen)
        }
        Value::Builtin {
            name: Builtin::Show,
            ..
        } => Some(Type::String),
        Value::String(_) => Some(Type::String),
        Value::Bool(_) => Some(Type::Bool),
        Value::Int(_) => Some(Type::Int),
        Value::Float(_) => Some(Type::Float),
        Value::Char(_) => Some(Type::Char),
        Value::Call { fun, .. } => {
            if let Some(f) = functions.iter().find(|f| f.name == *fun) {
                return Some(f.ret_ty.clone());
            }
            // Unresolved short trait method — sample any mangled impl's ret_ty.
            let sample = trait_methods
                .iter()
                .find(|((_, m), _)| m == fun)
                .and_then(|(_, mangled)| mangled.first())
                .and_then(|m| functions.iter().find(|f| f.name == *m));
            sample.map(|f| f.ret_ty.clone())
        }
        Value::AllocAdt {
            adt_name,
            tag,
            fields,
            ..
        } => {
            let field_tys: Vec<Type> = fields
                .iter()
                .map(|Local(id)| {
                    local_fixed_ty(block, *id, functions, trait_methods, param_tys, seen)
                        .unwrap_or(Type::Int)
                })
                .collect();
            // Result[T,E]: Ok → params[0]=T; Err → params[1]=E (other slot Int placeholder).
            // Option: None → [Int] placeholder so join with Some(T) yields Option[T].
            let params = if adt_name == "Result" {
                let payload = field_tys.first().cloned().unwrap_or(Type::Int);
                if *tag == 0 {
                    vec![payload, Type::Int]
                } else {
                    vec![Type::Int, payload]
                }
            } else if adt_name == "Option" && field_tys.is_empty() {
                vec![Type::Int]
            } else {
                field_tys
            };
            Some(Type::Adt {
                name: adt_name.clone(),
                params,
            })
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            let t = block_result_fixed_ty(then_block, functions, trait_methods, param_tys)?;
            let e = block_result_fixed_ty(else_block, functions, trait_methods, param_tys)?;
            join_fixed_ty(&t, &e)
        }
        Value::Binary {
            op:
                BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or,
            ..
        } => Some(Type::Bool),
        _ => None,
    }
}

fn join_fixed_ty(a: &Type, b: &Type) -> Option<Type> {
    if a == b {
        return Some(a.clone());
    }
    match (a, b) {
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
            if n1 == "Result" {
                let merge = |x: &Type, y: &Type| -> Type {
                    match (x, y) {
                        (Type::Int, other) | (Type::Var(_), other) => other.clone(),
                        (other, Type::Int) | (other, Type::Var(_)) => other.clone(),
                        (l, r) if l == r => l.clone(),
                        (l, _) => l.clone(),
                    }
                };
                let t = merge(
                    p1.first().unwrap_or(&Type::Int),
                    p2.first().unwrap_or(&Type::Int),
                );
                let e = merge(
                    p1.get(1).unwrap_or(&Type::Int),
                    p2.get(1).unwrap_or(&Type::Int),
                );
                Some(Type::Adt {
                    name: "Result".into(),
                    params: vec![t, e],
                })
            } else if n1 == "Option" {
                let merge = |x: &Type, y: &Type| -> Type {
                    match (x, y) {
                        (Type::Int, other) | (Type::Var(_), other) => other.clone(),
                        (other, Type::Int) | (other, Type::Var(_)) => other.clone(),
                        (l, r) if l == r => l.clone(),
                        (l, _) => l.clone(),
                    }
                };
                let p = merge(
                    p1.first().unwrap_or(&Type::Int),
                    p2.first().unwrap_or(&Type::Int),
                );
                Some(Type::Adt {
                    name: "Option".into(),
                    params: vec![p],
                })
            } else {
                None
            }
        }
        _ => None,
    }
}
