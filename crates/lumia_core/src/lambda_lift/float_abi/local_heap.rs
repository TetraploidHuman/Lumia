//! Local heap-type inference (`local_heap_ty` and Builtin arms).

use crate::ir::{Block, Local, Value};
use crate::value_ty::{
    alloc_list_ty, alloc_map_from_pair, alloc_set_ty, channel_recv_ok, elems_family_recv_ok,
    float_adt_field_ty, float_arith_binop_ty, float_list_append_ty, float_list_concat_ty,
    float_list_par_fold_ty, float_list_par_map_ty, float_map_remove_ty, float_map_set_ty,
    float_set_insert_ty, fun_recv_ok, fun_ret_of_callee_ty, is_fixed_result_builtin,
    list_get_recv_ok, list_passthrough_ok, lit_scalar_ty, stamp_or_via, stamp_or_via_gated_recv,
    task_recv_ok, via_gated_recv, InferValueCtx,
};
use crate::{CoreBinOp as BinOp, CoreUnOp as UnOp};
use lumia_syntax::Sym;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(crate) fn block_result_heap_ty(
    block: &Block,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
) -> Option<Type> {
    block_result_heap_ty_caps(block, fun_ret_tys, fun_param_tys, &HashMap::default())
}

/// Like [`block_result_heap_ty`], with known `ClosureCap` types from AllocClosure sites.
///
/// Callers must install FunKind lifted names via [`crate::lambda_lift::with_lifted_lambda_names`]
/// (or go through [`collect_fun_cap_tys`] / lift / fixup entry points).
pub(crate) fn block_result_heap_ty_caps(
    block: &Block,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
) -> Option<Type> {
    let Local(r) = block.result?;
    let float_locals = super::mark_float::compute_float_locals_in_block(block);
    local_heap_ty(
        block,
        r,
        &float_locals,
        fun_ret_tys,
        fun_param_tys,
        cap_tys,
        &mut HashSet::default(),
        &mut HashSet::default(),
    )
}

pub(super) fn heap_ty_via_builtin_value_ty(
    name: lumia_hir::Builtin,
    args: &[Local],
    local_tys: &HashMap<u32, Type>,
) -> Type {
    heap_ty_via_builtin_value_ty_ex(name, args, local_tys, None)
}

pub(super) fn heap_ty_via_builtin_value_ty_ex(
    name: lumia_hir::Builtin,
    args: &[Local],
    local_tys: &HashMap<u32, Type>,
    local_int_consts: Option<&HashMap<u32, i64>>,
) -> Type {
    let mut ctx = InferValueCtx::local_only(local_tys);
    ctx.local_int_consts = local_int_consts;
    crate::value_ty::builtin_value_ty(name, args, ctx)
}

/// Arg type for MapSet/Append/SetInsert-style upgrades: float locals win, else DFS.
pub(super) fn resolve_heap_arg_ty(
    block: &Block,
    id: u32,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    seen_slots: &mut HashSet<Sym>,
) -> Type {
    if float_locals.contains(&id) {
        Type::Float
    } else {
        local_heap_ty(
            block,
            id,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        )
        .unwrap_or(Type::Int)
    }
}

/// Recv (`args[0]`) + float-first arg tys at `arg_idxs` for container mutators.
pub(super) fn mutator_recv_args(
    block: &Block,
    args: &[Local],
    arg_idxs: &[usize],
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    seen_slots: &mut HashSet<Sym>,
) -> (Option<Type>, Vec<Type>) {
    let recv = local_heap_ty(
        block,
        args[0].0,
        float_locals,
        fun_ret_tys,
        fun_param_tys,
        cap_tys,
        seen,
        seen_slots,
    );
    let extras = arg_idxs
        .iter()
        .map(|&i| {
            resolve_heap_arg_ty(
                block,
                args[i].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            )
        })
        .collect();
    (recv, extras)
}

/// Ground `result_ty` stamp usable as float/heap ABI truth.

pub(super) fn local_heap_ty(
    block: &Block,
    id: u32,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    seen_slots: &mut HashSet<Sym>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    let value = crate::find_local_def(block, id)?;
    match value {
        Value::Local(Local(src)) => local_heap_ty(
            block,
            *src,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        ),
        Value::Name(n) => super::helpers::slot_heap_ty(
            block,
            n,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        ),
        Value::FunRef(name) | Value::AllocClosure { fun: name, .. } => {
            super::super::fun_ty_from_tables_tls(name, fun_ret_tys, fun_param_tys)
        }
        // Int/Unit stay open (`None`) — soft Int must not erase heap refinement.
        v @ (Value::String(_) | Value::Char(_) | Value::Float(_) | Value::Bool(_)) => {
            lit_scalar_ty(v)
        }
        Value::ClosureCap { index, .. } => cap_tys.get(index).cloned(),
        Value::Call { fun, .. } => fun_ret_tys.get(fun.as_str()).cloned(),
        Value::Binary { op, left, right }
            if !super::mark_float::binary_produces_bool(*op)
                && matches!(
                    *op,
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
                ) =>
        {
            let lt = local_heap_ty(
                block,
                left.0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            let rt = local_heap_ty(
                block,
                right.0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            float_arith_binop_ty(lt.as_ref(), rt.as_ref())
        }
        Value::Unary {
            op: UnOp::Neg,
            operand,
        } => match local_heap_ty(
            block,
            operand.0,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        ) {
            Some(Type::Float) => Some(Type::Float),
            _ => None,
        },
        Value::IndirectCall { callee, .. } => {
            let callee_ty = local_heap_ty(
                block,
                callee.0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            fun_ret_of_callee_ty(callee_ty.as_ref())
                .or_else(|| super::helpers::fun_ret_of_local(block, callee.0, fun_ret_tys, seen))
        }
        Value::AllocList { elems, .. } => Some(alloc_list_ty(super::helpers::alloc_elems_ty(
            block,
            elems,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        ))),
        Value::AllocSet { elems, .. } => Some(alloc_set_ty(super::helpers::alloc_elems_ty(
            block,
            elems,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        ))),
        Value::AllocMap { flat_pairs, .. } => {
            let kv = if flat_pairs.len() >= 2 {
                Some((
                    resolve_heap_arg_ty(
                        block,
                        flat_pairs[0].0,
                        float_locals,
                        fun_ret_tys,
                        fun_param_tys,
                        cap_tys,
                        seen,
                        seen_slots,
                    ),
                    resolve_heap_arg_ty(
                        block,
                        flat_pairs[1].0,
                        float_locals,
                        fun_ret_tys,
                        fun_param_tys,
                        cap_tys,
                        seen,
                        seen_slots,
                    ),
                ))
            } else {
                None
            };
            Some(alloc_map_from_pair(kv))
        }
        Value::AllocAdt {
            adt_name, fields, ..
        } => {
            let params: Vec<Type> = fields
                .iter()
                .map(|f| {
                    resolve_heap_arg_ty(
                        block,
                        f.0,
                        float_locals,
                        fun_ret_tys,
                        fun_param_tys,
                        cap_tys,
                        seen,
                        seen_slots,
                    )
                })
                .collect();
            Some(Type::Adt {
                name: adt_name.clone(),
                params,
            })
        }
        // Prefer lower/HIR `result_ty` stamps before hand-maintained Builtin arms.
        // Missing arms used to fall through → soft `List[Int]` (ListSortByKeys /
        // MapItems / ChannelNew regressions). Soft `Int` / open `Var` are ignored
        // so DFS refinement still runs when the stamp is only a scalar placeholder.
        Value::Builtin {
            result_ty: Some(t), ..
        } if super::helpers::stamped_abi_is_authoritative(t) => Some(t.clone()),
        Value::Builtin {
            name: lumia_hir::Builtin::ListGet,
            args,
            ..
        } if !args.is_empty() => {
            // Shared projection with `value_ty`; skip Option/`_` soft Int (heap wants None).
            let recv = local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            )?;
            via_gated_recv(lumia_hir::Builtin::ListGet, args, recv, |t| {
                list_get_recv_ok(t, /*allow_option=*/ false)
            })
        }
        Value::Builtin {
            name: name @ (lumia_hir::Builtin::Range | lumia_hir::Builtin::RangeInclusive),
            args,
            ..
        } => Some(heap_ty_via_builtin_value_ty(
            *name,
            args,
            &HashMap::default(),
        )),
        Value::Builtin {
            name:
                name @ (lumia_hir::Builtin::Elems
                | lumia_hir::Builtin::MapValues
                | lumia_hir::Builtin::MapKeys
                | lumia_hir::Builtin::MapItems),
            args,
            ..
        } if !args.is_empty() => {
            let recv = local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            )?;
            via_gated_recv(*name, args, recv, |r| elems_family_recv_ok(*name, r))
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ChannelNew,
            result_ty,
            ..
        } => stamp_or_via(
            result_ty,
            |t| matches!(t, Type::Channel(_)),
            || {
                // Soft / unstamped: same as `builtin_value_ty` without channel hint.
                Some(heap_ty_via_builtin_value_ty(
                    lumia_hir::Builtin::ChannelNew,
                    &[],
                    &HashMap::default(),
                ))
            },
        ),
        Value::Builtin {
            name: lumia_hir::Builtin::ChannelRecv,
            args,
            result_ty,
            ..
        } if !args.is_empty() => stamp_or_via_gated_recv(
            result_ty,
            |t| !matches!(t, Type::Int | Type::Var(_)),
            lumia_hir::Builtin::ChannelRecv,
            args,
            local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ),
            channel_recv_ok,
        ),
        Value::Builtin {
            name: lumia_hir::Builtin::ChannelRecvOpt,
            args,
            result_ty,
            ..
        } if !args.is_empty() => stamp_or_via_gated_recv(
            result_ty,
            |t| matches!(t, Type::Adt { .. }),
            lumia_hir::Builtin::ChannelRecvOpt,
            args,
            local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ),
            channel_recv_ok,
        ),
        Value::Builtin {
            name:
                lumia_hir::Builtin::ListTake
                | lumia_hir::Builtin::ListSlice
                | lumia_hir::Builtin::ListReverse
                | lumia_hir::Builtin::ListSort
                | lumia_hir::Builtin::ListSortByKeys,
            args,
            ..
        } if !args.is_empty() => {
            let recv = local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            )?;
            // Passthrough list builtins share the ListTake value_ty arm.
            via_gated_recv(
                lumia_hir::Builtin::ListTake,
                args,
                recv,
                list_passthrough_ok,
            )
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ListConcat,
            args,
            ..
        } if args.len() >= 2 => {
            let a = local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            let b = local_heap_ty(
                block,
                args[1].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            float_list_concat_ty(args, a, b)
        }
        Value::Builtin { name, .. } if is_fixed_result_builtin(*name) => Some(
            heap_ty_via_builtin_value_ty(*name, &[], &HashMap::default()),
        ),
        // MatchFail stays bottom ([`crate::block_result_is_bottom`]) — not Unit via.
        Value::Builtin {
            name: lumia_hir::Builtin::AdtField,
            args,
            ..
        } if args.len() >= 2 => {
            let idx = match crate::find_local_def(block, args[1].0) {
                Some(Value::Int(i)) if *i >= 0 => Some(*i as i64),
                _ => None,
            };
            let parent = local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            float_adt_field_ty(args, parent, idx, float_locals.contains(&id))
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ListAppend,
            args,
            ..
        } if args.len() >= 2 => {
            let (recv, extras) = mutator_recv_args(
                block,
                args,
                &[1],
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            float_list_append_ty(args, recv, extras.into_iter().next().unwrap_or(Type::Int))
        }
        Value::Builtin {
            name: lumia_hir::Builtin::MapSet,
            args,
            ..
        } if args.len() >= 3 => {
            // `m.set(k,v)` / `xs.set(i,v)` — float values must upgrade Map/List ABI.
            let (recv, extras) = mutator_recv_args(
                block,
                args,
                &[1, 2],
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            let mut it = extras.into_iter();
            float_map_set_ty(
                args,
                recv,
                it.next().unwrap_or(Type::Int),
                it.next().unwrap_or(Type::Int),
            )
        }
        Value::Builtin {
            name: lumia_hir::Builtin::SetInsert,
            args,
            ..
        } if args.len() >= 2 => {
            let (recv, extras) = mutator_recv_args(
                block,
                args,
                &[1],
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            float_set_insert_ty(args, recv, extras.into_iter().next().unwrap_or(Type::Int))
        }
        Value::Builtin {
            name: lumia_hir::Builtin::MapRemove,
            args,
            ..
        } if args.len() >= 2 => {
            let (recv, extras) = mutator_recv_args(
                block,
                args,
                &[1],
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            float_map_remove_ty(args, recv, extras.into_iter().next().unwrap_or(Type::Int))
        }
        Value::Builtin {
            name: lumia_hir::Builtin::TaskSpawn,
            args,
            ..
        } if !args.is_empty() => {
            let elem = super::helpers::fun_ret_of_local(block, args[0].0, fun_ret_tys, seen)
                .unwrap_or(Type::Int);
            // Seed a Fun so shared `builtin_value_ty` extracts Task[ret] (arity unused).
            via_gated_recv(
                lumia_hir::Builtin::TaskSpawn,
                args,
                Type::Fun(vec![], Box::new(elem), Effect::Pure),
                fun_recv_ok,
            )
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ListParFold,
            args,
            ..
        } if args.len() >= 2 => {
            let list = local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            let list_elem_owned = match &list {
                Some(Type::List(e)) => Some(e.as_ref().clone()),
                _ => None,
            };
            let cb_ret = if args.len() >= 3 {
                super::helpers::fun_ret_of_local(block, args[2].0, fun_ret_tys, seen)
            } else {
                None
            };
            let acc = local_heap_ty(
                block,
                args[1].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            );
            float_list_par_fold_ty(
                args,
                float_locals.contains(&args[1].0),
                list_elem_owned.as_ref(),
                cb_ret.as_ref(),
                list,
                acc,
            )
        }
        Value::Builtin {
            name: lumia_hir::Builtin::ListParMap,
            args,
            ..
        } if args.len() >= 2 => {
            // Result elem follows the callback Fun ret (`map { x -> x + 1.0 }`).
            let from_cb = super::helpers::fun_ret_of_local(block, args[1].0, fun_ret_tys, seen)
                .or_else(|| {
                    match local_heap_ty(
                        block,
                        args[1].0,
                        float_locals,
                        fun_ret_tys,
                        fun_param_tys,
                        cap_tys,
                        seen,
                        seen_slots,
                    ) {
                        Some(Type::Fun(_, r, _)) => Some(*r),
                        _ => None,
                    }
                });
            let from_list = match local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ) {
                Some(Type::List(e)) => Some(*e),
                _ => None,
            };
            let cb_fallback = if from_cb.is_none() {
                local_heap_ty(
                    block,
                    args[1].0,
                    float_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    cap_tys,
                    seen,
                    seen_slots,
                )
            } else {
                None
            };
            float_list_par_map_ty(args, from_list, from_cb, cb_fallback)
        }
        Value::Builtin {
            name: lumia_hir::Builtin::TaskJoin,
            args,
            ..
        } if !args.is_empty() => {
            let recv = local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            )?;
            via_gated_recv(lumia_hir::Builtin::TaskJoin, args, recv, task_recv_ok)
        }
        Value::Builtin {
            name: lumia_hir::Builtin::TaskJoinOpt,
            args,
            result_ty,
            ..
        } if !args.is_empty() => stamp_or_via_gated_recv(
            result_ty,
            |t| matches!(t, Type::Adt { .. }),
            lumia_hir::Builtin::TaskJoinOpt,
            args,
            local_heap_ty(
                block,
                args[0].0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            ),
            task_recv_ok,
        ),
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            // Resolve arm results via `block` (search root), not the nested
            // arm block alone — arm bodies reference outer locals (flatMap
            // `AdtField` of a `ListGet` defined in the loop body).
            let then_ty = then_block.result.and_then(|Local(r)| {
                local_heap_ty(
                    block,
                    r,
                    float_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    cap_tys,
                    seen,
                    seen_slots,
                )
            });
            let else_ty = else_block.result.and_then(|Local(r)| {
                local_heap_ty(
                    block,
                    r,
                    float_locals,
                    fun_ret_tys,
                    fun_param_tys,
                    cap_tys,
                    seen,
                    seen_slots,
                )
            });
            crate::value_ty::join_if_arm_tys(
                then_ty,
                else_ty,
                crate::block_result_is_bottom(then_block),
                crate::block_result_is_bottom(else_block),
                crate::value_ty::JoinAbiKind::Heap,
            )
        }
        _ => None,
    }
}
