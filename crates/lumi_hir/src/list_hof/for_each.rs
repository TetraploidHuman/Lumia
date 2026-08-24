//! List `forEach` desugaring (side-effect iteration, returns Unit).

use super::filter::apply_pred;
use super::{bind_fun, with_fun_bind};
use crate::ast::Expr;
use crate::lower::{for_each_elem, LowerCtx};
use lumi_syntax::Span;

/// `xs.forEach(f)` → `for x in xs { f(x) }; ()`
pub(crate) fn lower_list_for_each(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    let x = format!("__fe_x_{}", span.start.0);
    let (f_bind, pred_f) = bind_fun(f, span);
    let step = apply_pred(&pred_f, Expr::Var(x.clone(), span), span);
    with_fun_bind(
        f_bind,
        Expr::Seq {
            stmts: vec![for_each_elem(ctx, &x, list, step, span), Expr::Unit(span)],
            span,
        },
    )
}
