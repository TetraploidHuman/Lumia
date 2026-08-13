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
                let elem = self.expect_list_elem(lt, span, "slice/drop")?;
                Ok((Type::List(Box::new(elem)), self.union_eff(le, ie)))
            }
            Builtin::ListTake => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (it, ie) = self.infer_expr(&args[1])?;
                self.unify_at(span, it, Type::Int)?;
                let elem = self.expect_list_elem(lt, span, "take")?;
                Ok((Type::List(Box::new(elem)), self.union_eff(le, ie)))
            }
            Builtin::ListReverse => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let elem = self.expect_list_elem(lt, span, "reverse")?;
                Ok((Type::List(Box::new(elem)), le))
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
