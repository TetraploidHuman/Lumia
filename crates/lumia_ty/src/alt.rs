//! Desugar typed `Alt` into tag tests (Option / Result).

use lumia_hir::{AdtDef, Builtin, Expr, Item, Module};
use lumia_syntax::BinOp;
use lumia_syntax::Span;
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone, Copy)]
pub(crate) enum AltKind {
    Option,
    Result,
}

pub(crate) fn apply_alt_desugars(module: &mut Module, kinds: &HashMap<Span, AltKind>) {
    if kinds.is_empty() {
        return;
    }
    let tags = SuccessTags::from_adts(&module.adts);
    for item in &mut module.items {
        match item {
            Item::Fun(f) => desugar_in_expr(&mut f.body, kinds, &tags),
            Item::Val { body, .. } => desugar_in_expr(body, kinds, &tags),
        }
    }
}

struct SuccessTags {
    some: i64,
    ok: i64,
}

impl SuccessTags {
    fn from_adts(adts: &[AdtDef]) -> Self {
        Self {
            some: variant_tag(adts, lumia_hir::OPTION.name, "Some"),
            ok: variant_tag(adts, lumia_hir::RESULT.name, "Ok"),
        }
    }
}

fn variant_tag(adts: &[AdtDef], adt: &str, variant: &str) -> i64 {
    adts.iter()
        .find(|a| a.name == adt)
        .and_then(|a| a.variants.iter().find(|v| v.name == variant))
        .map(|v| v.tag)
        .unwrap_or_else(|| panic!("lumia: missing prelude variant {adt}::{variant} for alt"))
}

fn desugar_in_expr(expr: &mut Expr, kinds: &HashMap<Span, AltKind>, tags: &SuccessTags) {
    match expr {
        Expr::Alt {
            scrutinee,
            alt,
            span,
        } => {
            desugar_in_expr(scrutinee, kinds, tags);
            desugar_in_expr(alt, kinds, tags);
            let kind = kinds
                .get(span)
                .copied()
                .expect("alt kind recorded during inference");
            let scrutinee = std::mem::replace(scrutinee, Box::new(Expr::Unit(*span)));
            let alt = std::mem::replace(alt, Box::new(Expr::Unit(*span)));
            let success_tag = match kind {
                AltKind::Option => tags.some,
                AltKind::Result => tags.ok,
            };
            *expr = desugar_alt(*scrutinee, *alt, *span, kind, success_tag);
        }
        Expr::Let { value, body, .. } => {
            desugar_in_expr(value, kinds, tags);
            desugar_in_expr(body, kinds, tags);
        }
        Expr::Assign { value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Return { value, .. } => {
            desugar_in_expr(value, kinds, tags);
        }
        Expr::Lambda { body, .. } => desugar_in_expr(body, kinds, tags),
        Expr::Call { callee, args, .. } => {
            desugar_in_expr(callee, kinds, tags);
            for a in args {
                desugar_in_expr(a, kinds, tags);
            }
        }
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            for a in args {
                desugar_in_expr(a, kinds, tags);
            }
        }
        Expr::Binary { left, right, .. } => {
            desugar_in_expr(left, kinds, tags);
            desugar_in_expr(right, kinds, tags);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            desugar_in_expr(cond, kinds, tags);
            desugar_in_expr(then_branch, kinds, tags);
            desugar_in_expr(else_branch, kinds, tags);
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            desugar_in_expr(cond, kinds, tags);
            desugar_in_expr(body, kinds, tags);
            if let Some(s) = step {
                desugar_in_expr(s, kinds, tags);
            }
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                desugar_in_expr(s, kinds, tags);
            }
        }
        Expr::With { base, fields, .. } => {
            desugar_in_expr(base, kinds, tags);
            for (_, e) in fields {
                desugar_in_expr(e, kinds, tags);
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

fn desugar_alt(scrutinee: Expr, alt: Expr, span: Span, kind: AltKind, success_tag: i64) -> Expr {
    let s = "__alt_s".to_string();
    let scrut_var = Expr::Var(s.clone(), span);
    let tag = Expr::BuiltinCall {
        name: Builtin::AdtTag,
        args: vec![scrut_var.clone()],
        span,
    };
    let is_success = Expr::Binary {
        op: BinOp::Eq,
        left: Box::new(tag),
        right: Box::new(Expr::Int(success_tag, span)),
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
            ty: None,
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
        ty: None,
    }
}
