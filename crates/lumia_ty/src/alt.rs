//! Desugar typed `Alt` into tag tests (Option / Result).

use lumia_hir::{Builtin, Expr, Item, Module};
use lumia_syntax::BinOp;
use lumia_syntax::Span;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
pub(crate) enum AltKind {
    Option,
    Result,
}

pub(crate) fn apply_alt_desugars(module: &mut Module, kinds: &HashMap<Span, AltKind>) {
    if kinds.is_empty() {
        return;
    }
    for item in &mut module.items {
        match item {
            Item::Fun(f) => desugar_in_expr(&mut f.body, kinds),
            Item::Val { body, .. } => desugar_in_expr(body, kinds),
        }
    }
}

fn desugar_in_expr(expr: &mut Expr, kinds: &HashMap<Span, AltKind>) {
    match expr {
        Expr::Alt {
            scrutinee,
            alt,
            span,
        } => {
            desugar_in_expr(scrutinee, kinds);
            desugar_in_expr(alt, kinds);
            let kind = kinds
                .get(span)
                .copied()
                .expect("alt kind recorded during inference");
            let scrutinee = std::mem::replace(scrutinee, Box::new(Expr::Unit(*span)));
            let alt = std::mem::replace(alt, Box::new(Expr::Unit(*span)));
            *expr = desugar_alt(*scrutinee, *alt, *span, kind);
        }
        Expr::Let { value, body, .. } => {
            desugar_in_expr(value, kinds);
            desugar_in_expr(body, kinds);
        }
        Expr::Assign { value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Return { value, .. } => {
            desugar_in_expr(value, kinds);
        }
        Expr::Lambda { body, .. } => desugar_in_expr(body, kinds),
        Expr::Call { callee, args, .. } => {
            desugar_in_expr(callee, kinds);
            for a in args {
                desugar_in_expr(a, kinds);
            }
        }
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            for a in args {
                desugar_in_expr(a, kinds);
            }
        }
        Expr::Binary { left, right, .. } => {
            desugar_in_expr(left, kinds);
            desugar_in_expr(right, kinds);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            desugar_in_expr(cond, kinds);
            desugar_in_expr(then_branch, kinds);
            desugar_in_expr(else_branch, kinds);
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            desugar_in_expr(cond, kinds);
            desugar_in_expr(body, kinds);
            if let Some(s) = step {
                desugar_in_expr(s, kinds);
            }
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                desugar_in_expr(s, kinds);
            }
        }
        Expr::Var(_, _)
        | Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Bool(_, _)
        | Expr::String(_, _)
        | Expr::Char(_, _)
        | Expr::Unit(_)
        | Expr::Break(_)
        | Expr::Continue(_) => {}
    }
}

fn desugar_alt(scrutinee: Expr, alt: Expr, span: Span, kind: AltKind) -> Expr {
    let s = "__alt_s".to_string();
    let scrut_var = Expr::Var(s.clone(), span);
    let tag = Expr::BuiltinCall {
        name: Builtin::AdtTag,
        args: vec![scrut_var.clone()],
        span,
    };
    // Prelude: Option { Some(value)=0, None=1 }, Result { Ok(value)=0, Err(error)=1 }.
    let is_success = Expr::Binary {
        op: BinOp::Eq,
        left: Box::new(tag),
        right: Box::new(Expr::Int(0, span)),
        span,
    };
    let payload = Expr::BuiltinCall {
        name: Builtin::AdtField,
        args: vec![scrut_var, Expr::Int(0, span)],
        span,
    };
    let fail = match kind {
        AltKind::Option => alt,
        AltKind::Result => Expr::Let {
            name: "err".into(),
            value: Box::new(payload.clone()),
            body: Box::new(alt),
            mutable: false,
        },
    };
    Expr::Let {
        name: s,
        value: Box::new(scrutinee),
        body: Box::new(Expr::If {
            cond: Box::new(is_success),
            then_branch: Box::new(payload),
            else_branch: Box::new(fail),
            span,
        }),
        mutable: false,
    }
}
