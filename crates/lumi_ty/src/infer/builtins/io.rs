//! BuiltinCall typing — io family.

use super::super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumi_hir::{Builtin, Expr};

impl Infer {
    pub(crate) fn infer_io_builtin(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumi_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        // Arity checked in `infer_builtin_call`, except Assert: info allows 1..=2
        // (message injected later) but typing still requires a single Bool arg.
        match name {
            Builtin::Println => {
                let (t, e) = self.infer_expr(&args[0])?;
                let t = self.prune(t);
                match t {
                    Type::Int
                    | Type::String
                    | Type::Bool
                    | Type::Float
                    | Type::Char
                    | Type::Unit
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
                let (_, e) = self.infer_expr(&args[0])?;
                Ok((Type::String, e))
            }
            Builtin::ReadStdin => Ok((Type::String, Effect::io())),
            Builtin::MatchFail => {
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
