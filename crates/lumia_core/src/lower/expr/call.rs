//! Calls, builtins, ADT construction, and arithmetic ops.

use super::super::ctx::CoreLowerCtx;
use super::lower_expr;
use crate::ir::{AdtRepr, ListRepr, Local, MapRepr, Op, SetRepr, Value};
use lumia_hir::{Builtin, Expr as HirExpr};
use lumia_ty::{expr_span, Type};

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
            let Some(l) = lower_expr(ctx, left, ops, pure_region) else {
                ctx.note_ice("ICE: binary operand lowered to Unit; type checker should reject");
                return None;
            };
            let Some(r) = lower_expr(ctx, right, ops, pure_region) else {
                ctx.note_ice("ICE: binary operand lowered to Unit; type checker should reject");
                return None;
            };
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Binary {
                    op: (*op).into(),
                    left: l,
                    right: r,
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Unary { op, expr, .. } => {
            let Some(o) = lower_expr(ctx, expr, ops, pure_region) else {
                ctx.note_ice("ICE: unary operand lowered to Unit; type checker should reject");
                return None;
            };
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Unary {
                    op: (*op).into(),
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
                            fun: n.into(),
                            args: arg_locals,
                        },
                        pure_region && !io,
                    )
                }
                _ => {
                    // Local / expression callee → indirect call (first-class fn).
                    let Some(cal) = lower_expr(ctx, callee, ops, pure_region) else {
                        ctx.note_ice(
                            "ICE: failed to lower call callee (would have poisoned with Int(0))",
                        );
                        return None;
                    };
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
        HirExpr::BuiltinCall { name, args, span } => {
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
            // Bare `assert(cond)` → inject `path:line` message at Core (typed HIR stays 1-arg).
            if matches!(name, Builtin::Assert) && arg_locals.len() == 1 {
                if let Some(msg) = ctx.assert_fail_message(*span) {
                    let msg_local = ctx.fresh();
                    ops.push(Op::Let {
                        local: msg_local,
                        value: Value::String(msg),
                        pure_region: true,
                    });
                    arg_locals.push(msg_local);
                }
            }
            let is_io = name.is_io();
            let dest = ctx.fresh();
            let result_ty = stamp_builtin_result_ty(ctx, *name, expr);
            ops.push(Op::Let {
                local: dest,
                value: Value::Builtin {
                    name: *name,
                    args: arg_locals,
                    result_ty,
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
                    adt_name: adt_name.to_string(),
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

/// Stamp ground builtin results from HIR typecheck (`type_at`).
fn stamp_builtin_result_ty(ctx: &CoreLowerCtx, name: Builtin, expr: &HirExpr) -> Option<Type> {
    match name {
        // Channel[T] from send/recv typing — avoid erased Int elem.
        Builtin::ChannelNew => {
            let ty = ctx.type_of_span(expr_span(expr))?;
            match ty {
                Type::Channel(ref e) if type_is_ground(e) => Some(ty),
                _ => None,
            }
        }
        // Payload of recv / join — lift heap lattice uses `type_may_heap` on the stamp
        // (same Typed path as codegen roots) instead of hardcoding non-heap.
        Builtin::ChannelRecv | Builtin::TaskJoin => {
            let ty = ctx.type_of_span(expr_span(expr))?;
            type_is_ground(&ty).then_some(ty)
        }
        // Match `Ok`/`Err`/`Some`: derive from receiver + ctor hint (not the
        // AdtField expr's type_at — those spans collide with arm bodies).
        Builtin::AdtField => stamp_adt_field_result_ty(ctx, expr),
        _ => None,
    }
}

fn stamp_adt_field_result_ty(ctx: &CoreLowerCtx, expr: &HirExpr) -> Option<Type> {
    let HirExpr::BuiltinCall {
        name: Builtin::AdtField,
        args,
        ..
    } = expr
    else {
        return None;
    };
    if args.len() != 3 {
        return None;
    }
    let HirExpr::Int(idx, _) = &args[1] else {
        return None;
    };
    if *idx < 0 {
        return None;
    }
    let idx = *idx as usize;
    let HirExpr::String(ctor, _) = &args[2] else {
        return None;
    };
    let recv = ctx.type_of_span(expr_span(&args[0]))?;
    let Type::Adt { name, params } = &recv else {
        return None;
    };
    let ty = if name == lumia_hir::RESULT.name && (ctor == "Ok" || ctor == "Err") {
        if idx != 0 {
            return None;
        }
        let pi = if ctor == "Ok" { 0 } else { 1 };
        params.get(pi).cloned()?
    } else if name == lumia_hir::OPTION.name && ctor == "Some" {
        if idx != 0 {
            return None;
        }
        params.first().cloned()?
    } else if name.as_str() == ctor.as_str() {
        params.get(idx).cloned()?
    } else {
        params.get(idx).cloned()?
    };
    match &ty {
        Type::Unit | Type::Var(_) => None,
        t if type_is_ground(t) => Some(ty),
        _ => None,
    }
}

fn type_is_ground(t: &Type) -> bool {
    match t {
        Type::Var(_) => false,
        Type::Fun(ps, r, _) => ps.iter().all(type_is_ground) && type_is_ground(r),
        Type::List(e) | Type::Set(e) | Type::Task(e) | Type::Channel(e) => type_is_ground(e),
        Type::Map(k, v) => type_is_ground(k) && type_is_ground(v),
        Type::Tuple(ts) | Type::TuplePrefix(ts) | Type::Adt { params: ts, .. } => {
            ts.iter().all(type_is_ground)
        }
        _ => true,
    }
}

#[cfg(test)]
mod stamp_tests {
    use crate::compile_source_to_core;
    use crate::ir::{Op, Value};
    use crate::visit::for_each_block_dfs;
    use lumia_hir::Builtin;
    use lumia_ty::Type;

    #[test]
    fn adt_field_err_string_stamped_when_ok_is_float() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
  val r = if false { Ok(1.5) } else { Err("e") }
  r match {
    Ok(x) -> println(x)
    Err(s) -> println(s)
  }
}
"#,
        )
        .expect("core");
        let mut found_string = false;
        let mut found_float = false;
        for fun in &core.functions {
            for_each_block_dfs(&fun.body, &mut |b| {
                for op in &b.ops {
                    if let Op::Let {
                        value:
                            Value::Builtin {
                                name: Builtin::AdtField,
                                result_ty: Some(ty),
                                ..
                            },
                        ..
                    } = op
                    {
                        found_string |= matches!(ty, Type::String);
                        found_float |= matches!(ty, Type::Float);
                    }
                }
            });
        }
        assert!(
            found_string && found_float,
            "expected Ok→Float and Err→String AdtField stamps"
        );
    }

    #[test]
    fn channel_recv_list_payload_stamped_ground() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
  val ch = channel(1)
  spawn { ch.send(listOf(1, 2)) }
  println(ch.recv())
}
"#,
        )
        .expect("core");
        let mut found = false;
        for fun in &core.functions {
            for_each_block_dfs(&fun.body, &mut |b| {
                for op in &b.ops {
                    if let Op::Let {
                        value:
                            Value::Builtin {
                                name: Builtin::ChannelRecv,
                                result_ty: Some(Type::List(_)),
                                ..
                            },
                        ..
                    } = op
                    {
                        found = true;
                    }
                }
            });
        }
        assert!(found, "expected ChannelRecv stamped List payload");
    }
}
