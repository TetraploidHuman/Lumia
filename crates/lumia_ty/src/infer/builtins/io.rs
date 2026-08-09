//! BuiltinCall typing — io family.

use super::super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(crate) fn infer_io_builtin(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr => {
                if args.len() != 1 {
                    return Err(at(span, "println takes 1 argument"));
                }
                let (t, e) = self.infer_expr(&args[0])?;
                let t = self.prune(t);
                match t {
                    Type::Int
                    | Type::String
                    | Type::Bool
                    | Type::Float
                    | Type::Char
                    | Type::Adt { .. }
                    | Type::List(_)
                    | Type::Map(_, _)
                    | Type::Set(_)
                    | Type::Tuple(_) => {}
                    Type::Var(_) => {
                        // Leave open: freezing to Int rejected `f(1.5)` for
                        // `{ x -> println(x); x }` and poisoned later Float uses.
                        // Unresolved vars still print via `println_auto` (Int default).
                    }
                    other => {
                        return Err(at(span, format!("println: unsupported type {other:?}")));
                    }
                }
                Ok((Type::Unit, self.union_eff(Effect::io(), e)))
            }
            Builtin::Show => {
                if args.len() != 1 {
                    return Err(at(span, "show takes 1 argument"));
                }
                let (_, e) = self.infer_expr(&args[0])?;
                Ok((Type::String, e))
            }
            Builtin::ReadStdin => {
                if !args.is_empty() {
                    return Err(at(span, "readStdin takes 0 arguments"));
                }
                Ok((Type::String, Effect::io()))
            }
            Builtin::MatchFail => {
                if !args.is_empty() {
                    return Err(at(span, "match fail takes 0 arguments"));
                }
                // Diverges; fresh var unifies with any arm result type.
                Ok((self.fresh(), Effect::pure()))
            }
            Builtin::Assert => {
                if args.len() != 1 {
                    return Err(at(span, "assert takes 1 argument"));
                }
                let (ct, ce) = self.infer_expr(&args[0])?;
                self.unify_at(span, ct, Type::Bool)?;
                Ok((Type::Unit, ce))
            }
            _ => unreachable!("non-io builtin dispatched to infer_io_builtin"),
        }
    }
}
