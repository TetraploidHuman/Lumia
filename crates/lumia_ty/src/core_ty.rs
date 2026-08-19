//! Closed interned ABI lattice ([`CoreTy`]) for mid-end / codegen.
//!
//! Inference keeps open [`Type`] (`Var`, `TuplePrefix`, `Effect::Var`). At the
//! HIR→Core boundary those are closed: remaining holes are [`Type::Unknown`],
//! never a fake `Int` or `Var(u32::MAX)`. Compound nodes share [`Arc`] children
//! so cloning a Core type is pointer bumps, not a deep tree copy.

use super::types::{Effect, Type};
use lumia_syntax::Sym;
use std::ops::Deref;
use std::sync::Arc;

/// Closed ABI type: a zonked interned [`Type`] with no open vars.
///
/// Invariants (debug-checked by [`Self::from_open`]):
/// - no [`Type::Var`]
/// - no [`Type::TuplePrefix`] (frozen to [`Type::Tuple`])
/// - no [`Effect::Var`] (zonked to [`Effect::Pure`])
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CoreTy(Type);

impl CoreTy {
    #[inline]
    pub fn unknown() -> Self {
        Self(Type::Unknown)
    }

    #[inline]
    pub fn int() -> Self {
        Self(Type::Int)
    }

    #[inline]
    pub fn float() -> Self {
        Self(Type::Float)
    }

    #[inline]
    pub fn bool() -> Self {
        Self(Type::Bool)
    }

    #[inline]
    pub fn string() -> Self {
        Self(Type::String)
    }

    #[inline]
    pub fn char() -> Self {
        Self(Type::Char)
    }

    #[inline]
    pub fn unit() -> Self {
        Self(Type::Unit)
    }

    #[inline]
    pub fn list(elem: CoreTy) -> Self {
        Self(Type::list(elem.into_type()))
    }

    #[inline]
    pub fn set(elem: CoreTy) -> Self {
        Self(Type::set(elem.into_type()))
    }

    #[inline]
    pub fn map(k: CoreTy, v: CoreTy) -> Self {
        Self(Type::map(k.into_type(), v.into_type()))
    }

    #[inline]
    pub fn task(elem: CoreTy) -> Self {
        Self(Type::task(elem.into_type()))
    }

    #[inline]
    pub fn channel(elem: CoreTy) -> Self {
        Self(Type::channel(elem.into_type()))
    }

    #[inline]
    pub fn fun(params: Vec<CoreTy>, ret: CoreTy, eff: Effect) -> Self {
        debug_assert!(!matches!(eff, Effect::Var(_)));
        Self(Type::fun(
            params.into_iter().map(Self::into_type).collect(),
            ret.into_type(),
            eff,
        ))
    }

    #[inline]
    pub fn adt(name: Sym, params: Vec<CoreTy>) -> Self {
        Self(Type::Adt {
            name,
            params: params.into_iter().map(Self::into_type).collect(),
        })
    }

    #[inline]
    pub fn tuple(elems: Vec<CoreTy>) -> Self {
        Self(Type::Tuple(
            elems.into_iter().map(Self::into_type).collect(),
        ))
    }

    /// Close an open inference type for Core / codegen.
    pub fn from_open(ty: &Type) -> Self {
        Self(close_type(ty))
    }

    #[inline]
    pub fn as_type(&self) -> &Type {
        &self.0
    }

    #[inline]
    pub fn into_type(self) -> Type {
        self.0
    }

    /// Already-closed type (skip a second walk). Debug-asserts the invariant.
    pub fn from_closed(ty: Type) -> Self {
        debug_assert!(
            is_closed(&ty),
            "CoreTy::from_closed got an open type: {ty}"
        );
        Self(ty)
    }
}

impl Deref for CoreTy {
    type Target = Type;

    #[inline]
    fn deref(&self) -> &Type {
        &self.0
    }
}

impl From<CoreTy> for Type {
    #[inline]
    fn from(ty: CoreTy) -> Type {
        ty.0
    }
}

impl AsRef<Type> for CoreTy {
    #[inline]
    fn as_ref(&self) -> &Type {
        &self.0
    }
}

impl std::fmt::Display for CoreTy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Close `ty` for the mid-end: vars/holes → [`Type::Unknown`], prefixes freeze,
/// effect vars → pure. Shares `Arc` children when a node is already closed.
pub fn close_type(ty: &Type) -> Type {
    match ty {
        Type::Var(_) => Type::Unknown,
        Type::Unknown
        | Type::Int
        | Type::Float
        | Type::Bool
        | Type::String
        | Type::Char
        | Type::Unit => ty.clone(),
        Type::Fun(ps, r, e) => {
            let params: Vec<Type> = ps.iter().map(close_type).collect();
            let ret = close_type(r);
            let eff = match e {
                Effect::Var(_) => Effect::Pure,
                other => *other,
            };
            Type::fun(params, ret, eff)
        }
        Type::List(t) => reuse_unary(ty, t, Type::List, close_type),
        Type::Set(t) => reuse_unary(ty, t, Type::Set, close_type),
        Type::Task(t) => reuse_unary(ty, t, Type::Task, close_type),
        Type::Channel(t) => reuse_unary(ty, t, Type::Channel, close_type),
        Type::Map(k, v) => {
            let ck = close_type(k);
            let cv = close_type(v);
            if ck == **k && cv == **v {
                ty.clone()
            } else {
                Type::map(ck, cv)
            }
        }
        Type::Adt { name, params } => Type::Adt {
            name: name.clone(),
            params: params.iter().map(close_type).collect(),
        },
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(close_type).collect()),
        Type::TuplePrefix(ts) => Type::Tuple(ts.iter().map(close_type).collect()),
    }
}

fn reuse_unary(
    orig: &Type,
    child: &Arc<Type>,
    wrap: fn(Arc<Type>) -> Type,
    close: fn(&Type) -> Type,
) -> Type {
    let c = close(child);
    if c == **child {
        orig.clone()
    } else {
        wrap(Arc::new(c))
    }
}

pub fn is_closed(ty: &Type) -> bool {
    match ty {
        Type::Var(_) | Type::TuplePrefix(_) => false,
        Type::Fun(_, _, Effect::Var(_)) => false,
        Type::Unknown
        | Type::Int
        | Type::Float
        | Type::Bool
        | Type::String
        | Type::Char
        | Type::Unit => true,
        Type::Fun(ps, r, _) => ps.iter().all(is_closed) && is_closed(r),
        Type::List(t) | Type::Set(t) | Type::Task(t) | Type::Channel(t) => is_closed(t),
        Type::Map(k, v) => is_closed(k) && is_closed(v),
        Type::Adt { params, .. } | Type::Tuple(params) => params.iter().all(is_closed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_maps_var_and_max_sentinel_to_unknown() {
        assert_eq!(close_type(&Type::Var(0)), Type::Unknown);
        assert_eq!(close_type(&Type::Var(u32::MAX)), Type::Unknown);
        assert_eq!(
            close_type(&Type::list(Type::Var(1))),
            Type::list(Type::Unknown)
        );
    }

    #[test]
    fn close_freezes_tuple_prefix_and_effect_var() {
        let t = Type::TuplePrefix(vec![Type::Int, Type::Float]);
        assert_eq!(
            close_type(&t),
            Type::Tuple(vec![Type::Int, Type::Float])
        );
        let f = Type::fun(vec![Type::Int], Type::Int, Effect::Var(3));
        let closed = close_type(&f);
        match closed {
            Type::Fun(_, _, e) => assert_eq!(e, Effect::Pure),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn close_reuses_arc_when_already_closed() {
        let inner = Type::list(Type::Float);
        let closed = close_type(&inner);
        match (&inner, &closed) {
            (Type::List(a), Type::List(b)) => assert!(Arc::ptr_eq(a, b)),
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn core_ty_from_open_is_closed() {
        let ty = CoreTy::from_open(&Type::map(Type::Var(0), Type::String));
        assert!(is_closed(&ty));
        assert_eq!(&*ty, &Type::map(Type::Unknown, Type::String));
    }
}
