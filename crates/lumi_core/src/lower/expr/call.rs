//! Calls, builtins, ADT construction, and arithmetic ops.

use super::super::ctx::CoreLowerCtx;
use super::lower_expr;
use crate::ir::{AdtRepr, ListRepr, Local, MapRepr, Op, SetRepr, Value};
use lumi_hir::{Builtin, Expr as HirExpr};

pub(super) fn lower_call_like(
    ctx: &mut CoreLowerCtx,
    expr: &HirExpr,
    ops: &mut Vec<Op>,
    pure_region: bool,
) -> Option<Local> {
    match expr {
        HirExpr::Binary {
            op, left, right, ..
        } => {
            let l = lower_expr(ctx, left, ops, pure_region)
                .expect("ICE: binary operand lowered to Unit; type checker should reject");
            let r = lower_expr(ctx, right, ops, pure_region)
                .expect("ICE: binary operand lowered to Unit; type checker should reject");
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Binary {
                    op: *op,
                    left: l,
                    right: r,
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Unary { op, expr, .. } => {
            let o = lower_expr(ctx, expr, ops, pure_region)
                .expect("ICE: unary operand lowered to Unit; type checker should reject");
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Unary {
                    op: *op,
                    operand: o,
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Call { callee, args, .. } => {
            let mut arg_locals = vec![];
            for a in args {
                if let Some(l) = lower_expr(ctx, a, ops, pure_region) {
                    arg_locals.push(l);
                }
            }
            let dest = ctx.fresh();
            let fun_name = match callee.as_ref() {
                HirExpr::Var(n, _) => Some(n.as_str()),
                _ => None,
            };
            let (value, call_pure) = match fun_name {
                Some("listOf") => (
                    Value::AllocList {
                        elems: arg_locals,
                        repr: ListRepr::HeapList,
                    },
                    pure_region,
                ),
                Some("setOf") => (
                    Value::AllocSet {
                        elems: arg_locals,
                        repr: SetRepr::HeapSet,
                    },
                    pure_region,
                ),
                Some("mapOf") => (
                    Value::AllocMap {
                        flat_pairs: arg_locals,
                        repr: MapRepr::HashOrdered,
                    },
                    pure_region,
                ),
                Some(n) if ctx.toplevel_funs.contains(n) || ctx.trait_method_names.contains(n) => {
                    let io = ctx.io_funs.contains(n);
                    (
                        Value::Call {
                            fun: n.to_string(),
                            args: arg_locals,
                        },
                        pure_region && !io,
                    )
                }
                _ => {
                    // Local / expression callee → indirect call (first-class fn).
                    let cal = lower_expr(ctx, callee, ops, pure_region).unwrap_or_else(|| {
                        let l = ctx.fresh();
                        ops.push(Op::Let {
                            local: l,
                            value: Value::Int(0),
                            pure_region,
                        });
                        l
                    });
                    (
                        Value::IndirectCall {
                            callee: cal,
                            args: arg_locals,
                        },
                        // May invoke an IO Fun; region marker must not claim purity.
                        false,
                    )
                }
            };
            ops.push(Op::Let {
                local: dest,
                value,
                pure_region: call_pure,
            });
            Some(dest)
        }
        HirExpr::BuiltinCall { name, args, .. } => {
            let mut arg_locals = vec![];
            // Product field checks carry an expected-ADT name as a 3rd HIR arg;
            // Core/runtime only need (obj, index).
            let use_args: &[HirExpr] = if matches!(name, Builtin::AdtField) && args.len() == 3 {
                &args[..2]
            } else {
                args
            };
            for a in use_args {
                if let Some(l) = lower_expr(ctx, a, ops, true) {
                    arg_locals.push(l);
                }
            }
            let is_io = name.is_io();
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Builtin {
                    name: *name,
                    args: arg_locals,
                },
                pure_region: !is_io,
            });
            Some(dest)
        }
        HirExpr::AdtNew {
            adt_name,
            tag,
            args,
            ..
        } => {
            let mut fields = vec![];
            for a in args {
                if let Some(l) = lower_expr(ctx, a, ops, pure_region) {
                    fields.push(l);
                }
            }
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::AllocAdt {
                    adt_name: adt_name.clone(),
                    tag: *tag,
                    fields,
                    repr: AdtRepr::HeapAdt,
                },
                pure_region,
            });
            Some(dest)
        }
        _ => unreachable!("lower_call_like: unexpected expr"),
    }
}
