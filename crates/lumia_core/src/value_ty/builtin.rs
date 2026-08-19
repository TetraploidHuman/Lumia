//! Builtin `Value` → [`Type`] arms (split from the `value_ty` module).

use super::InferValueCtx;
use crate::Local;
use lumia_hir::Builtin;
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashMap as HashMap;
use std::sync::Arc;

/// Fixed String/Int/Bool/Unit Builtin results (no recv chase).
///
/// `MatchFail` is intentionally **excluded** — float_abi treats it as bottom;
/// `mono/ret_ty` may still via it to Unit via [`builtin_value_ty`].
pub(crate) fn is_fixed_result_builtin(name: Builtin) -> bool {
    matches!(
        name,
        Builtin::Show
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
    )
}

/// ListGet container / Option gate used by ret_ty (float_abi omits Option).
pub(crate) fn list_get_recv_ok(t: &Type, allow_option: bool) -> bool {
    matches!(t, Type::List(_) | Type::Set(_) | Type::Map(_, _))
        || (allow_option && matches!(t, Type::Adt { name, .. } if lumia_hir::is_option(name)))
}

pub(crate) fn task_recv_ok(t: &Type) -> bool {
    matches!(t, Type::Task(_))
}

pub(crate) fn channel_recv_ok(t: &Type) -> bool {
    matches!(t, Type::Channel(_))
}

pub(crate) fn fun_recv_ok(t: &Type) -> bool {
    matches!(t, Type::Fun(_, _, _))
}

pub(crate) fn list_passthrough_ok(t: &Type) -> bool {
    matches!(t, Type::List(_))
}

/// Stamp-first then [`via_gated_recv`] (ChannelRecv / RecvOpt / TaskJoinOpt).
pub(crate) fn stamp_or_via_gated_recv(
    result_ty: &Option<Type>,
    stamp_ok: impl FnOnce(&Type) -> bool,
    name: Builtin,
    args: &[Local],
    recv: Option<Type>,
    gate: impl FnOnce(&Type) -> bool,
) -> Option<Type> {
    stamp_or_via(result_ty, stamp_ok, || {
        via_gated_recv(name, args, recv?, gate)
    })
}

/// Shared ListParFold seeded projection (float early stays in float_abi).
pub(crate) fn list_par_fold_via(args: &[Local], list: Option<Type>, acc: Type) -> Option<Type> {
    let a1 = args.get(1)?.0;
    if let Some(list) = list {
        via_gated_recv_seeded(
            Builtin::ListParFold,
            args,
            list,
            |_| true,
            |tys| {
                tys.insert(a1, acc);
            },
        )
    } else {
        let mut tys = HashMap::default();
        tys.insert(a1, acc);
        Some(builtin_value_ty(
            Builtin::ListParFold,
            args,
            InferValueCtx::local_only(&tys),
        ))
    }
}

/// Shared ListParMap seeded projection (Float soft stays in float_abi).
pub(crate) fn list_par_map_via(
    args: &[Local],
    list: Option<Type>,
    cb_seed: Option<Type>,
) -> Option<Type> {
    let seed = |tys: &mut HashMap<u32, Type>| {
        if let (Some(a1), Some(cb)) = (args.get(1).map(|a| a.0), cb_seed.clone()) {
            tys.insert(a1, cb);
        }
    };
    if let Some(list) = list {
        via_gated_recv_seeded(Builtin::ListParMap, args, list, list_passthrough_ok, seed)
    } else {
        let mut tys = HashMap::default();
        seed(&mut tys);
        Some(builtin_value_ty(
            Builtin::ListParMap,
            args,
            InferValueCtx::local_only(&tys),
        ))
    }
}

/// float_abi ListParFold: Float/scalar early, else [`list_par_fold_via`].
pub(crate) fn float_list_par_fold_ty(
    args: &[Local],
    acc_is_float_local: bool,
    list_elem: Option<&Type>,
    cb_ret: Option<&Type>,
    list: Option<Type>,
    acc: Option<Type>,
) -> Option<Type> {
    if let Some(t) = super::par_fold_float_abi_early(acc_is_float_local, list_elem, cb_ret) {
        return Some(t);
    }
    list_par_fold_via(args, list, acc?)
}

/// float_abi ListParMap: Float early → `List[Float]`, else [`list_par_map_via`].
pub(crate) fn float_list_par_map_ty(
    args: &[Local],
    list_elem: Option<Type>,
    cb_ret: Option<Type>,
    cb_fallback: Option<Type>,
) -> Option<Type> {
    if matches!(
        super::par_map_float_abi_early(list_elem.as_ref(), cb_ret.as_ref()),
        Some(Type::Float)
    ) {
        return Some(Type::List(Arc::new(Type::Float)));
    }
    let list = list_elem.map(|e| Type::List(Arc::new(e)));
    let cb_seed = cb_ret
        .map(|e| Type::Fun(vec![], Arc::new(e), Effect::Pure))
        .or(cb_fallback);
    list_par_map_via(args, list, cb_seed)
}

/// float_abi AdtField: via parent+idx, else Float when the result local is float.
pub(crate) fn float_adt_field_ty(
    args: &[Local],
    parent: Option<Type>,
    idx: Option<i64>,
    result_is_float_local: bool,
) -> Option<Type> {
    let float_fb = || result_is_float_local.then_some(Type::Float);
    let Some(idx) = idx else {
        return float_fb();
    };
    match parent {
        Some(p) => adt_field_via(args, p, idx).or_else(float_fb),
        None => float_fb(),
    }
}

fn adt_field_result_ty(args: &[Local], ctx: InferValueCtx<'_>) -> Type {
    let params: Option<&[Type]> =
        args.first()
            .and_then(|a| ctx.local_tys.get(&a.0))
            .and_then(|t| match t {
                Type::Adt { params, .. } if !params.is_empty() => Some(params.as_slice()),
                Type::Tuple(ts) | Type::TuplePrefix(ts) if !ts.is_empty() => Some(ts.as_slice()),
                _ => None,
            });
    let Some(params) = params else {
        return Type::Int;
    };
    let idx = args
        .get(1)
        .and_then(|a| ctx.local_int_consts.and_then(|m| m.get(&a.0).copied()))
        .unwrap_or(0);
    if idx < 0 {
        return Type::Int;
    }
    params.get(idx as usize).cloned().unwrap_or(Type::Int)
}

/// Seed `args[0]` with `recv` and project via [`builtin_value_ty`] when `gate` holds.
///
/// Shared by `mono/ret_ty` and `float_abi::local_heap_ty` gated Builtin arms.
pub(crate) fn via_gated_recv(
    name: Builtin,
    args: &[Local],
    recv: Type,
    gate: impl FnOnce(&Type) -> bool,
) -> Option<Type> {
    via_gated_recv_seeded(name, args, recv, gate, |_| {})
}

/// Like [`via_gated_recv`], but `seed_extra` may insert elem/key/val arg tys
/// (Append / SetInsert / MapRemove / MapSet).
pub(crate) fn via_gated_recv_seeded(
    name: Builtin,
    args: &[Local],
    recv: Type,
    gate: impl FnOnce(&Type) -> bool,
    seed_extra: impl FnOnce(&mut HashMap<u32, Type>),
) -> Option<Type> {
    if !gate(&recv) {
        return None;
    }
    let a0 = args.first()?;
    let mut local_tys = HashMap::default();
    local_tys.insert(a0.0, recv);
    seed_extra(&mut local_tys);
    Some(builtin_value_ty(
        name,
        args,
        InferValueCtx::local_only(&local_tys),
    ))
}

/// Elems / MapKeys / MapValues / MapItems recv shape gate (shared ret_ty ↔ float_abi).
pub(crate) fn elems_family_recv_ok(name: Builtin, recv: &Type) -> bool {
    match (name, recv) {
        (Builtin::Elems, Type::List(_) | Type::Set(_) | Type::Map(_, _)) => true,
        (Builtin::MapValues | Builtin::MapKeys, Type::Map(_, _)) => true,
        (Builtin::MapItems, Type::Map(_, _) | Type::List(_)) => true,
        _ => false,
    }
}

/// Shared Builtin → [`Type`] projection for `value_ty` and float-ABI `local_heap_ty`.
pub(crate) fn builtin_value_ty(name: Builtin, args: &[Local], ctx: InferValueCtx<'_>) -> Type {
    let local_tys = ctx.local_tys;
    match name {
        Builtin::Show
        | Builtin::ReadStdin
        | Builtin::StrTrim
        | Builtin::StrSplit
        | Builtin::StrSubstring
        | Builtin::StrToLower
        | Builtin::StrToUpper
        | Builtin::ListJoin => Type::String,
        Builtin::ListLen | Builtin::AdtTag => Type::Int,
        Builtin::Contains | Builtin::StrStartsWith | Builtin::StrEndsWith => Type::Bool,
        Builtin::Println
        | Builtin::MatchFail
        | Builtin::Assert
        | Builtin::ChannelSend
        | Builtin::ChannelClose
        | Builtin::ScopeEnter
        | Builtin::ScopeLeave
        | Builtin::ScopeCancel => Type::Unit,
        Builtin::ChannelNew => Type::Channel(Arc::new(
            ctx.channel_elem_hint.cloned().unwrap_or(Type::Int),
        )),
        Builtin::ChannelRecv | Builtin::TaskJoin => args
            .first()
            .and_then(|a| local_tys.get(&a.0))
            .map(|t| match t {
                Type::Channel(e) | Type::Task(e) => {
                    let elem = (**e).clone();
                    if matches!(elem, Type::Int | Type::Var(_)) {
                        if let Some(hint) = ctx.channel_elem_hint {
                            if matches!(t, Type::Channel(_)) {
                                return hint.clone();
                            }
                        }
                    }
                    elem
                }
                _ => Type::Int,
            })
            .unwrap_or_else(|| ctx.channel_elem_hint.cloned().unwrap_or(Type::Int)),
        Builtin::ChannelRecvOpt => args
            .first()
            .and_then(|a| local_tys.get(&a.0))
            .map(|t| match t {
                Type::Channel(e) => {
                    let elem = if matches!(e.as_ref(), Type::Int | Type::Var(_)) {
                        ctx.channel_elem_hint
                            .cloned()
                            .unwrap_or_else(|| (**e).clone())
                    } else {
                        (**e).clone()
                    };
                    Type::Adt {
                        name: lumia_hir::OPTION.name.into(),
                        params: vec![elem],
                    }
                }
                _ => Type::Adt {
                    name: lumia_hir::OPTION.name.into(),
                    params: vec![ctx.channel_elem_hint.cloned().unwrap_or(Type::Int)],
                },
            })
            .unwrap_or(Type::Adt {
                name: lumia_hir::OPTION.name.into(),
                params: vec![ctx.channel_elem_hint.cloned().unwrap_or(Type::Int)],
            }),
        Builtin::TaskJoinOpt => args
            .first()
            .and_then(|a| local_tys.get(&a.0))
            .map(|t| match t {
                Type::Task(e) => Type::Adt {
                    name: lumia_hir::OPTION.name.into(),
                    params: vec![(**e).clone()],
                },
                _ => Type::Adt {
                    name: lumia_hir::OPTION.name.into(),
                    params: vec![Type::Int],
                },
            })
            .unwrap_or(Type::Adt {
                name: lumia_hir::OPTION.name.into(),
                params: vec![Type::Int],
            }),
        Builtin::TaskSpawn => Type::Task(Arc::new(
            args.first()
                .and_then(|a| local_tys.get(&a.0))
                .and_then(|t| match t {
                    Type::Fun(_, r, _) => Some((**r).clone()),
                    _ => None,
                })
                .unwrap_or(Type::Int),
        )),
        Builtin::ListGet => args
            .first()
            .and_then(|a| local_tys.get(&a.0))
            .map(|t| match t {
                Type::List(e) | Type::Set(e) => (**e).clone(),
                Type::Map(_, v) => Type::Adt {
                    name: lumia_hir::OPTION.name.into(),
                    params: vec![(**v).clone()],
                },
                Type::Adt { name, .. } if lumia_hir::is_option(name) => t.clone(),
                _ => Type::Int,
            })
            .unwrap_or(Type::Int),
        Builtin::AdtField => adt_field_result_ty(args, ctx),
        Builtin::ListParFold => args
            .get(1)
            .and_then(|a| local_tys.get(&a.0).cloned())
            .unwrap_or(Type::Int),
        Builtin::ListParMap => {
            let list_ty = args
                .first()
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::List(Arc::new(Type::Int)));
            // Prefer callback Fun ret when known (`map { x -> x + 1.0 }` → List[Float]).
            // Soft open-Var→Float on Float lists is [`super::par_map_result_elem_ty`] /
            // float_abi; concrete Int must stay Int.
            match args.get(1).and_then(|a| local_tys.get(&a.0)) {
                Some(Type::Fun(_, r, _)) => Type::List(r.clone()),
                _ => list_ty,
            }
        }
        Builtin::ListSlice
        | Builtin::ListTake
        | Builtin::ListReverse
        | Builtin::ListSort
        | Builtin::ListSortByKeys => args
            .first()
            .and_then(|a| local_tys.get(&a.0).cloned())
            .unwrap_or(Type::List(Arc::new(Type::Int))),
        Builtin::ListAppend => {
            let list_ty = args
                .first()
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::List(Arc::new(Type::Int)));
            // Empty `listOf()` starts as List[Int]; appending a concrete elem
            // (Float / Task[…] / Fun / …) must upgrade so later ListGet /
            // join / println see the real ABI (`map { spawn {…} }` etc.).
            // Share [`prefer_concrete_heap_ty`] with float_abi (nested shapes).
            match (&list_ty, args.get(1).and_then(|a| local_tys.get(&a.0))) {
                (Type::List(e), Some(elem)) => Type::List(Arc::new(
                    super::prefer_concrete_heap_ty(e.as_ref().clone(), elem.clone()),
                )),
                _ => list_ty,
            }
        }
        Builtin::ListConcat => {
            let a = args
                .first()
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::List(Arc::new(Type::Int)));
            let b = args.get(1).and_then(|a| local_tys.get(&a.0));
            // flatMap: empty `listOf()` acc is List[Int]; concat a concrete chunk
            // (Float / Fun / …) must upgrade so later ListGet + icall / println
            // see the real ABI (same idea as ListAppend).
            // String `.concat` shares this builtin — keep String, not List.
            // List×List elems share [`prefer_concrete_heap_ty`] with float_abi
            // (nested List/Fun shapes — not only soft Int/Var erase).
            match (&a, b) {
                (Type::String, _) | (_, Some(Type::String)) => Type::String,
                (Type::List(e1), Some(Type::List(e2))) => Type::List(Arc::new(
                    super::prefer_concrete_heap_ty(e1.as_ref().clone(), e2.as_ref().clone()),
                )),
                (Type::List(e), _) | (_, Some(Type::List(e)))
                    if matches!(e.as_ref(), Type::Float) =>
                {
                    Type::List(Arc::new(Type::Float))
                }
                _ => a,
            }
        }
        Builtin::Elems => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::List(e) | Type::Set(e)) => Type::List(e.clone()),
            Some(Type::Map(k, _)) => Type::List(k.clone()),
            _ => Type::List(Arc::new(Type::Int)),
        },
        Builtin::MapKeys => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::Map(k, _)) => Type::List(k.clone()),
            _ => Type::List(Arc::new(Type::Int)),
        },
        Builtin::MapValues => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::Map(_, v)) => Type::List(v.clone()),
            _ => Type::List(Arc::new(Type::Int)),
        },
        Builtin::Range | Builtin::RangeInclusive => Type::List(Arc::new(Type::Int)),
        Builtin::MapItems => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::Map(k, v)) => Type::List(Arc::new(Type::Adt {
                name: "__Tuple".into(),
                params: vec![(**k).clone(), (**v).clone()],
            })),
            Some(Type::List(elem)) => Type::List(elem.clone()),
            _ => Type::List(Arc::new(Type::Adt {
                name: "__Tuple".into(),
                params: vec![Type::Int, Type::Int],
            })),
        },
        Builtin::MapSet => {
            let key_ty = args
                .get(1)
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::Int);
            let val_ty = args
                .get(2)
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::Int);
            match args.first().and_then(|a| local_tys.get(&a.0)) {
                Some(Type::List(e)) => Type::List(Arc::new(super::prefer_concrete_heap_ty(
                    e.as_ref().clone(),
                    val_ty,
                ))),
                Some(Type::Map(k, v)) => Type::Map(
                    Arc::new(super::prefer_concrete_heap_ty(k.as_ref().clone(), key_ty)),
                    Arc::new(super::prefer_concrete_heap_ty(v.as_ref().clone(), val_ty)),
                ),
                // Free / poly: Int key ⇒ list index update (not Map).
                _ if matches!(key_ty, Type::Int) => {
                    Type::List(Arc::new(super::prefer_concrete_heap_ty(Type::Int, val_ty)))
                }
                _ => Type::Map(Arc::new(key_ty), Arc::new(val_ty)),
            }
        }
        Builtin::MapRemove => {
            let key_ty = args
                .get(1)
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::Int);
            match args.first().and_then(|a| local_tys.get(&a.0)) {
                Some(Type::List(e)) => Type::List(e.clone()),
                Some(Type::Set(e)) => Type::Set(Arc::new(super::prefer_concrete_heap_ty(
                    e.as_ref().clone(),
                    key_ty,
                ))),
                Some(Type::Map(k, v)) => Type::Map(
                    Arc::new(super::prefer_concrete_heap_ty(k.as_ref().clone(), key_ty)),
                    v.clone(),
                ),
                _ => Type::Map(Arc::new(key_ty), Arc::new(Type::Int)),
            }
        }
        Builtin::SetInsert => {
            let elem_ty = args
                .get(1)
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::Int);
            match args.first().and_then(|a| local_tys.get(&a.0)) {
                Some(Type::Set(e)) => Type::Set(Arc::new(super::prefer_concrete_heap_ty(
                    e.as_ref().clone(),
                    elem_ty,
                ))),
                _ => Type::Set(Arc::new(elem_ty)),
            }
        }
    }
}

/// Known recv that passes `gate` → seeded via; otherwise `open(recv)`.
///
/// Shared scaffolding for float_abi container mutators (Append / MapSet /
/// SetInsert / MapRemove) so open-arm policy stays local while known arms
/// always go through [`via_gated_recv_seeded`].
pub(crate) fn via_gated_or_open(
    name: Builtin,
    args: &[Local],
    recv: Option<Type>,
    gate: impl FnOnce(&Type) -> bool,
    seed_extra: impl FnOnce(&mut HashMap<u32, Type>),
    open: impl FnOnce(Option<Type>) -> Option<Type>,
) -> Option<Type> {
    match recv {
        Some(r) if gate(&r) => via_gated_recv_seeded(name, args, r, |_| true, seed_extra),
        other => open(other),
    }
}

/// float_abi [`ListAppend`]: known List → via prefer; non-List pass-through;
/// open → `List[elem]` (soft `List[Int]` for Int/Var).
pub(crate) fn float_list_append_ty(args: &[Local], recv: Option<Type>, elem: Type) -> Option<Type> {
    let a1 = args.get(1)?.0;
    let elem_for_open = elem.clone();
    via_gated_or_open(
        Builtin::ListAppend,
        args,
        recv,
        |t| matches!(t, Type::List(_)),
        |tys| {
            tys.insert(a1, elem);
        },
        move |recv| match recv {
            Some(other) => Some(other),
            None if !matches!(elem_for_open, Type::Int | Type::Var(_)) => {
                Some(Type::List(Arc::new(elem_for_open)))
            }
            None => Some(Type::List(Arc::new(Type::Int))),
        },
    )
}

/// float_abi [`MapSet`]: known Map|List → via prefer; open → `Map(key,val)`
/// (**never** Int-key→List — that guess stays in `builtin_value_ty` only).
pub(crate) fn float_map_set_ty(
    args: &[Local],
    recv: Option<Type>,
    key: Type,
    val: Type,
) -> Option<Type> {
    let a1 = args.get(1)?.0;
    let a2 = args.get(2)?.0;
    let key_open = key.clone();
    let val_open = val.clone();
    via_gated_or_open(
        Builtin::MapSet,
        args,
        recv,
        |t| matches!(t, Type::Map(_, _) | Type::List(_)),
        |tys| {
            tys.insert(a1, key);
            tys.insert(a2, val);
        },
        move |_| Some(Type::Map(Arc::new(key_open), Arc::new(val_open))),
    )
}

/// float_abi [`SetInsert`]: known Set → via prefer; open → via seeded `Set[elem]`.
pub(crate) fn float_set_insert_ty(args: &[Local], recv: Option<Type>, elem: Type) -> Option<Type> {
    let a1 = args.get(1)?.0;
    let elem_open = elem.clone();
    via_gated_or_open(
        Builtin::SetInsert,
        args,
        recv,
        |t| matches!(t, Type::Set(_)),
        |tys| {
            tys.insert(a1, elem);
        },
        move |_| {
            via_gated_recv_seeded(
                Builtin::SetInsert,
                args,
                Type::Set(Arc::new(elem_open.clone())),
                |_| true,
                |tys| {
                    tys.insert(a1, elem_open);
                },
            )
        },
    )
}

/// float_abi [`MapRemove`]: known Map|List|Set → via prefer; open → via / soft
/// `Map(key, Int)` (never invent List/Set from an unknown recv).
pub(crate) fn float_map_remove_ty(args: &[Local], recv: Option<Type>, key: Type) -> Option<Type> {
    let a1 = args.get(1)?.0;
    let key_open = key.clone();
    via_gated_or_open(
        Builtin::MapRemove,
        args,
        recv,
        |t| matches!(t, Type::Map(_, _) | Type::List(_) | Type::Set(_)),
        |tys| {
            tys.insert(a1, key);
        },
        move |other| {
            let projected = match other {
                Some(r) => via_gated_recv_seeded(
                    Builtin::MapRemove,
                    args,
                    r,
                    |_| true,
                    |tys| {
                        tys.insert(a1, key_open.clone());
                    },
                ),
                None => {
                    let mut tys = HashMap::default();
                    tys.insert(a1, key_open.clone());
                    Some(builtin_value_ty(
                        Builtin::MapRemove,
                        args,
                        InferValueCtx::local_only(&tys),
                    ))
                }
            };
            match projected {
                Some(Type::Map(k, v)) => Some(Type::Map(k, v)),
                _ => Some(Type::Map(Arc::new(key_open), Arc::new(Type::Int))),
            }
        },
    )
}

/// float_abi [`ListConcat`]: both sides → via + String/List filter; one side →
/// concrete String/`List(e)` only (never invent soft `List[Int]`).
pub(crate) fn float_list_concat_ty(
    args: &[Local],
    a: Option<Type>,
    b: Option<Type>,
) -> Option<Type> {
    let a0 = args.first()?.0;
    let a1 = args.get(1)?.0;
    match (a, b) {
        (Some(ta), Some(tb)) => {
            let mut tys = HashMap::default();
            tys.insert(a0, ta.clone());
            tys.insert(a1, tb.clone());
            let via = builtin_value_ty(Builtin::ListConcat, args, InferValueCtx::local_only(&tys));
            match (ta, tb, via) {
                (Type::String, _, Type::String) | (_, Type::String, Type::String) => {
                    Some(Type::String)
                }
                (Type::List(_), Type::List(_), Type::List(e))
                | (Type::List(_), _, Type::List(e))
                | (_, Type::List(_), Type::List(e)) => Some(Type::List(e)),
                _ => None,
            }
        }
        (Some(Type::String), _) | (_, Some(Type::String)) => Some(Type::String),
        (Some(Type::List(e)), _) | (_, Some(Type::List(e))) => Some(Type::List(e)),
        _ => None,
    }
}

/// `mono/ret_ty` ListConcat: both sides known String or List×List → via; else None.
pub(crate) fn list_concat_both_known(args: &[Local], a: Type, b: Type) -> Option<Type> {
    match (&a, &b) {
        (Type::String, _) | (_, Type::String) | (Type::List(_), Type::List(_)) => {
            let a0 = args.first()?.0;
            let a1 = args.get(1)?.0;
            let mut local_tys = HashMap::default();
            local_tys.insert(a0, a);
            local_tys.insert(a1, b);
            Some(builtin_value_ty(
                Builtin::ListConcat,
                args,
                InferValueCtx::local_only(&local_tys),
            ))
        }
        _ => None,
    }
}

/// Shared AdtField projection when parent is Adt/Tuple (seed + int const index).
pub(crate) fn adt_field_via(args: &[Local], parent: Type, idx: i64) -> Option<Type> {
    if !matches!(
        parent,
        Type::Adt { .. } | Type::Tuple(_) | Type::TuplePrefix(_)
    ) {
        return None;
    }
    let a0 = args.first()?.0;
    let a1 = args.get(1)?.0;
    let mut tys = HashMap::default();
    tys.insert(a0, parent);
    let mut consts = HashMap::default();
    consts.insert(a1, idx);
    Some(builtin_value_ty(
        Builtin::AdtField,
        args,
        InferValueCtx::with_int_consts(&tys, &consts),
    ))
}

/// Prefer stamped `result_ty` when `stamp_ok`; else run `via`.
pub(crate) fn stamp_or_via(
    result_ty: &Option<Type>,
    stamp_ok: impl FnOnce(&Type) -> bool,
    via: impl FnOnce() -> Option<Type>,
) -> Option<Type> {
    match result_ty {
        Some(t) if stamp_ok(t) => Some(t.clone()),
        _ => via(),
    }
}
