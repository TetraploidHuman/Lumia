//! Shared ABI type join / prefer lattice (value_ty If arms + float/heap ABI).
//!
//! Keeps `join_value_tys` and float_abi `join_heap_tys` from drifting apart
//! (Todo: Value→Type 三套 walker / prefer 近拷贝).

use lumia_ty::{Effect, Type};

/// Policy for If/alt joining.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinAbiKind {
    /// `Value::If` inference: Fun vs scalar keeps Fun; no container merge.
    Value,
    /// Heap/float ABI: merge List/Map/Set/Task/Channel; Adt via [`prefer_concrete_heap_ty`].
    Heap,
}

/// Join two types for If/alt / match arms.
pub fn join_abi_tys(a: &Type, b: &Type, kind: JoinAbiKind) -> Option<Type> {
    if a == b {
        return Some(a.clone());
    }
    match (a, b) {
        // MatchFail / empty arm: Unit is bottom, not a real payload.
        (Type::Unit, other) | (other, Type::Unit) => Some(other.clone()),
        // `Result/Option alt float`: then=`AdtField` may see only the Err/None
        // construction params (e.g. String from `Err("e")`) while else is Float.
        // Prefer Float so println does not treat IEEE bits as Int/String.
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
        // `Err("e") alt { x -> … }` / `None alt fun`: String vs Fun — keep Fun.
        (Type::Fun(_, _, _), other) | (other, Type::Fun(_, _, _))
            if kind == JoinAbiKind::Value
                && matches!(
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
        (Type::Int | Type::Var(_), other) => Some(other.clone()),
        (other, Type::Int | Type::Var(_)) => Some(other.clone()),
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
                    JoinAbiKind::Heap => prefer_concrete_heap_ty(x, y),
                });
            }
            Some(Type::Adt {
                name: n1.clone(),
                params,
            })
        }
        // Prefer Fun when joining two Fun shapes (alt arms / unwrapOr).
        (Type::Fun(p1, r1, e1), Type::Fun(p2, r2, e2)) if kind == JoinAbiKind::Value => {
            let n = p1.len().max(p2.len());
            let mut params = Vec::with_capacity(n);
            for i in 0..n {
                let x = p1.get(i).cloned().unwrap_or(Type::Int);
                let y = p2.get(i).cloned().unwrap_or(Type::Int);
                params.push(join_abi_tys(&x, &y, kind).unwrap_or(x));
            }
            let ret = join_abi_tys(r1, r2, kind).unwrap_or_else(|| (**r1).clone());
            Some(Type::Fun(params, Box::new(ret), e1.union(*e2)))
        }
        (Type::List(e1), Type::List(e2)) if kind == JoinAbiKind::Heap => {
            Some(Type::List(Box::new(prefer_concrete_heap_ty(
                e1.as_ref().clone(),
                e2.as_ref().clone(),
            ))))
        }
        (Type::Set(e1), Type::Set(e2)) if kind == JoinAbiKind::Heap => {
            Some(Type::Set(Box::new(prefer_concrete_heap_ty(
                e1.as_ref().clone(),
                e2.as_ref().clone(),
            ))))
        }
        (Type::Task(e1), Type::Task(e2)) if kind == JoinAbiKind::Heap => {
            Some(Type::Task(Box::new(prefer_concrete_heap_ty(
                e1.as_ref().clone(),
                e2.as_ref().clone(),
            ))))
        }
        (Type::Channel(e1), Type::Channel(e2)) if kind == JoinAbiKind::Heap => {
            Some(Type::Channel(Box::new(prefer_concrete_heap_ty(
                e1.as_ref().clone(),
                e2.as_ref().clone(),
            ))))
        }
        (Type::Map(k1, v1), Type::Map(k2, v2)) if kind == JoinAbiKind::Heap => Some(Type::Map(
            Box::new(prefer_concrete_heap_ty(k1.as_ref().clone(), k2.as_ref().clone())),
            Box::new(prefer_concrete_heap_ty(v1.as_ref().clone(), v2.as_ref().clone())),
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
                Box::new(prefer_concrete_heap_ty(r1.as_ref().clone(), r2.as_ref().clone())),
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
        let a = Type::List(Box::new(Type::Int));
        let b = Type::List(Box::new(Type::Float));
        assert_eq!(
            join_abi_tys(&a, &b, JoinAbiKind::Heap),
            Some(Type::List(Box::new(Type::Float)))
        );
    }

    #[test]
    fn join_value_keeps_fun_over_string() {
        let f = Type::Fun(vec![Type::Int], Box::new(Type::Int), Effect::pure());
        assert_eq!(
            join_abi_tys(&f, &Type::String, JoinAbiKind::Value),
            Some(f.clone())
        );
    }

    #[test]
    fn prefer_list_int_placeholder_yields_to_map() {
        let list = Type::List(Box::new(Type::Int));
        let map = Type::Map(Box::new(Type::Int), Box::new(Type::Float));
        assert_eq!(prefer_concrete_heap_ty(list, map.clone()), map);
    }
}
