use super::fun_index::FunIndex;
use crate::ir::{Block, CoreFun, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::{BinOp, UnOp};
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
                Type::List(_) | Type::Map(_, _) | Type::Set(_) | Type::Task(_) | Type::Channel(_) => {
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
        Type::Task(e) => match inferred {
            Type::Task(_) => inferred.clone(),
            Type::Float | Type::Bool | Type::Int | Type::String | Type::Char
                if matches!(e.as_ref(), Type::Var(_)) =>
            {
                Type::Task(Box::new(inferred.clone()))
            }
            _ => orig.clone(),
        },
        Type::Channel(e) => match inferred {
            Type::Channel(_) => inferred.clone(),
            Type::Float | Type::Bool | Type::Int | Type::String | Type::Char
                if matches!(e.as_ref(), Type::Var(_)) =>
            {
                Type::Channel(Box::new(inferred.clone()))
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
    let empty = HashMap::default();
    let index = FunIndex::new(functions, &empty, trait_methods, None);
    let Local(r) = block.result?;
    let mut seen = HashSet::default();
    let mut expanding = HashSet::default();
    local_fixed_ty(
        block,
        r,
        &index,
        trait_methods,
        param_tys,
        &mut seen,
        &mut expanding,
    )
}

fn local_fixed_ty(
    block: &Block,
    id: u32,
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
) -> Option<Type> {
    // `seen` is an in-progress stack (cycle guard), not a permanent memo.
    // Shared locals (e.g. one `n` used by meanFood/meanThreat/meanDisp) must
    // be re-typed on sibling field walks — a sticky set typed only the first
    // float field and left the rest as Int (println bit-patterns).
    if !seen.insert(id) {
        return None;
    }
    let result = if let Some(t) = param_tys.get(&id) {
        Some(t.clone())
    } else {
        let mut found = None;
        for op in &block.ops {
            if let Op::Let { local, value, .. } = op {
                if local.0 == id {
                    found = value_fixed_ty(
                        block,
                        value,
                        index,
                        trait_methods,
                        param_tys,
                        seen,
                        expanding,
                    );
                    break;
                }
            }
        }
        found
    };
    seen.remove(&id);
    result
}

fn ret_ty_needs_call_site_fix(ret: &Type) -> bool {
    match ret {
        Type::Int | Type::Var(_) => true,
        Type::List(e) | Type::Set(e) | Type::Task(e) | Type::Channel(e) => {
            matches!(e.as_ref(), Type::Int | Type::Var(_))
        }
        Type::Map(k, v) => {
            matches!(k.as_ref(), Type::Int | Type::Var(_))
                || matches!(v.as_ref(), Type::Int | Type::Var(_))
        }
        Type::Adt { params, .. } => params
            .iter()
            .any(|p| matches!(p, Type::Int | Type::Var(_))),
        _ => false,
    }
}

fn value_fixed_ty(
    block: &Block,
    value: &Value,
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
) -> Option<Type> {
    match value {
        Value::Local(Local(id)) => {
            local_fixed_ty(block, *id, index, trait_methods, param_tys, seen, expanding)
        }
        Value::Name(name) => {
            slot_fixed_ty(block, name, index, trait_methods, param_tys, seen, expanding)
        }
        Value::Builtin {
            name: Builtin::Show,
            ..
        } => Some(Type::String),
        Value::Builtin {
            name: Builtin::ListGet,
            args,
        } => {
            let list_ty = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            match list_ty {
                Type::List(e) | Type::Set(e) => Some(*e),
                Type::Map(_, v) => Some(Type::Adt {
                    name: "Option".into(),
                    params: vec![*v],
                }),
                other => Some(other),
            }
        }
        Value::Builtin {
            name: Builtin::AdtField,
            args,
        } => adt_field_fixed_ty(block, args, index, trait_methods, param_tys, seen, expanding),
        Value::String(_) => Some(Type::String),
        Value::Bool(_) => Some(Type::Bool),
        Value::Int(_) => Some(Type::Int),
        Value::Float(_) => Some(Type::Float),
        Value::Char(_) => Some(Type::Char),
        Value::Binary { op, left, right } => binary_fixed_ty(
            block,
            *op,
            left.0,
            right.0,
            index,
            trait_methods,
            param_tys,
            seen,
            expanding,
        ),
        Value::Unary { op, operand } => match op {
            UnOp::Not => Some(Type::Bool),
            UnOp::Neg => {
                local_fixed_ty(block, operand.0, index, trait_methods, param_tys, seen, expanding)
            }
        },
        Value::Call { fun, args } => {
            let Some(f) = index.get(fun) else {
                // Unresolved short trait method: do **not** sample an arbitrary
                // mangled impl (Float vs Int / heap vs scalar can disagree). Leave
                // open until `resolve_trait_method_calls` rewrites the Call.
                let _ = trait_methods;
                return None;
            };
            // ABI-erased / open ret (`id`, poly wrappers): walk the callee body
            // with call-site arg types so `touch`→`id(b)` still yields `Box`,
            // not Int (else later `addx` misses `$Box_*` clones).
            if ret_ty_needs_call_site_fix(&f.ret_ty) {
                // Self-/mutual recursion: entering the callee body re-hits this Call.
                if !expanding.insert(fun.clone()) {
                    return Some(f.ret_ty.clone());
                }
                let mut call_params: HashMap<u32, Type> = HashMap::default();
                for (i, p) in f.params.iter().enumerate() {
                    let ty = args
                        .get(i)
                        .and_then(|a| {
                            local_fixed_ty(
                                block, a.0, index, trait_methods, param_tys, seen, expanding,
                            )
                        })
                        .or_else(|| f.param_tys.get(i).cloned())
                        .unwrap_or(Type::Int);
                    call_params.insert(p.0, ty);
                }
                let refined = block_result_fixed_ty_indexed(
                    &f.body,
                    index,
                    trait_methods,
                    &call_params,
                    expanding,
                );
                expanding.remove(fun);
                if let Some(t) = refined {
                    return Some(t);
                }
                for a in args {
                    if let Some(t) = local_fixed_ty(
                        block, a.0, index, trait_methods, param_tys, seen, expanding,
                    ) {
                        if !matches!(t, Type::Int | Type::Var(_)) {
                            return Some(t);
                        }
                    }
                }
            }
            Some(f.ret_ty.clone())
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
                    local_fixed_ty(block, *id, index, trait_methods, param_tys, seen, expanding)
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
                let mut params = field_tys;
                if let Some(max) = index.sum_max_arity.get(adt_name).copied() {
                    while params.len() < max {
                        params.push(Type::Int);
                    }
                }
                params
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
            let t = block_result_fixed_ty_indexed(
                then_block, index, trait_methods, param_tys, expanding,
            )?;
            let e = block_result_fixed_ty_indexed(
                else_block, index, trait_methods, param_tys, expanding,
            )?;
            join_fixed_ty(&t, &e)
        }
        _ => None,
    }
}

/// Mutable/immutable slot load: type from any Let/Assign into `name`.
/// Numeric slots prefer Float; a heap/container type must not be overwritten
/// by Float (that put live pointers in XMM regs → NaN-canon UAF).
fn slot_fixed_ty(
    block: &Block,
    name: &str,
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
) -> Option<Type> {
    let mut found: Option<Type> = None;
    scan_slot_ty(
        block,
        name,
        index,
        trait_methods,
        param_tys,
        seen,
        expanding,
        &mut found,
    );
    found
}

fn scan_slot_ty(
    block: &Block,
    name: &str,
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
    found: &mut Option<Type>,
) {
    for op in &block.ops {
        match op {
            Op::Assign {
                name: n,
                value: Local(id),
            } if n == name => {
                if let Some(t) =
                    local_fixed_ty(block, *id, index, trait_methods, param_tys, seen, expanding)
                {
                    *found = Some(merge_slot_ty(found.take(), t));
                }
            }
            Op::Let { value, .. } => {
                scan_value_slots(
                    value, name, index, trait_methods, param_tys, seen, expanding, found,
                );
            }
            _ => {}
        }
    }
}

fn scan_value_slots(
    value: &Value,
    name: &str,
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
    found: &mut Option<Type>,
) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            scan_slot_ty(
                then_block,
                name,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
                found,
            );
            scan_slot_ty(
                else_block,
                name,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
                found,
            );
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            scan_slot_ty(
                header, name, index, trait_methods, param_tys, seen, expanding, found,
            );
            scan_slot_ty(
                body, name, index, trait_methods, param_tys, seen, expanding, found,
            );
            scan_slot_ty(
                latch, name, index, trait_methods, param_tys, seen, expanding, found,
            );
        }
        _ => {}
    }
}

fn is_ref_ty(t: &Type) -> bool {
    matches!(
        t,
        Type::List(_)
            | Type::Map(_, _)
            | Type::Set(_)
            | Type::Task(_)
            | Type::Channel(_)
            | Type::Adt { .. }
            | Type::String
            | Type::Fun(_, _, _)
            | Type::Tuple(_)
            | Type::TuplePrefix(_)
    )
}

fn merge_slot_ty(prev: Option<Type>, next: Type) -> Type {
    match (prev, next) {
        (None, t) => t,
        (Some(p), n) if p == n => p,
        // Pointer-carrying slots win over unboxed numeric — never store a
        // List/ADT pointer as Float (XMM NaN canonicalization / missed GC root).
        (Some(p), n) if is_ref_ty(&p) && !is_ref_ty(&n) => p,
        (Some(p), n) if !is_ref_ty(&p) && is_ref_ty(&n) => n,
        (Some(Type::Float), _) | (_, Type::Float) => Type::Float,
        (Some(p), _) => p,
    }
}

fn binary_fixed_ty(
    block: &Block,
    op: BinOp,
    left: u32,
    right: u32,
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
) -> Option<Type> {
    match op {
        BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or => Some(Type::Bool),
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            let l =
                local_fixed_ty(block, left, index, trait_methods, param_tys, seen, expanding)?;
            let r =
                local_fixed_ty(block, right, index, trait_methods, param_tys, seen, expanding)?;
            match (&l, &r) {
                (Type::Float, _) | (_, Type::Float) => Some(Type::Float),
                (Type::Int, Type::Int) => Some(Type::Int),
                _ => Some(l),
            }
        }
    }
}

fn adt_field_fixed_ty(
    block: &Block,
    args: &[Local],
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
) -> Option<Type> {
    let recv = args.first()?;
    let idx_local = args.get(1)?;
    let recv_ty =
        local_fixed_ty(block, recv.0, index, trait_methods, param_tys, seen, expanding)?;
    let idx = int_const_in_block(block, idx_local.0)?;
    if idx < 0 {
        return None;
    }
    match recv_ty {
        Type::Adt { params, .. } | Type::Tuple(params) | Type::TuplePrefix(params) => {
            params.get(idx as usize).cloned()
        }
        _ => None,
    }
}

fn int_const_in_block(block: &Block, id: u32) -> Option<i64> {
    for op in &block.ops {
        if let Op::Let {
            local,
            value: Value::Int(n),
            ..
        } = op
        {
            if local.0 == id {
                return Some(*n);
            }
        }
        if let Op::Let {
            local,
            value: Value::Local(Local(src)),
            ..
        } = op
        {
            if local.0 == id {
                return int_const_in_block(block, *src);
            }
        }
    }
    None
}

fn block_result_fixed_ty_indexed(
    block: &Block,
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
    expanding: &mut HashSet<String>,
) -> Option<Type> {
    let Local(r) = block.result?;
    let mut seen = HashSet::default();
    local_fixed_ty(block, r, index, trait_methods, param_tys, &mut seen, expanding)
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
                // User sums / products: pad to max width (Circle vs Rect).
                let merge = |x: &Type, y: &Type| -> Type {
                    match (x, y) {
                        (Type::Int, other) | (Type::Var(_), other) => other.clone(),
                        (other, Type::Int) | (other, Type::Var(_)) => other.clone(),
                        (l, r) if l == r => l.clone(),
                        (l, _) => l.clone(),
                    }
                };
                let n = p1.len().max(p2.len());
                let mut params = Vec::with_capacity(n);
                for i in 0..n {
                    params.push(merge(
                        p1.get(i).unwrap_or(&Type::Int),
                        p2.get(i).unwrap_or(&Type::Int),
                    ));
                }
                Some(Type::Adt {
                    name: n1.clone(),
                    params,
                })
            }
        }
        _ => None,
    }
}
