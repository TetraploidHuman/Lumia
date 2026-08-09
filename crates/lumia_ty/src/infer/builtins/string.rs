//! BuiltinCall typing — string family.

use super::super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(crate) fn infer_string_builtin(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::StrTrim | Builtin::StrToLower | Builtin::StrToUpper => {
                if args.len() != 1 {
                    return Err(at(span, format!("{name:?} takes 1 argument")));
                }
                let (st, se) = self.infer_expr(&args[0])?;
                self.unify_at(span, st, Type::String)?;
                Ok((Type::String, se))
            }
            Builtin::StrSplit => {
                if args.len() != 2 {
                    return Err(at(span, "split takes 2 arguments"));
                }
                let (st, se) = self.infer_expr(&args[0])?;
                let (ct, ce) = self.infer_expr(&args[1])?;
                self.unify_at(span, st, Type::String)?;
                self.unify_at(span, ct, Type::Char)?;
                Ok((Type::List(Box::new(Type::String)), self.union_eff(se, ce)))
            }
            Builtin::StrSubstring => {
                if args.len() != 3 {
                    return Err(at(span, "substring takes 3 arguments (string, start, end)"));
                }
                let (st, se) = self.infer_expr(&args[0])?;
                let (a, ae) = self.infer_expr(&args[1])?;
                let (b, be) = self.infer_expr(&args[2])?;
                self.unify_at(span, st, Type::String)?;
                self.unify_at(span, a, Type::Int)?;
                self.unify_at(span, b, Type::Int)?;
                Ok((Type::String, self.union3_eff(se, ae, be)))
            }
            Builtin::StrStartsWith | Builtin::StrEndsWith => {
                if args.len() != 2 {
                    return Err(at(span, "startsWith/endsWith takes 2 arguments"));
                }
                let (st, se) = self.infer_expr(&args[0])?;
                let (pt, pe) = self.infer_expr(&args[1])?;
                self.unify_at(span, st, Type::String)?;
                self.unify_at(span, pt, Type::String)?;
                Ok((Type::Bool, self.union_eff(se, pe)))
            }
            _ => unreachable!("non-string builtin dispatched to infer_string_builtin"),
        }
    }
}
