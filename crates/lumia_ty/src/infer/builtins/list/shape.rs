//! List shape ops: slice/take/reverse/sort/join.

use super::super::super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(super) fn infer_list_shape(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::ListSlice => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (it, ie) = self.infer_expr(&args[1])?;
                self.unify_at(span, it, Type::Int)?;
                match self.prune(lt.clone()) {
                    Type::String => Ok((Type::String, self.union_eff(le, ie))),
                    Type::List(t) => Ok((Type::List(t), self.union_eff(le, ie))),
                    Type::Var(v) => {
                        self.uni.take_vars.insert(v);
                        Ok((lt, self.union_eff(le, ie)))
                    }
                    other => Err(at(
                        span,
                        format!("slice/drop: expected List or String, got {other:?}"),
                    )),
                }
            }
            Builtin::ListTake => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (it, ie) = self.infer_expr(&args[1])?;
                self.unify_at(span, it, Type::Int)?;
                match self.prune(lt.clone()) {
                    Type::String => Ok((Type::String, self.union_eff(le, ie))),
                    Type::List(t) => Ok((Type::List(t), self.union_eff(le, ie))),
                    Type::Var(v) => {
                        self.uni.take_vars.insert(v);
                        Ok((lt, self.union_eff(le, ie)))
                    }
                    other => Err(at(
                        span,
                        format!("take: expected List or String, got {other:?}"),
                    )),
                }
            }
            Builtin::ListReverse => {
                let (lt, le) = self.infer_expr(&args[0])?;
                match self.prune(lt.clone()) {
                    Type::String => Ok((Type::String, le)),
                    Type::List(t) => Ok((Type::List(t), le)),
                    Type::Var(v) => {
                        self.uni.take_vars.insert(v);
                        Ok((lt, le))
                    }
                    other => Err(at(
                        span,
                        format!("reverse: expected List or String, got {other:?}"),
                    )),
                }
            }
            Builtin::ListSort => {
                let (lt, le) = self.infer_expr(&args[0])?;
                match self.prune(lt.clone()) {
                    Type::List(t) => {
                        self.unify_at(span, *t, Type::Int)?;
                    }
                    Type::Var(_) => {
                        self.unify_at(span, lt, Type::List(Box::new(Type::Int)))?;
                    }
                    other => {
                        return Err(at(span, format!("sort: expected List[Int], got {other:?}")));
                    }
                }
                Ok((Type::List(Box::new(Type::Int)), le))
            }
            Builtin::ListJoin => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (st, se) = self.infer_expr(&args[1])?;
                self.unify_at(span, st, Type::String)?;
                match self.prune(lt.clone()) {
                    Type::List(t) => self.unify_at(span, *t, Type::String)?,
                    Type::Var(_) => {
                        self.unify_at(span, lt, Type::List(Box::new(Type::String)))?;
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("join: expected List[String], got {other:?}"),
                        ));
                    }
                }
                Ok((Type::String, self.union_eff(le, se)))
            }
            _ => unreachable!("infer_list_shape"),
        }
    }
}
