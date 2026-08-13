//! HIR expression → Core ops.

mod bind;
mod call;
mod control;
mod lit;

use super::ctx::CoreLowerCtx;
use crate::ir::{Block, Local, Op};
use lumia_hir::Expr as HirExpr;

pub(super) fn lower_expr_block(ctx: &mut CoreLowerCtx, expr: &HirExpr) -> (Block, Option<Local>) {
    let mut ops = vec![];
    let result = lower_expr(ctx, expr, &mut ops, true);
    (
        Block {
            params: vec![],
            ops,
            result,
        },
        result,
    )
}

pub(super) fn lower_expr(
    ctx: &mut CoreLowerCtx,
    expr: &HirExpr,
    ops: &mut Vec<Op>,
    pure_region: bool,
) -> Option<Local> {
    match expr {
        HirExpr::Int(..)
        | HirExpr::Float(..)
        | HirExpr::Bool(..)
        | HirExpr::String(..)
        | HirExpr::Char(..)
        | HirExpr::Unit(_)
        | HirExpr::Var(..) => lit::lower_lit(ctx, expr, ops, pure_region),

        HirExpr::Let { .. }
        | HirExpr::Assign { .. }
        | HirExpr::Seq { .. }
        | HirExpr::Lambda { .. } => bind::lower_bind(ctx, expr, ops, pure_region),

        HirExpr::Binary { .. }
        | HirExpr::Unary { .. }
        | HirExpr::Call { .. }
        | HirExpr::BuiltinCall { .. }
        | HirExpr::AdtNew { .. } => call::lower_call_like(ctx, expr, ops, pure_region),

        HirExpr::If { .. }
        | HirExpr::Loop { .. }
        | HirExpr::Break(_)
        | HirExpr::Continue(_)
        | HirExpr::Return { .. }
        | HirExpr::Alt { .. }
        | HirExpr::With { .. } => control::lower_control(ctx, expr, ops, pure_region),
    }
}
