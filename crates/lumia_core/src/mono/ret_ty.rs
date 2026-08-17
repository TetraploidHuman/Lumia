use super::fun_index::FunIndex;
use crate::ir::{Block, CoreFun, Local, Op, Value};
use crate::value_ty::{
    builtin_value_ty, elems_family_recv_ok, prefer_concrete_heap_ty, via_gated_recv,
    via_gated_recv_seeded, InferValueCtx,
};
use lumia_hir::Builtin;
use crate::{CoreBinOp as BinOp, CoreUnOp as UnOp};
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
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(String, String), Vec<String>>,
    param_tys: &HashMap<u32, Type>,
) -> Option<Type> {
    let Local(r) = block.result?;
    let mut seen = HashSet::default();
    let mut expanding = HashSet::default();
    local_fixed_ty(
        block,
        r,
        index,
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
        Value::Builtin { name, args, .. }
            if matches!(
                *name,
                Builtin::Show
                    | Builtin::MatchFail
                    | Builtin::ListLen
                    | Builtin::AdtTag
                    | Builtin::Contains
                    | Builtin::StrStartsWith
                    | Builtin::StrEndsWith
                    | Builtin::Println
                    | Builtin::Assert
                    | Builtin::ChannelSend
                    | Builtin::ChannelClose
                    | Builtin::ScopeEnter
                    | Builtin::ScopeLeave
                    | Builtin::ScopeCancel
                    | Builtin::ReadStdin
                    | Builtin::StrTrim
                    | Builtin::StrSplit
                    | Builtin::StrSubstring
                    | Builtin::StrToLower
                    | Builtin::StrToUpper
                    | Builtin::ListJoin
            ) =>
        {
            // Fixed scalar/Unit/String results — share value_ty projection.
            let empty = HashMap::default();
            Some(builtin_value_ty(
                *name,
                args,
                InferValueCtx::local_only(&empty),
            ))
        }
        Value::Builtin {
            name: Builtin::ListGet,
            args,
            ..
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
            // Container / Option share projection; other shapes keep recv (≠ Int soft).
            via_gated_recv(Builtin::ListGet, args, list_ty.clone(), |t| {
                matches!(t, Type::List(_) | Type::Set(_) | Type::Map(_, _))
                    || matches!(t, Type::Adt { name, .. } if lumia_hir::is_option(name))
            })
            .or(Some(list_ty))
        }
        Value::Builtin {
            name: Builtin::AdtField,
            args, .. } => adt_field_fixed_ty(block, args, index, trait_methods, param_tys, seen, expanding),
        // `unwrapTask = { t -> t.join() }`: body must yield payload, not Task.
        Value::Builtin {
            name: Builtin::TaskJoin,
            args,
            ..
        } => {
            let recv = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            via_gated_recv(Builtin::TaskJoin, args, recv, |t| {
                matches!(t, Type::Task(_))
            })
        }
        Value::Builtin {
            name: Builtin::ChannelRecv,
            args,
            ..
        } => {
            let recv = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            via_gated_recv(Builtin::ChannelRecv, args, recv, |t| {
                matches!(t, Type::Channel(_))
            })
        }
        Value::Builtin {
            name: Builtin::TaskJoinOpt,
            args,
            ..
        } => {
            let recv = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            via_gated_recv(Builtin::TaskJoinOpt, args, recv, |t| {
                matches!(t, Type::Task(_))
            })
        }
        Value::Builtin {
            name: Builtin::ChannelRecvOpt,
            args,
            ..
        } => {
            let recv = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            via_gated_recv(Builtin::ChannelRecvOpt, args, recv, |t| {
                matches!(t, Type::Channel(_))
            })
        }
        Value::Builtin { name, args, .. }
            if matches!(*name, Builtin::Range | Builtin::RangeInclusive) =>
        {
            let empty = HashMap::default();
            Some(builtin_value_ty(
                *name,
                args,
                InferValueCtx::local_only(&empty),
            ))
        }
        Value::Builtin { name, args, .. }
            if matches!(
                *name,
                Builtin::Elems | Builtin::MapKeys | Builtin::MapValues | Builtin::MapItems
            ) =>
        {
            let recv = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            via_gated_recv(*name, args, recv, |r| elems_family_recv_ok(*name, r))
        }
        Value::Builtin { name, args, .. }
            if matches!(
                *name,
                Builtin::ListTake
                    | Builtin::ListSlice
                    | Builtin::ListReverse
                    | Builtin::ListSort
                    | Builtin::ListSortByKeys
            ) =>
        {
            let recv = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            // Passthrough list builtins share the ListTake value_ty arm.
            via_gated_recv(Builtin::ListTake, args, recv, |t| matches!(t, Type::List(_)))
        }
        Value::Builtin {
            name: Builtin::TaskSpawn,
            args,
            ..
        } => {
            let fun_ty = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            via_gated_recv(Builtin::TaskSpawn, args, fun_ty, |t| {
                matches!(t, Type::Fun(_, _, _))
            })
        }
        Value::Builtin {
            name: Builtin::ChannelNew,
            result_ty,
            args,
            ..
        } => match result_ty {
            Some(Type::Channel(_)) => result_ty.clone(),
            _ => {
                let empty = HashMap::default();
                Some(builtin_value_ty(
                    Builtin::ChannelNew,
                    args,
                    InferValueCtx::local_only(&empty),
                ))
            }
        },
        Value::Builtin {
            name: Builtin::ListAppend,
            args,
            ..
        } => {
            let list = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            via_gated_recv_seeded(
                Builtin::ListAppend,
                args,
                list,
                |t| matches!(t, Type::List(_)),
                |tys| {
                    if let Some(elem) = args.get(1).and_then(|a| {
                        local_fixed_ty(block, a.0, index, trait_methods, param_tys, seen, expanding)
                    }) {
                        tys.insert(args[1].0, elem);
                    }
                },
            )
        }
        Value::Builtin {
            name: Builtin::SetInsert,
            args,
            ..
        } => {
            let set = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            via_gated_recv_seeded(
                Builtin::SetInsert,
                args,
                set,
                |t| matches!(t, Type::Set(_)),
                |tys| {
                    if let Some(elem) = args.get(1).and_then(|a| {
                        local_fixed_ty(block, a.0, index, trait_methods, param_tys, seen, expanding)
                    }) {
                        tys.insert(args[1].0, elem);
                    }
                },
            )
        }
        Value::Builtin {
            name: Builtin::MapRemove,
            args,
            ..
        } => {
            let recv = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            via_gated_recv_seeded(
                Builtin::MapRemove,
                args,
                recv,
                |t| matches!(t, Type::Map(_, _) | Type::List(_) | Type::Set(_)),
                |tys| {
                    if let Some(key) = args.get(1).and_then(|a| {
                        local_fixed_ty(block, a.0, index, trait_methods, param_tys, seen, expanding)
                    }) {
                        tys.insert(args[1].0, key);
                    }
                },
            )
        }
        Value::Builtin {
            name: Builtin::MapSet,
            args,
            ..
        } => {
            // Gate Map|List only — open Int-key→List guess stays out of ret_ty.
            let recv = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            via_gated_recv_seeded(
                Builtin::MapSet,
                args,
                recv,
                |t| matches!(t, Type::Map(_, _) | Type::List(_)),
                |tys| {
                    if let Some(key) = args.get(1).and_then(|a| {
                        local_fixed_ty(block, a.0, index, trait_methods, param_tys, seen, expanding)
                    }) {
                        tys.insert(args[1].0, key);
                    }
                    if let Some(val) = args.get(2).and_then(|a| {
                        local_fixed_ty(block, a.0, index, trait_methods, param_tys, seen, expanding)
                    }) {
                        tys.insert(args[2].0, val);
                    }
                },
            )
        }
        Value::Builtin {
            name: Builtin::ListConcat,
            args,
            ..
        } => {
            // Both sides List only — share soft value_ty upgrade; no float_abi prefer.
            let a = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            let b = local_fixed_ty(
                block,
                args.get(1)?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            match (&a, &b) {
                (Type::String, _) | (_, Type::String) => {
                    let mut local_tys = HashMap::default();
                    local_tys.insert(args[0].0, a);
                    local_tys.insert(args[1].0, b);
                    Some(builtin_value_ty(
                        Builtin::ListConcat,
                        args,
                        InferValueCtx::local_only(&local_tys),
                    ))
                }
                (Type::List(_), Type::List(_)) => {
                    let mut local_tys = HashMap::default();
                    local_tys.insert(args[0].0, a);
                    local_tys.insert(args[1].0, b);
                    Some(builtin_value_ty(
                        Builtin::ListConcat,
                        args,
                        InferValueCtx::local_only(&local_tys),
                    ))
                }
                _ => None,
            }
        }
        Value::Builtin {
            name: Builtin::ListParMap,
            args,
            ..
        } => {
            // Gate List recv only — Float soft upgrades stay in float_abi.
            let recv = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            via_gated_recv_seeded(
                Builtin::ListParMap,
                args,
                recv,
                |t| matches!(t, Type::List(_)),
                |tys| {
                    if let Some(cb) = args.get(1).and_then(|a| {
                        local_fixed_ty(block, a.0, index, trait_methods, param_tys, seen, expanding)
                    }) {
                        tys.insert(args[1].0, cb);
                    }
                },
            )
        }
        Value::Builtin {
            name: Builtin::ListParFold,
            args,
            ..
        } => {
            // Acc type from seed (shared lattice); no Float specials here.
            let acc = local_fixed_ty(
                block,
                args.get(1)?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            if let Some(list) = args.first().and_then(|a| {
                local_fixed_ty(block, a.0, index, trait_methods, param_tys, seen, expanding)
            }) {
                via_gated_recv_seeded(Builtin::ListParFold, args, list, |_| true, |tys| {
                    tys.insert(args[1].0, acc);
                })
            } else {
                let mut local_tys = HashMap::default();
                local_tys.insert(args[1].0, acc);
                Some(builtin_value_ty(
                    Builtin::ListParFold,
                    args,
                    InferValueCtx::local_only(&local_tys),
                ))
            }
        }
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
                    // Cycle: open generic `ret` is useless. Prefer a concrete
                    // call-site arg ABI (fold/acc Float) so `sumAt(xs,i,acc)`
                    // clones keep `ret=Float` instead of key's first-List.
                    for a in args.iter().rev() {
                        if let Some(t) = local_fixed_ty(
                            block, a.0, index, trait_methods, param_tys, seen, expanding,
                        ) {
                            if !matches!(t, Type::Int | Type::Var(_)) {
                                return Some(t);
                            }
                        }
                    }
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
            let params = if lumia_hir::is_result(adt_name) {
                let payload = field_tys.first().cloned().unwrap_or(Type::Int);
                let ok_tag = lumia_hir::RESULT.default_tag("Ok").unwrap_or(0);
                if *tag == ok_tag {
                    vec![payload, Type::Int]
                } else {
                    vec![Type::Int, payload]
                }
            } else if lumia_hir::is_option(adt_name) && field_tys.is_empty() {
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
        Value::AllocList { elems, .. } => {
            let elem = elems.first().and_then(|e| {
                local_fixed_ty(block, e.0, index, trait_methods, param_tys, seen, expanding)
            });
            Some(Type::List(Box::new(elem.unwrap_or(Type::Int))))
        }
        Value::AllocSet { elems, .. } => {
            let elem = elems.first().and_then(|e| {
                local_fixed_ty(block, e.0, index, trait_methods, param_tys, seen, expanding)
            });
            Some(Type::Set(Box::new(elem.unwrap_or(Type::Int))))
        }
        Value::AllocMap { flat_pairs, .. } => {
            let (k, v) = if flat_pairs.len() >= 2 {
                (
                    local_fixed_ty(
                        block,
                        flat_pairs[0].0,
                        index,
                        trait_methods,
                        param_tys,
                        seen,
                        expanding,
                    )
                    .unwrap_or(Type::Int),
                    local_fixed_ty(
                        block,
                        flat_pairs[1].0,
                        index,
                        trait_methods,
                        param_tys,
                        seen,
                        expanding,
                    )
                    .unwrap_or(Type::Int),
                )
            } else {
                (Type::Int, Type::Int)
            };
            Some(Type::Map(Box::new(k), Box::new(v)))
        }
        Value::FunRef(name) | Value::AllocClosure { fun: name, .. } => {
            let f = index.get(name)?;
            Some(Type::Fun(
                f.param_tys.clone(),
                Box::new(f.ret_ty.clone()),
                f.effect,
            ))
        }
        Value::IndirectCall { callee, .. } => {
            match local_fixed_ty(block, callee.0, index, trait_methods, param_tys, seen, expanding) {
                Some(Type::Fun(_, ret, _)) => Some(*ret),
                _ => None,
            }
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

fn merge_slot_ty(prev: Option<Type>, next: Type) -> Type {
    use crate::type_may_heap;
    match (prev, next) {
        (None, t) => t,
        (Some(p), n) if p == n => p,
        // Pointer-carrying slots win over unboxed numeric — never store a
        // List/ADT/Char pointer as Float (XMM NaN canonicalization / missed GC root).
        // Shared lattice: [`crate::type_may_heap`] (was a near-copy `is_ref_ty`).
        (Some(p), n) if type_may_heap(&p) && !type_may_heap(&n) => p,
        (Some(p), n) if !type_may_heap(&p) && type_may_heap(&n) => n,
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
        | BinOp::Ge => Some(Type::Bool),
        BinOp::And | BinOp::Or => {
            debug_assert!(false, "ICE: BinOp::And|Or in Core; expected If desugar");
            Some(Type::Bool)
        }
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
    match &recv_ty {
        Type::Adt { .. } | Type::Tuple(_) | Type::TuplePrefix(_) => {
            let mut local_tys = HashMap::default();
            local_tys.insert(recv.0, recv_ty);
            let mut consts = HashMap::default();
            consts.insert(idx_local.0, idx);
            Some(builtin_value_ty(
                Builtin::AdtField,
                args,
                InferValueCtx::with_int_consts(&local_tys, &consts),
            ))
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
        // MatchFail / empty arm: Unit is bottom.
        (Type::Unit, other) | (other, Type::Unit) => Some(other.clone()),
        // Float beats scalar (String/Bool/Char/Int/Var) — parity with `join_abi_tys`
        // (`Err("e") alt 9.5` / Option alt float must not keep String for println).
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
        // Fun vs scalar — keep Fun (parity with `JoinAbiKind::Value`).
        (Type::Fun(_, _, _), other) | (other, Type::Fun(_, _, _))
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
            match (a, b) {
                (Type::Fun(_, _, _), _) => Some(a.clone()),
                _ => Some(b.clone()),
            }
        }
        // Fun×Fun merge (parity with `join_abi` Value).
        (Type::Fun(p1, r1, e1), Type::Fun(p2, r2, e2)) => {
            let n = p1.len().max(p2.len());
            let mut params = Vec::with_capacity(n);
            for i in 0..n {
                let x = p1.get(i).cloned().unwrap_or(Type::Int);
                let y = p2.get(i).cloned().unwrap_or(Type::Int);
                params.push(join_fixed_ty(&x, &y).unwrap_or(x));
            }
            let ret = join_fixed_ty(r1, r2).unwrap_or_else(|| (**r1).clone());
            Some(Type::Fun(params, Box::new(ret), e1.union(*e2)))
        }
        (Type::Bool, Type::Int | Type::Var(_))
        | (Type::Int | Type::Var(_), Type::Bool) => Some(Type::Bool),
        (Type::String, Type::Int | Type::Var(_))
        | (Type::Int | Type::Var(_), Type::String) => Some(Type::String),
        (Type::Char, Type::Int | Type::Var(_))
        | (Type::Int | Type::Var(_), Type::Char) => Some(Type::Char),
        // Container merges — Heap-style prefer (parity with `join_abi` Heap).
        (Type::List(e1), Type::List(e2)) => Some(Type::List(Box::new(prefer_concrete_heap_ty(
            e1.as_ref().clone(),
            e2.as_ref().clone(),
        )))),
        (Type::Set(e1), Type::Set(e2)) => Some(Type::Set(Box::new(prefer_concrete_heap_ty(
            e1.as_ref().clone(),
            e2.as_ref().clone(),
        )))),
        (Type::Task(e1), Type::Task(e2)) => Some(Type::Task(Box::new(prefer_concrete_heap_ty(
            e1.as_ref().clone(),
            e2.as_ref().clone(),
        )))),
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
            // Prefer Float over String in Result/Option payloads (same lattice as Heap join).
            let n = p1.len().max(p2.len());
            let mut params = Vec::with_capacity(n);
            for i in 0..n {
                params.push(prefer_concrete_heap_ty(
                    p1.get(i).cloned().unwrap_or(Type::Int),
                    p2.get(i).cloned().unwrap_or(Type::Int),
                ));
            }
            Some(Type::Adt {
                name: n1.clone(),
                params,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "ret_ty_tests.rs"]
mod tests;
