//! Shared ABI type join / prefer lattice (value_ty If arms + float/heap ABI).
//!
//! Keeps `join_value_tys` and float_abi `join_heap_tys` from drifting apart
//! (Todo: Value→Type 三套 walker / prefer 近拷贝).

use lumia_ty::{Effect, Type};
use std::sync::Arc;

/// Policy for If/alt joining.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinAbiKind {
    /// `Value::If` inference: Fun vs scalar keeps Fun; no container merge;
    /// blanket `Int|Var → other`.
    Value,
    /// Heap/float ABI: merge List/Map/Set/Task/Channel; Adt via [`prefer_concrete_heap_ty`].
    Heap,
    /// Mono `ret_ty` If join: Fun lattice like [`Value`], containers/Adt like
    /// [`Heap`], but **no** blanket `Int|Var → other` (only Bool/String/Char upgrades).
    Fixed,
}

/// Policy for folding successive `Assign` writes to the same named slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinAssignKind {
    /// Float/heap ABI refresh (`float_abi` slot heap typing).
    Heap,
    /// Mono fixed `ret_ty` slot typing ([`join_slot_assign_ty`]).
    Fixed,
}

/// Fold one more slot write into an accumulator (`None` → first write).
#[inline]
pub fn fold_slot_assign_ty(acc: &mut Option<Type>, next: Type, kind: JoinAssignKind) {
    *acc = Some(match acc.take() {
        None => next,
        Some(prev) => match kind {
            JoinAssignKind::Heap => prefer_concrete_heap_ty(prev, next),
            JoinAssignKind::Fixed => join_slot_assign_ty(Some(prev), next),
        },
    });
}

/// Mono fixed-ret If join (thin wrapper over [`join_abi_tys`] + [`JoinAbiKind::Fixed`]).
#[inline]
pub fn join_fixed_ty(a: &Type, b: &Type) -> Option<Type> {
    join_abi_tys(a, b, JoinAbiKind::Fixed)
}

/// Join optional types from `if`/match arms, treating bottom (`MatchFail`) arms as absent.
///
/// Used by float heap ABI ([`JoinAbiKind::Heap`]) and mono fixed ret ([`JoinAbiKind::Fixed`]).
pub fn join_if_arm_tys(
    then_ty: Option<Type>,
    else_ty: Option<Type>,
    then_bottom: bool,
    else_bottom: bool,
    kind: JoinAbiKind,
) -> Option<Type> {
    if then_bottom {
        return else_ty;
    }
    if else_bottom {
        return then_ty;
    }
    match (then_ty, else_ty) {
        (Some(a), Some(b)) => join_abi_tys(&a, &b, kind),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

/// Join two types for If/alt / match arms.
pub fn join_abi_tys(a: &Type, b: &Type, kind: JoinAbiKind) -> Option<Type> {
    if a == b {
        return Some(a.clone());
    }
    let fun_scalar = matches!(kind, JoinAbiKind::Value | JoinAbiKind::Fixed);
    let containers = matches!(kind, JoinAbiKind::Heap | JoinAbiKind::Fixed);
    match (a, b) {
        // MatchFail / empty arm: Unit is bottom, not a real payload.
        (Type::Unit, other) | (other, Type::Unit) => Some(other.clone()),
        // `Result/Option alt float`: then=`AdtField` may see only the Err/None
        // construction params (e.g. String from `Err("e")`) while else is Float.
        // Prefer Float so println does not treat IEEE bits as Int/String.
        (Type::Float, other) | (other, Type::Float)
            if matches!(
                other,
                Type::Int | Type::Var(_) | Type::Bool | Type::String | Type::Char | Type::Float
            ) =>
        {
            Some(Type::Float)
        }
        // `Err("e") alt { x -> … }` / `None alt fun`: String vs Fun — keep Fun.
        (Type::Fun(_, _, _), other) | (other, Type::Fun(_, _, _))
            if fun_scalar
                && matches!(
                    other,
                    Type::Int | Type::Var(_) | Type::Bool | Type::String | Type::Char | Type::Float
                ) =>
        {
            match (a, b) {
                (Type::Fun(_, _, _), _) => Some(a.clone()),
                _ => Some(b.clone()),
            }
        }
        // Prefer Fun when joining two Fun shapes (alt arms / unwrapOr / mono Fixed).
        (Type::Fun(p1, r1, e1), Type::Fun(p2, r2, e2)) if fun_scalar => {
            let n = p1.len().max(p2.len());
            let mut params = Vec::with_capacity(n);
            for i in 0..n {
                let x = p1.get(i).cloned().unwrap_or(Type::Int);
                let y = p2.get(i).cloned().unwrap_or(Type::Int);
                params.push(join_abi_tys(&x, &y, kind).unwrap_or(x));
            }
            let ret = join_abi_tys(r1, r2, kind).unwrap_or_else(|| (**r1).clone());
            Some(Type::Fun(params, Arc::new(ret), e1.union(*e2)))
        }
        // Fixed: upgrade only Bool/String/Char over soft Int (not arbitrary payloads).
        (Type::Bool, Type::Int | Type::Var(_)) | (Type::Int | Type::Var(_), Type::Bool)
            if kind == JoinAbiKind::Fixed =>
        {
            Some(Type::Bool)
        }
        (Type::String, Type::Int | Type::Var(_)) | (Type::Int | Type::Var(_), Type::String)
            if kind == JoinAbiKind::Fixed =>
        {
            Some(Type::String)
        }
        (Type::Char, Type::Int | Type::Var(_)) | (Type::Int | Type::Var(_), Type::Char)
            if kind == JoinAbiKind::Fixed =>
        {
            Some(Type::Char)
        }
        (Type::Int | Type::Var(_), other) if kind != JoinAbiKind::Fixed => Some(other.clone()),
        (other, Type::Int | Type::Var(_)) if kind != JoinAbiKind::Fixed => Some(other.clone()),
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
            let n = p1.len().max(p2.len());
            let mut params = Vec::with_capacity(n);
            for i in 0..n {
                let x = p1.get(i).cloned().unwrap_or(Type::Int);
                let y = p2.get(i).cloned().unwrap_or(Type::Int);
                params.push(match kind {
                    JoinAbiKind::Value => join_abi_tys(&x, &y, kind).unwrap_or(x),
                    JoinAbiKind::Heap | JoinAbiKind::Fixed => prefer_concrete_heap_ty(x, y),
                });
            }
            Some(Type::Adt {
                name: n1.clone(),
                params,
            })
        }
        (Type::List(e1), Type::List(e2)) if containers => Some(Type::List(Arc::new(
            prefer_concrete_heap_ty(e1.as_ref().clone(), e2.as_ref().clone()),
        ))),
        (Type::Set(e1), Type::Set(e2)) if containers => Some(Type::Set(Arc::new(
            prefer_concrete_heap_ty(e1.as_ref().clone(), e2.as_ref().clone()),
        ))),
        (Type::Task(e1), Type::Task(e2)) if containers => Some(Type::Task(Arc::new(
            prefer_concrete_heap_ty(e1.as_ref().clone(), e2.as_ref().clone()),
        ))),
        (Type::Channel(e1), Type::Channel(e2)) if containers => Some(Type::Channel(Arc::new(
            prefer_concrete_heap_ty(e1.as_ref().clone(), e2.as_ref().clone()),
        ))),
        (Type::Map(k1, v1), Type::Map(k2, v2)) if containers => Some(Type::Map(
            Arc::new(prefer_concrete_heap_ty(
                k1.as_ref().clone(),
                k2.as_ref().clone(),
            )),
            Arc::new(prefer_concrete_heap_ty(
                v1.as_ref().clone(),
                v2.as_ref().clone(),
            )),
        )),
        _ => None,
    }
}

/// Prefer a concrete heap/float ABI type when merging placeholders.
///
/// Soft `List(Int)` placeholders yield to Map/Set/Task/String/…; Float beats
/// scalar; Fun shapes are preserved (not collapsed to Float).
pub fn prefer_concrete_heap_ty(x: Type, y: Type) -> Type {
    if x == y {
        return x;
    }
    match (&x, &y) {
        // Fun ABI must not collapse to Float (curried compose / make(k) rets).
        (Type::Fun(p1, r1, e1), Type::Fun(p2, r2, e2)) => {
            let n = p1.len().max(p2.len());
            let mut params = Vec::with_capacity(n);
            for i in 0..n {
                let a = p1.get(i).cloned().unwrap_or(Type::Int);
                let b = p2.get(i).cloned().unwrap_or(Type::Int);
                params.push(prefer_concrete_heap_ty(a, b));
            }
            Type::Fun(
                params,
                Arc::new(prefer_concrete_heap_ty(
                    r1.as_ref().clone(),
                    r2.as_ref().clone(),
                )),
                if e1.has_io() || e2.has_io() {
                    Effect::io()
                } else {
                    Effect::pure()
                },
            )
        }
        (Type::Fun(_, _, _), _) => x.clone(),
        (_, Type::Fun(_, _, _)) => y.clone(),
        // Lift may-heap placeholder `List(Int)` must yield to Map/Set/Task/String/…
        // (`mapOf(…).set` was stuck as List → `.get` used list indexing;
        // spawn String was stuck as List → `.len()` used `lumia_list_len`).
        (
            Type::List(e),
            other @ (Type::Map(_, _)
            | Type::Set(_)
            | Type::Task(_)
            | Type::Channel(_)
            | Type::Adt { .. }
            | Type::String
            | Type::Char),
        ) if matches!(e.as_ref(), Type::Int | Type::Var(_)) => other.clone(),
        (
            other @ (Type::Map(_, _)
            | Type::Set(_)
            | Type::Task(_)
            | Type::Channel(_)
            | Type::Adt { .. }
            | Type::String
            | Type::Char),
            Type::List(e),
        ) if matches!(e.as_ref(), Type::Int | Type::Var(_)) => other.clone(),
        (Type::Float, _) | (_, Type::Float) => Type::Float,
        (Type::Int | Type::Var(_), other) => other.clone(),
        (other, Type::Int | Type::Var(_)) => other.clone(),
        (
            Type::Adt {
                name: n1,
                params: _,
            },
            Type::Adt {
                name: n2,
                params: _,
            },
        ) if n1 == n2 => join_abi_tys(&x, &y, JoinAbiKind::Heap).unwrap_or(x),
        (Type::List(_), Type::List(_))
        | (Type::Set(_), Type::Set(_))
        | (Type::Task(_), Type::Task(_))
        | (Type::Channel(_), Type::Channel(_))
        | (Type::Map(_, _), Type::Map(_, _)) => {
            join_abi_tys(&x, &y, JoinAbiKind::Heap).unwrap_or(x)
        }
        _ => x,
    }
}

/// Join types from successive `Assign` writes to the same named slot (mono `ret_ty`).
///
/// Heap pointers beat Float/scalars — unlike [`prefer_concrete_heap_ty`], `Char`/`String`
/// beat Float so mutable slots are not mis-typed as XMM NaN bit patterns.
pub fn join_slot_assign_ty(prev: Option<Type>, next: Type) -> Type {
    use crate::type_may_heap;
    match (prev, next) {
        (None, t) => t,
        (Some(p), n) if p == n => p,
        (Some(p), n) if type_may_heap(&p) && !type_may_heap(&n) => p,
        (Some(p), n) if !type_may_heap(&p) && type_may_heap(&n) => n,
        (Some(Type::Float), _) | (_, Type::Float) => Type::Float,
        (Some(p), _) => p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_value_float_beats_string() {
        assert_eq!(
            join_abi_tys(&Type::String, &Type::Float, JoinAbiKind::Value),
            Some(Type::Float)
        );
    }

    #[test]
    fn join_heap_merges_list_float() {
        let a = Type::List(Arc::new(Type::Int));
        let b = Type::List(Arc::new(Type::Float));
        assert_eq!(
            join_abi_tys(&a, &b, JoinAbiKind::Heap),
            Some(Type::List(Arc::new(Type::Float)))
        );
    }

    #[test]
    fn join_value_keeps_fun_over_string() {
        let f = Type::Fun(vec![Type::Int], Arc::new(Type::Int), Effect::pure());
        assert_eq!(
            join_abi_tys(&f, &Type::String, JoinAbiKind::Value),
            Some(f.clone())
        );
    }

    #[test]
    fn join_fixed_float_beats_string_and_bool() {
        assert_eq!(
            join_fixed_ty(&Type::String, &Type::Float),
            Some(Type::Float)
        );
        assert_eq!(join_fixed_ty(&Type::Float, &Type::Bool), Some(Type::Float));
        assert_eq!(join_fixed_ty(&Type::Char, &Type::Float), Some(Type::Float));
    }

    #[test]
    fn join_fixed_keeps_fun_over_string() {
        let f = Type::Fun(vec![], Arc::new(Type::Int), Effect::pure());
        assert_eq!(join_fixed_ty(&f, &Type::String), Some(f.clone()));
        assert_eq!(join_fixed_ty(&Type::String, &f), Some(f));
    }

    #[test]
    fn join_fixed_merges_list_and_result_float() {
        assert_eq!(
            join_fixed_ty(
                &Type::List(Arc::new(Type::Int)),
                &Type::List(Arc::new(Type::Float))
            ),
            Some(Type::List(Arc::new(Type::Float)))
        );
        let a = Type::Adt {
            name: "Result".into(),
            params: vec![Type::String, Type::Int],
        };
        let b = Type::Adt {
            name: "Result".into(),
            params: vec![Type::Float, Type::Int],
        };
        assert_eq!(
            join_fixed_ty(&a, &b),
            Some(Type::Adt {
                name: "Result".into(),
                params: vec![Type::Float, Type::Int],
            })
        );
    }

    #[test]
    fn join_fixed_fun_fun_merges_rets() {
        let a = Type::Fun(vec![Type::Int], Arc::new(Type::Int), Effect::pure());
        let b = Type::Fun(vec![Type::Int], Arc::new(Type::Float), Effect::pure());
        assert_eq!(
            join_fixed_ty(&a, &b),
            Some(Type::Fun(
                vec![Type::Int],
                Arc::new(Type::Float),
                Effect::pure()
            ))
        );
    }

    #[test]
    fn join_fixed_does_not_blanket_int_to_list() {
        // Value would yield List; Fixed must stay open (None).
        assert_eq!(
            join_fixed_ty(&Type::Int, &Type::List(Arc::new(Type::Float))),
            None
        );
    }

    #[test]
    fn prefer_list_int_placeholder_yields_to_map() {
        let list = Type::List(Arc::new(Type::Int));
        let map = Type::Map(Arc::new(Type::Int), Arc::new(Type::Float));
        assert_eq!(prefer_concrete_heap_ty(list, map.clone()), map);
    }

    #[test]
    fn join_slot_assign_char_beats_float() {
        assert_eq!(
            join_slot_assign_ty(Some(Type::Char), Type::Float),
            Type::Char
        );
        assert_eq!(
            join_slot_assign_ty(Some(Type::Float), Type::Char),
            Type::Char
        );
        assert_eq!(
            join_slot_assign_ty(Some(Type::List(Arc::new(Type::Int))), Type::Int),
            Type::List(Arc::new(Type::Int))
        );
    }

    #[test]
    fn join_if_arm_tys_bottom_then_takes_else() {
        assert_eq!(
            join_if_arm_tys(
                Some(Type::Int),
                Some(Type::Float),
                true,
                false,
                JoinAbiKind::Heap,
            ),
            Some(Type::Float),
        );
    }

    #[test]
    fn join_if_arm_tys_bottom_else_takes_then() {
        assert_eq!(
            join_if_arm_tys(
                Some(Type::Float),
                Some(Type::Int),
                false,
                true,
                JoinAbiKind::Fixed,
            ),
            Some(Type::Float),
        );
    }

    #[test]
    fn join_if_arm_tys_merges_heap_containers() {
        assert_eq!(
            join_if_arm_tys(
                Some(Type::List(Arc::new(Type::Int))),
                Some(Type::List(Arc::new(Type::Float))),
                false,
                false,
                JoinAbiKind::Heap,
            ),
            Some(Type::List(Arc::new(Type::Float))),
        );
    }

    #[test]
    fn fold_slot_assign_heap_merges_list_elem() {
        let mut acc = None;
        fold_slot_assign_ty(
            &mut acc,
            Type::List(Arc::new(Type::Int)),
            JoinAssignKind::Heap,
        );
        fold_slot_assign_ty(
            &mut acc,
            Type::List(Arc::new(Type::Float)),
            JoinAssignKind::Heap,
        );
        assert_eq!(acc, Some(Type::List(Arc::new(Type::Float))));
    }

    #[test]
    fn fold_slot_assign_fixed_char_beats_float() {
        let mut acc = None;
        fold_slot_assign_ty(&mut acc, Type::Char, JoinAssignKind::Fixed);
        fold_slot_assign_ty(&mut acc, Type::Float, JoinAssignKind::Fixed);
        assert_eq!(acc, Some(Type::Char));
    }
}
