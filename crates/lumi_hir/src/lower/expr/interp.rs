//! String interpolation lowering.

use super::super::ctx::LowerCtx;
use super::lower_expr;
use crate::ast::{Builtin, Expr};
use lumi_syntax::Span;

pub(super) fn lower_interp(ctx: &LowerCtx, parts: &[lumi_syntax::InterpPart], span: Span) -> Expr {
    let mut pieces: Vec<Expr> = Vec::new();
    for p in parts {
        match p {
            lumi_syntax::InterpPart::Lit(s) => {
                pieces.push(Expr::String(s.clone(), span));
            }
            lumi_syntax::InterpPart::Expr(e) => {
                pieces.push(Expr::BuiltinCall {
                    name: Builtin::Show,
                    args: vec![lower_expr(ctx, e)],
                    span,
                });
            }
        }
    }
    if pieces.is_empty() {
        return Expr::String(String::new(), span);
    }
    let mut acc = pieces.remove(0);
    for p in pieces {
        acc = Expr::BuiltinCall {
            name: Builtin::ListConcat,
            args: vec![acc, p],
            span,
        };
    }
    acc
}
