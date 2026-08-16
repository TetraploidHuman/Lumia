//! Control flow: if / loop / break / continue / return.

use super::super::ctx::CoreLowerCtx;
use super::{lower_expr, lower_expr_block};
use crate::ir::{Block, Local, Op, Value};
use lumia_hir::Expr as HirExpr;

pub(super) fn lower_control(
    ctx: &mut CoreLowerCtx,
    expr: &HirExpr,
    ops: &mut Vec<Op>,
    pure_region: bool,
) -> Option<Local> {
    match expr {
        HirExpr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let Some(c) = lower_expr(ctx, cond, ops, pure_region) else {
                ctx.note_ice(
                    "ICE: if condition lowered to Unit; type checker should reject",
                );
                return None;
            };
            // Isolate arm bindings so `val`/`var` inside then/else cannot leak.
            let saved = ctx.save_bindings();
            let (then_block, _) = lower_expr_block(ctx, then_branch);
            ctx.restore_bindings(saved.clone());
            let (else_block, _) = lower_expr_block(ctx, else_branch);
            ctx.restore_bindings(saved);
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::If {
                    cond: c,
                    then_block: Box::new(then_block),
                    else_block: Box::new(else_block),
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Loop {
            cond, body, step, ..
        } => {
            // Loop header/body/latch share outer bindings but must not leak
            // names introduced only inside those blocks.
            let saved = ctx.save_bindings();
            let (header, _) = lower_expr_block(ctx, cond);
            ctx.restore_bindings(saved.clone());
            let (body_block, _) = lower_expr_block(ctx, body);
            ctx.restore_bindings(saved.clone());
            let latch = if let Some(s) = step {
                let (b, _) = lower_expr_block(ctx, s);
                b
            } else {
                Block {
                    ops: vec![],
                    result: None,
                }
            };
            ctx.restore_bindings(saved);
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Loop {
                    header: Box::new(header),
                    body: Box::new(body_block),
                    latch: Box::new(latch),
                },
                pure_region: false,
            });
            Some(dest)
        }
        HirExpr::Break(_) => {
            ops.push(Op::Break);
            None
        }
        HirExpr::Continue(_) => {
            ops.push(Op::Continue);
            None
        }
        HirExpr::Return { value, .. } => {
            if let Some(v) = lower_expr(ctx, value, ops, pure_region) {
                ops.push(Op::Return { value: v });
            }
            None
        }
        HirExpr::Alt { span, .. } => {
            ctx.note_ice(format!(
                "ICE: Alt reached Core lower at {span:?}; expected typecheck desugar"
            ));
            None
        }
        HirExpr::With { span, .. } => {
            ctx.note_ice(format!(
                "ICE: With reached Core lower at {span:?}; expected typecheck rewrite"
            ));
            None
        }
        _ => unreachable!("lower_control: unexpected expr"),
    }
}
