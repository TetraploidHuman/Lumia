//! List build: append/concat/range.

use super::super::super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumi_hir::{Builtin, Expr};

impl Infer {
    pub(super) fn infer_list_build(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumi_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::ListAppend => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (et, ee) = self.infer_expr(&args[1])?;
                let list_ty = match self.prune(lt.clone()) {
                    Type::List(t) => {
                        self.unify_at(span, et, *t.clone())?;
                        Type::List(t)
                    }
                    Type::Var(_) => {
                        self.unify_at(span, lt, Type::List(Box::new(et.clone())))?;
                        Type::List(Box::new(et))
                    }
                    other => {
                        return Err(at(span, format!("append: expected List, got {other:?}")));
                    }
                };
                Ok((list_ty, self.union_eff(le, ee)))
            }
            Builtin::ListConcat => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (rt, re) = self.infer_expr(&args[1])?;
                let lt = self.prune(lt);
                let rt = self.prune(rt);
                match (&lt, &rt) {
                    (Type::String, Type::String)
                    | (Type::String, Type::Var(_))
                    | (Type::Var(_), Type::String) => {
                        self.unify_at(span, lt, Type::String)?;
                        self.unify_at(span, rt, Type::String)?;
                        Ok((Type::String, self.union_eff(le, re)))
                    }
                    _ => {
                        let list_ty = match (lt.clone(), rt.clone()) {
                            (Type::List(a), Type::List(b)) => {
                                self.unify_at(span, *a.clone(), *b)?;
                                Type::List(a)
                            }
                            (Type::List(a), Type::Var(_)) => {
                                self.unify_at(span, rt, Type::List(a.clone()))?;
                                Type::List(a)
                            }
                            (Type::Var(_), Type::List(b)) => {
                                self.unify_at(span, lt, Type::List(b.clone()))?;
                                Type::List(b)
                            }
                            (Type::Var(_), Type::Var(_)) => {
                                let elem = self.fresh();
                                let list = Type::List(Box::new(elem));
                                self.unify_at(span, lt, list.clone())?;
                                self.unify_at(span, rt, list.clone())?;
                                list
                            }
                            (other, _) => {
                                return Err(at(
                                    span,
                                    format!("concat: expected List or String, got {other:?}"),
                                ));
                            }
                        };
                        Ok((list_ty, self.union_eff(le, re)))
                    }
                }
            }
            Builtin::Range | Builtin::RangeInclusive => {
                let mut eff = Effect::pure();
                for a in args {
                    let (t, e) = self.infer_expr(a)?;
                    self.unify_at(span, t, Type::Int)?;
                    eff = self.union_eff(eff, e);
                }
                Ok((Type::List(Box::new(Type::Int)), eff))
            }
            _ => unreachable!("infer_list_build"),
        }
    }
}
