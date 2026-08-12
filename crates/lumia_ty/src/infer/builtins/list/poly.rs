//! ListLen / ListGet / Elems (polymorphic over List/Set/Map/String).

use super::super::super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(super) fn infer_list_poly(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::ListLen => {
                let (t, e) = self.infer_expr(&args[0])?;
                let t = self.prune(t);
                match t {
                    Type::List(_) | Type::Set(_) | Type::Map(_, _) | Type::String => {}
                    Type::Var(_) => {
                        let elem = self.fresh();
                        self.unify_at(span, t, Type::List(Box::new(elem)))?;
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("len: expected List/Set/Map/String, got {other:?}"),
                        ));
                    }
                }
                Ok((Type::Int, e))
            }
            Builtin::ListGet => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (it, ie) = self.infer_expr(&args[1])?;
                let lt_p = self.prune(lt);
                let elem = match lt_p {
                    Type::List(t) => {
                        self.unify_at(span, it, Type::Int)?;
                        *t
                    }
                    Type::Set(t) => {
                        self.unify_at(span, it, Type::Int)?;
                        *t
                    }
                    Type::Map(k, v) => {
                        self.unify_at(span, it, *k)?;
                        Type::Adt {
                            name: "Option".into(),
                            params: vec![*v],
                        }
                    }
                    Type::Var(v) => {
                        self.unify_at(span, it, Type::Int)?;
                        let elem = self.fresh();
                        self.unify_at(span, Type::Var(v), Type::List(Box::new(elem.clone())))?;
                        elem
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("get: expected List, Set, or Map, got {other:?}"),
                        ));
                    }
                };
                Ok((elem, self.union_eff(le, ie)))
            }
            Builtin::Elems => {
                let (ct, ce) = self.infer_expr(&args[0])?;
                let list_ty = match self.prune(ct.clone()) {
                    Type::List(e) => Type::List(e),
                    Type::Set(e) => Type::List(e),
                    Type::Map(k, _) => Type::List(k),
                    Type::Var(_) => {
                        let e = self.fresh();
                        self.unify_at(span, ct, Type::List(Box::new(e.clone())))?;
                        Type::List(Box::new(e))
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("elems: expected List, Set, or Map, got {other:?}"),
                        ));
                    }
                };
                Ok((list_ty, ce))
            }
            _ => unreachable!("infer_list_poly"),
        }
    }
}
