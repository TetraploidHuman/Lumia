#![allow(clippy::too_many_arguments)]

use super::fun_index::FunIndex;
use crate::find_top_level_local_def;
use crate::for_each_named_slot_assign_in_block;
use crate::ir::{Block, CoreFun, Local, Value};
use crate::value_ty::{
    adt_field_via, alloc_list_ty, alloc_map_from_pair, alloc_set_ty, binop_float_or_int,
    builtin_value_ty, channel_recv_ok, elems_family_recv_ok, fun_recv_ok, fun_ret_of_callee_ty,
    is_fixed_result_builtin, join_if_arm_tys, list_concat_both_known, list_get_recv_ok,
    list_par_fold_via, list_par_map_via, list_passthrough_ok, lit_scalar_ty, pad_adt_params,
    stamp_or_via, task_recv_ok, via_gated_recv, via_gated_recv_seeded, InferValueCtx, JoinAbiKind,
};
use std::sync::Arc;
use crate::value_ty::{fold_slot_assign_ty, JoinAssignKind};
use crate::{block_result_is_bottom, CoreBinOp as BinOp, CoreUnOp as UnOp};
use lumia_hir::Builtin;
use lumia_syntax::Sym;
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
                Type::List(_)
                | Type::Map(_, _)
                | Type::Set(_)
                | Type::Task(_)
                | Type::Channel(_) => {
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
                Type::List(Arc::new(inferred.clone()))
            }
            _ => orig.clone(),
        },
        Type::Set(e) => match inferred {
            Type::Set(_) => inferred.clone(),
            Type::Float | Type::Bool | Type::Int | Type::String | Type::Char
                if matches!(e.as_ref(), Type::Var(_)) =>
            {
                Type::Set(Arc::new(inferred.clone()))
            }
            _ => orig.clone(),
        },
        Type::Task(e) => match inferred {
            Type::Task(_) => inferred.clone(),
            Type::Float | Type::Bool | Type::Int | Type::String | Type::Char
                if matches!(e.as_ref(), Type::Var(_)) =>
            {
                Type::Task(Arc::new(inferred.clone()))
            }
            _ => orig.clone(),
        },
        Type::Channel(e) => match inferred {
            Type::Channel(_) => inferred.clone(),
            Type::Float | Type::Bool | Type::Int | Type::String | Type::Char
                if matches!(e.as_ref(), Type::Var(_)) =>
            {
                Type::Channel(Arc::new(inferred.clone()))
            }
            _ => orig.clone(),
        },
        Type::Map(k, v) => match inferred {
            Type::Map(_, _) => inferred.clone(),
            Type::Float | Type::Bool | Type::Int | Type::String | Type::Char
                if matches!(k.as_ref(), Type::Var(_)) =>
            {
                Type::Map(Arc::new(inferred.clone()), v.clone())
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
    trait_methods: &HashMap<(Sym, Sym), Vec<Sym>>,
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

/// Merge the fixed body result with the inferred mono key result.
///
/// This lives in `ret_ty` so `mono/specialize` can share the same "ret lattice"
/// decision logic when upgrading erased Int/Option/... results.
pub(super) fn merge_mono_ret_with_inferred(body: Type, inferred: &Type) -> Type {
    match (&body, inferred) {
        (
            Type::Adt {
                name: bn,
                params: bp,
            },
            Type::Adt {
                name: inan,
                params: ip,
            },
        ) if bn == inan && lumia_hir::is_option_or_result(bn) => {
            let body_payload = bp.first();
            let inf_payload = ip.first();
            if option_result_payload_weaker(body_payload, inf_payload) {
                return inferred.clone();
            }
            refine_mono_container_ret(&body, inferred)
        }
        (Type::Int | Type::Var(_), _) => match inferred {
            // Soft/`Var` body may still need scalar upgrades from the MonoKey
            // (Float ABI, bool, …). **Concrete Int must not** — otherwise
            // `{ x -> 1 }` specialized at Float (`__lam$Float`) gets `ret=Float`
            // and auto-parallel map tags Int `1` as IEEE (denormal Show).
            Type::Float | Type::Bool | Type::String | Type::Char | Type::Fun(_, _, _)
                if matches!(body, Type::Var(_)) =>
            {
                inferred.clone()
            }
            // Do **not** promote body `Int` to List/Map/ADT from the MonoKey.
            // `{ xs -> xs.len() }` body is Int while the key is `$List_Int`;
            // preferring List made Call results look heap-ish (retain on `3`).
            Type::Adt { .. }
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Set(_)
            | Type::Task(_)
            | Type::Channel(_)
                if matches!(body, Type::Var(_)) =>
            {
                inferred.clone()
            }
            _ => body,
        },
        (
            Type::Adt { .. }
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Set(_)
            | Type::Task(_)
            | Type::Channel(_),
            _,
        ) => refine_mono_container_ret(&body, inferred),
        _ => body,
    }
}

fn option_result_payload_weaker(body: Option<&Type>, inferred: Option<&Type>) -> bool {
    let Some(inf) = inferred else {
        return false;
    };
    // Inferred must be a concrete payload worth preferring.
    match inf {
        Type::Int | Type::Var(_) => return false,
        Type::List(e) if matches!(e.as_ref(), Type::Int | Type::Var(_)) => return false,
        _ => {}
    }
    match body {
        None => true,
        // Scalar body from `AdtField(Some(inner))` is concrete. Do not prefer a
        // nested `Option`/`Result` MonoKey shape (`flatten(Some(Some(3)))`
        // inferred `Option[Option[Int]]` over body `Option[Int]`).
        Some(Type::Int | Type::Var(_)) => matches!(
            inf,
            Type::Float | Type::Bool | Type::String | Type::Char | Type::Fun(_, _, _)
        ),
        Some(Type::List(e)) if matches!(e.as_ref(), Type::Int | Type::Var(_)) => {
            matches!(
                inf,
                Type::Float
                    | Type::Bool
                    | Type::String
                    | Type::Char
                    | Type::Fun(_, _, _)
                    | Type::List(_)
            )
        }
        // `Option[Option[Int]]` vs `Option[Float]` from nested andThen join.
        Some(Type::Adt { name, params }) if lumia_hir::is_option_or_result(name) => {
            params
                .first()
                .is_none_or(|p| matches!(p, Type::Int | Type::Var(_)))
                || !matches!(inf, Type::Adt { name: n, .. } if lumia_hir::is_option_or_result(n))
        }
        _ => false,
    }
}

fn local_fixed_ty(
    block: &Block,
    id: u32,
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(Sym, Sym), Vec<Sym>>,
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
        find_top_level_local_def(block, id).and_then(|value| {
            value_fixed_ty(
                block,
                value,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )
        })
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
        Type::Adt { params, .. } => params.iter().any(|p| matches!(p, Type::Int | Type::Var(_))),
        _ => false,
    }
}

fn value_fixed_ty(
    block: &Block,
    value: &Value,
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(Sym, Sym), Vec<Sym>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
) -> Option<Type> {
    match value {
        Value::Local(Local(id)) => {
            local_fixed_ty(block, *id, index, trait_methods, param_tys, seen, expanding)
        }
        Value::Name(name) => slot_fixed_ty(
            block,
            name,
            index,
            trait_methods,
            param_tys,
            seen,
            expanding,
        ),
        Value::Builtin { name, args, .. }
            if is_fixed_result_builtin(*name) || matches!(*name, Builtin::MatchFail) =>
        {
            // Fixed scalar/Unit/String results — share value_ty projection.
            // MatchFail → Unit here; float_abi keeps MatchFail as bottom.
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
                list_get_recv_ok(t, /*allow_option=*/ true)
            })
            .or(Some(list_ty))
        }
        Value::Builtin {
            name: Builtin::AdtField,
            args,
            ..
        } => adt_field_fixed_ty(
            block,
            args,
            index,
            trait_methods,
            param_tys,
            seen,
            expanding,
        ),
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
            via_gated_recv(Builtin::TaskJoin, args, recv, task_recv_ok)
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
            via_gated_recv(Builtin::ChannelRecv, args, recv, channel_recv_ok)
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
            via_gated_recv(Builtin::TaskJoinOpt, args, recv, task_recv_ok)
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
            via_gated_recv(Builtin::ChannelRecvOpt, args, recv, channel_recv_ok)
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
            via_gated_recv(Builtin::ListTake, args, recv, list_passthrough_ok)
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
            via_gated_recv(Builtin::TaskSpawn, args, fun_ty, fun_recv_ok)
        }
        Value::Builtin {
            name: Builtin::ChannelNew,
            result_ty,
            args,
            ..
        } => stamp_or_via(
            result_ty,
            |t| matches!(t, Type::Channel(_)),
            || {
                let empty = HashMap::default();
                Some(builtin_value_ty(
                    Builtin::ChannelNew,
                    args,
                    InferValueCtx::local_only(&empty),
                ))
            },
        ),
        Value::Builtin {
            name: Builtin::ListAppend,
            args,
            ..
        } => mutator_fixed_seeded(
            block,
            Builtin::ListAppend,
            args,
            index,
            trait_methods,
            param_tys,
            seen,
            expanding,
            |t| matches!(t, Type::List(_)),
            &[1],
        ),
        Value::Builtin {
            name: Builtin::SetInsert,
            args,
            ..
        } => mutator_fixed_seeded(
            block,
            Builtin::SetInsert,
            args,
            index,
            trait_methods,
            param_tys,
            seen,
            expanding,
            |t| matches!(t, Type::Set(_)),
            &[1],
        ),
        Value::Builtin {
            name: Builtin::MapRemove,
            args,
            ..
        } => mutator_fixed_seeded(
            block,
            Builtin::MapRemove,
            args,
            index,
            trait_methods,
            param_tys,
            seen,
            expanding,
            |t| matches!(t, Type::Map(_, _) | Type::List(_) | Type::Set(_)),
            &[1],
        ),
        Value::Builtin {
            name: Builtin::MapSet,
            args,
            ..
        } => {
            // Gate Map|List only — open Int-key→List guess stays out of ret_ty.
            mutator_fixed_seeded(
                block,
                Builtin::MapSet,
                args,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
                |t| matches!(t, Type::Map(_, _) | Type::List(_)),
                &[1, 2],
            )
        }
        Value::Builtin {
            name: Builtin::ListConcat,
            args,
            ..
        } => {
            // Both sides String or List×List — share `builtin_value_ty` (prefer).
            // Open one-side policy stays in float_abi (`float_list_concat_ty`).
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
            list_concat_both_known(args, a, b)
        }
        Value::Builtin {
            name: Builtin::ListParMap,
            args,
            ..
        } => {
            // Gate List recv only — Float soft upgrades stay in float_abi.
            let list = local_fixed_ty(
                block,
                args.first()?.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            let cb_seed = args.get(1).and_then(|a| {
                local_fixed_ty(block, a.0, index, trait_methods, param_tys, seen, expanding)
            });
            list_par_map_via(args, Some(list), cb_seed)
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
            let list = args.first().and_then(|a| {
                local_fixed_ty(block, a.0, index, trait_methods, param_tys, seen, expanding)
            });
            list_par_fold_via(args, list, acc)
        }
        v @ (Value::String(_)
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Char(_)) => lit_scalar_ty(v),
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
            UnOp::Neg => local_fixed_ty(
                block,
                operand.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            ),
        },
        Value::Call { fun, args } => {
            let Some(f) = index.get(fun.as_str()) else {
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
                if !expanding.insert(fun.name.to_string()) {
                    // Cycle: open generic `ret` is useless. Prefer a concrete
                    // call-site arg ABI (fold/acc Float) so `sumAt(xs,i,acc)`
                    // clones keep `ret=Float` instead of key's first-List.
                    for a in args.iter().rev() {
                        if let Some(t) = local_fixed_ty(
                            block,
                            a.0,
                            index,
                            trait_methods,
                            param_tys,
                            seen,
                            expanding,
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
                                block,
                                a.0,
                                index,
                                trait_methods,
                                param_tys,
                                seen,
                                expanding,
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
                expanding.remove(fun.as_str());
                if let Some(t) = refined {
                    return Some(t);
                }
                for a in args {
                    if let Some(t) =
                        local_fixed_ty(block, a.0, index, trait_methods, param_tys, seen, expanding)
                    {
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
                pad_adt_params(field_tys, index.sum_max_arity.get(adt_name.as_str()).copied())
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
            let then_bottom = block_result_is_bottom(then_block);
            let else_bottom = block_result_is_bottom(else_block);
            let t = if then_bottom {
                None
            } else {
                block_result_fixed_ty_indexed(
                    then_block,
                    index,
                    trait_methods,
                    param_tys,
                    expanding,
                )
            };
            let e = if else_bottom {
                None
            } else {
                block_result_fixed_ty_indexed(
                    else_block,
                    index,
                    trait_methods,
                    param_tys,
                    expanding,
                )
            };
            join_if_arm_tys(t, e, then_bottom, else_bottom, JoinAbiKind::Fixed)
        }
        Value::AllocList { elems, .. } => {
            let elem = elems.first().and_then(|e| {
                local_fixed_ty(block, e.0, index, trait_methods, param_tys, seen, expanding)
            });
            Some(alloc_list_ty(elem.unwrap_or(Type::Int)))
        }
        Value::AllocSet { elems, .. } => {
            let elem = elems.first().and_then(|e| {
                local_fixed_ty(block, e.0, index, trait_methods, param_tys, seen, expanding)
            });
            Some(alloc_set_ty(elem.unwrap_or(Type::Int)))
        }
        Value::AllocMap { flat_pairs, .. } => {
            let kv = if flat_pairs.len() >= 2 {
                Some((
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
                ))
            } else {
                None
            };
            Some(alloc_map_from_pair(kv))
        }
        Value::FunRef(name) | Value::AllocClosure { fun: name, .. } => {
            let f = index.get(name.as_str())?;
            Some(Type::Fun(
                f.param_tys.clone(),
                Arc::new(f.ret_ty.clone()),
                f.effect,
            ))
        }
        Value::IndirectCall { callee, .. } => fun_ret_of_callee_ty(
            local_fixed_ty(
                block,
                callee.0,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )
            .as_ref(),
        ),
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
    trait_methods: &HashMap<(Sym, Sym), Vec<Sym>>,
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
    trait_methods: &HashMap<(Sym, Sym), Vec<Sym>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
    found: &mut Option<Type>,
) {
    let sym = lumia_syntax::Sym::from(name);
    for_each_named_slot_assign_in_block(block, &sym, &mut |Local(id)| {
        if let Some(t) = local_fixed_ty(block, id, index, trait_methods, param_tys, seen, expanding)
        {
            fold_slot_assign_ty(found, t, JoinAssignKind::Fixed);
        }
    });
}

fn binary_fixed_ty(
    block: &Block,
    op: BinOp,
    left: u32,
    right: u32,
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(Sym, Sym), Vec<Sym>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
) -> Option<Type> {
    match op {
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Some(Type::Bool),
        BinOp::And | BinOp::Or => {
            debug_assert!(false, "ICE: BinOp::And|Or in Core; expected If desugar");
            Some(Type::Bool)
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            let l = local_fixed_ty(
                block,
                left,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            let r = local_fixed_ty(
                block,
                right,
                index,
                trait_methods,
                param_tys,
                seen,
                expanding,
            )?;
            match (&l, &r) {
                (Type::Float, _) | (_, Type::Float) => Some(binop_float_or_int(&l, &r)),
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
    trait_methods: &HashMap<(Sym, Sym), Vec<Sym>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
) -> Option<Type> {
    let recv = args.first()?;
    let idx_local = args.get(1)?;
    let recv_ty = local_fixed_ty(
        block,
        recv.0,
        index,
        trait_methods,
        param_tys,
        seen,
        expanding,
    )?;
    let idx = int_const_in_block(block, idx_local.0)?;
    if idx < 0 {
        return None;
    }
    adt_field_via(args, recv_ty, idx)
}

/// Resolve recv + seed arg tys, then [`via_gated_recv_seeded`] (ret_ty mutators).
fn mutator_fixed_seeded(
    block: &Block,
    name: Builtin,
    args: &[Local],
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(Sym, Sym), Vec<Sym>>,
    param_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    expanding: &mut HashSet<String>,
    gate: impl FnOnce(&Type) -> bool,
    seed_idxs: &[usize],
) -> Option<Type> {
    let recv = local_fixed_ty(
        block,
        args.first()?.0,
        index,
        trait_methods,
        param_tys,
        seen,
        expanding,
    )?;
    let mut seeds = Vec::new();
    for &i in seed_idxs {
        let a = args.get(i)?;
        if let Some(t) =
            local_fixed_ty(block, a.0, index, trait_methods, param_tys, seen, expanding)
        {
            seeds.push((a.0, t));
        }
    }
    via_gated_recv_seeded(name, args, recv, gate, |tys| {
        for (id, t) in seeds {
            tys.insert(id, t);
        }
    })
}

fn int_const_in_block(block: &Block, id: u32) -> Option<i64> {
    match find_top_level_local_def(block, id)? {
        Value::Int(n) => Some(*n),
        Value::Local(Local(src)) => int_const_in_block(block, *src),
        _ => None,
    }
}

fn block_result_fixed_ty_indexed(
    block: &Block,
    index: &FunIndex<'_>,
    trait_methods: &HashMap<(Sym, Sym), Vec<Sym>>,
    param_tys: &HashMap<u32, Type>,
    expanding: &mut HashSet<String>,
) -> Option<Type> {
    let Local(r) = block.result?;
    let mut seen = HashSet::default();
    local_fixed_ty(
        block,
        r,
        index,
        trait_methods,
        param_tys,
        &mut seen,
        expanding,
    )
}

#[cfg(test)]
#[path = "tests/ret_ty_tests.rs"]
mod tests;
