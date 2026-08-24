//! BuiltinCall typing — list family.

mod build;
mod hof;
mod poly;
mod shape;

use super::super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumi_hir::{Builtin, Expr};

impl Infer {
    /// Expect `ty` to be `List[elem]`, or constrain a Var to a fresh List. Returns elem.
    pub(super) fn expect_list_elem(
        &mut self,
        ty: Type,
        span: lumi_syntax::Span,
        op: &str,
    ) -> Result<Type, TypeError> {
        match self.prune(ty.clone()) {
            Type::List(t) => Ok(*t),
            Type::Var(_) => {
                let elem = self.fresh();
                self.unify_at(span, ty, Type::List(Box::new(elem.clone())))?;
                Ok(elem)
            }
            other => Err(at(span, format!("{op}: expected List, got {other:?}"))),
        }
    }

    pub(super) fn callback_effect(&mut self, ft: Type) -> Effect {
        match self.prune(ft) {
            Type::Fun(_, _, e) => self.prune_eff(e),
            _ => Effect::pure(),
        }
    }

    pub(crate) fn infer_list_builtin(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumi_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::ListLen | Builtin::ListGet | Builtin::Elems => {
                self.infer_list_poly(name, args, span)
            }
            Builtin::ListSlice
            | Builtin::ListTake
            | Builtin::ListReverse
            | Builtin::ListSort
            | Builtin::ListJoin => self.infer_list_shape(name, args, span),
            Builtin::ListSortByKeys | Builtin::ListParMap | Builtin::ListParFold => {
                self.infer_list_hof(name, args, span)
            }
            Builtin::ListAppend
            | Builtin::ListConcat
            | Builtin::Range
            | Builtin::RangeInclusive => self.infer_list_build(name, args, span),
            _ => unreachable!("non-list builtin dispatched to infer_list_builtin"),
        }
    }
}
