//! Builtin `Value` → [`Type`] arms (split from the `value_ty` module).

use super::InferValueCtx;
use crate::Local;
use lumia_hir::Builtin;
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

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

/// Merge branch result types for `Value::If` (pad sum ADT params to a shared width).
fn upgrade_placeholder(old: &Type, new: Type) -> Type {
    if matches!(new, Type::Float) {
        return Type::Float;
    }
    if matches!(old, Type::Int | Type::Var(_)) && !matches!(new, Type::Int | Type::Var(_)) {
        return new;
    }
    old.clone()
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
        Builtin::Println | Builtin::MatchFail | Builtin::Assert
        | Builtin::ChannelSend
        | Builtin::ChannelClose
        | Builtin::ScopeEnter
        | Builtin::ScopeLeave
        | Builtin::ScopeCancel => Type::Unit,
        Builtin::ChannelNew => Type::Channel(Box::new(
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
            .unwrap_or_else(|| {
                ctx.channel_elem_hint
                    .cloned()
                    .unwrap_or(Type::Int)
            }),
        Builtin::ChannelRecvOpt => args
            .first()
            .and_then(|a| local_tys.get(&a.0))
            .map(|t| match t {
                Type::Channel(e) => {
                    let elem = if matches!(e.as_ref(), Type::Int | Type::Var(_)) {
                        ctx.channel_elem_hint.cloned().unwrap_or_else(|| (**e).clone())
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
        Builtin::TaskSpawn => Type::Task(Box::new(
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
                .unwrap_or(Type::List(Box::new(Type::Int)));
            // Prefer callback Fun ret when known (`map { x -> x + 1.0 }` → List[Float]).
            // Soft Float-from-list when Fun ret is Int/Var stays in float_abi specials.
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
            .unwrap_or(Type::List(Box::new(Type::Int))),
        Builtin::ListAppend => {
            let list_ty = args
                .first()
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::List(Box::new(Type::Int)));
            // Empty `listOf()` starts as List[Int]; appending a concrete elem
            // (Float / Task[…] / Fun / …) must upgrade so later ListGet /
            // join / println see the real ABI (`map { spawn {…} }` etc.).
            match (&list_ty, args.get(1).and_then(|a| local_tys.get(&a.0))) {
                (Type::List(e), Some(elem))
                    if matches!(e.as_ref(), Type::Int | Type::Var(_))
                        && !matches!(elem, Type::Int | Type::Var(_)) =>
                {
                    Type::List(Box::new(elem.clone()))
                }
                _ => list_ty,
            }
        }
        Builtin::ListConcat => {
            let a = args
                .first()
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::List(Box::new(Type::Int)));
            let b = args.get(1).and_then(|a| local_tys.get(&a.0));
            // flatMap: empty `listOf()` acc is List[Int]; concat a concrete chunk
            // (Float / Fun / …) must upgrade so later ListGet + icall / println
            // see the real ABI (same idea as ListAppend).
            // String `.concat` shares this builtin — keep String, not List.
            match (&a, b) {
                (Type::String, _) | (_, Some(Type::String)) => Type::String,
                (Type::List(e1), Some(Type::List(e2))) => {
                    let erased = |t: &Type| matches!(t, Type::Int | Type::Var(_));
                    if erased(e1.as_ref()) && !erased(e2.as_ref()) {
                        Type::List(e2.clone())
                    } else if !erased(e1.as_ref()) && erased(e2.as_ref()) {
                        Type::List(e1.clone())
                    } else if matches!(e1.as_ref(), Type::Float)
                        || matches!(e2.as_ref(), Type::Float)
                    {
                        Type::List(Box::new(Type::Float))
                    } else {
                        a
                    }
                }
                (Type::List(e), _) | (_, Some(Type::List(e)))
                    if matches!(e.as_ref(), Type::Float) =>
                {
                    Type::List(Box::new(Type::Float))
                }
                _ => a,
            }
        }
        Builtin::Elems => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::List(e) | Type::Set(e)) => Type::List(e.clone()),
            Some(Type::Map(k, _)) => Type::List(k.clone()),
            _ => Type::List(Box::new(Type::Int)),
        },
        Builtin::MapKeys => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::Map(k, _)) => Type::List(k.clone()),
            _ => Type::List(Box::new(Type::Int)),
        },
        Builtin::MapValues => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::Map(_, v)) => Type::List(v.clone()),
            _ => Type::List(Box::new(Type::Int)),
        },
        Builtin::Range | Builtin::RangeInclusive => Type::List(Box::new(Type::Int)),
        Builtin::MapItems => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::Map(k, v)) => Type::List(Box::new(Type::Adt {
                name: "__Tuple".into(),
                params: vec![(**k).clone(), (**v).clone()],
            })),
            Some(Type::List(elem)) => Type::List(elem.clone()),
            _ => Type::List(Box::new(Type::Adt {
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
                Some(Type::List(e)) => {
                    Type::List(Box::new(upgrade_placeholder(e.as_ref(), val_ty)))
                }
                Some(Type::Map(k, v)) => Type::Map(
                    Box::new(upgrade_placeholder(k.as_ref(), key_ty)),
                    Box::new(upgrade_placeholder(v.as_ref(), val_ty)),
                ),
                // Free / poly: Int key ⇒ list index update (not Map).
                _ if matches!(key_ty, Type::Int) => {
                    Type::List(Box::new(upgrade_placeholder(&Type::Int, val_ty)))
                }
                _ => Type::Map(Box::new(key_ty), Box::new(val_ty)),
            }
        }
        Builtin::MapRemove => {
            let key_ty = args
                .get(1)
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::Int);
            match args.first().and_then(|a| local_tys.get(&a.0)) {
                Some(Type::List(e)) => Type::List(e.clone()),
                Some(Type::Set(e)) => {
                    Type::Set(Box::new(upgrade_placeholder(e.as_ref(), key_ty)))
                }
                Some(Type::Map(k, v)) => Type::Map(
                    Box::new(upgrade_placeholder(k.as_ref(), key_ty)),
                    v.clone(),
                ),
                _ => Type::Map(Box::new(key_ty), Box::new(Type::Int)),
            }
        }
        Builtin::SetInsert => {
            let elem_ty = args
                .get(1)
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::Int);
            match args.first().and_then(|a| local_tys.get(&a.0)) {
                Some(Type::Set(e)) => {
                    Type::Set(Box::new(upgrade_placeholder(e.as_ref(), elem_ty)))
                }
                _ => Type::Set(Box::new(elem_ty)),
            }
        }
    }
}

