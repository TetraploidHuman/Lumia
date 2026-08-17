//! Let / assign / seq / lambda bindings.

use super::super::ctx::CoreLowerCtx;
use super::{lower_expr, lower_expr_block};
use crate::ir::{Local, Op, Value};
use lumia_hir::Expr as HirExpr;

pub(super) fn lower_bind(
    ctx: &mut CoreLowerCtx,
    expr: &HirExpr,
    ops: &mut Vec<Op>,
    pure_region: bool,
) -> Option<Local> {
    match expr {
        HirExpr::Let {
            name,
            value,
            body,
            mutable,
            ..
        } => {
            // Unit RHS (e.g. `scope { for … }` → Seq ending in Unit): materialize
            // a Unit local so `__scope_r_*` / other names still bind. Skipping the
            // bind left `Value::Name` unbound at codegen (`unbound mutable`).
            let v = match lower_expr(ctx, value, ops, pure_region) {
                Some(l) => l,
                None => {
                    let l = ctx.fresh();
                    ops.push(Op::Let {
                        local: l,
                        value: Value::Unit,
                        pure_region,
                    });
                    l
                }
            };
            let saved = ctx.save_bindings();
            if *mutable {
                ctx.bind_mutable(name.clone(), v);
                ops.push(Op::Assign {
                    name: name.clone(),
                    value: v,
                });
            } else {
                // `val` may shadow an outer `var` for the duration of `body`.
                ctx.mutables.remove(name);
                ctx.bind_name(name.clone(), v);
            }
            let result = lower_expr(ctx, body, ops, pure_region);
            ctx.restore_bindings(saved);
            result
        }
        HirExpr::Assign { name, value, .. } => {
            let v = match lower_expr(ctx, value, ops, pure_region) {
                Some(l) => l,
                None => {
                    // Unit RHS: materialize a 0 local so assign never panics.
                    let l = ctx.fresh();
                    ops.push(Op::Let {
                        local: l,
                        value: Value::Unit,
                        pure_region,
                    });
                    l
                }
            };
            if ctx.mutables.contains(name) {
                ops.push(Op::Assign {
                    name: name.clone(),
                    value: v,
                });
            } else {
                // Immutable binding: ty rejects user assigns; do not mutate an
                // outer `var` shadowed by `val` (and do not mark name mutable).
                ctx.bind_name(name.clone(), v);
            }
            None
        }
        HirExpr::Seq { stmts, .. } => {
            let mut last = None;
            for s in stmts {
                last = lower_expr(ctx, s, ops, pure_region);
            }
            last
        }
        HirExpr::Lambda { params, body, .. } => {
            let mut inner = CoreLowerCtx {
                next: ctx.next,
                name_to_local: ctx.name_to_local.clone(),
                mutables: ctx.mutables.clone(),
                toplevel_funs: ctx.toplevel_funs.clone(),
                toplevel_vals: ctx.toplevel_vals.clone(),
                trait_method_names: ctx.trait_method_names.clone(),
                io_funs: ctx.io_funs.clone(),
                type_at: ctx.type_at.clone(),
                assert_files: ctx.assert_files.clone(),
                ice: None,
            };
            let mut pls = vec![];
            for p in params {
                let l = inner.fresh();
                inner.bind_name(p.clone(), l);
                pls.push(l);
            }
            let (block, _) = lower_expr_block(&mut inner, body);
            if let Some(msg) = inner.ice.take() {
                ctx.note_ice(msg);
            }
            ctx.next = inner.next;
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Lambda {
                    params: pls,
                    body: Box::new(block),
                },
                pure_region,
            });
            Some(dest)
        }
        _ => unreachable!("lower_bind: non-binding"),
    }
}

#[cfg(test)]
mod tests {
    use crate::compile_source_to_core;
    use crate::ir::Value;
    use crate::visit::for_each_block_dfs;

    #[test]
    fn scope_for_last_stmt_binds_unit_not_unbound_name() {
        // `scope { for … }` lowers to `let __scope_r = Seq(…, Unit)`. Skipping
        // Unit Let left `Name(__scope_r_*)` and codegen failed `unbound mutable`.
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    scope {
        for x in listOf(1, 2) { println(x) }
    }
}
"#,
        )
        .expect("core");
        let mut unbound_scope_r = false;
        let mut unit_lets = 0usize;
        for fun in &core.functions {
            if fun.name != "main" {
                continue;
            }
            for_each_block_dfs(&fun.body, &mut |b| {
                for op in &b.ops {
                    if let crate::ir::Op::Let { value, .. } = op {
                        if matches!(value, Value::Unit) {
                            unit_lets += 1;
                        }
                        if let Value::Name(n) = value {
                            if n.starts_with("__scope_r_") {
                                unbound_scope_r = true;
                            }
                        }
                    }
                }
            });
        }
        assert!(!unbound_scope_r, "scope result still referenced as unbound Name");
        assert!(unit_lets >= 1, "expected Unit materialization for scope result");
    }
}
