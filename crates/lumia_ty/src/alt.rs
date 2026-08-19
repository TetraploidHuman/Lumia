//! Desugar typed `Alt` into tag tests (Option / Result).

use crate::types::TypeError;
use lumia_hir::{for_each_expr_mut, AdtDef, Builtin, Expr, Item, Module};
use lumia_syntax::BinOp;
use lumia_syntax::Span;
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AltKind {
    Option,
    Result,
}

pub(crate) fn apply_alt_desugars(
    module: &mut Module,
    kinds: &HashMap<Span, AltKind>,
) -> Result<(), TypeError> {
    if kinds.is_empty() {
        return Ok(());
    }
    let tags = SuccessTags::try_from_adts(&module.adts)?;
    for item in &mut module.items {
        match item {
            Item::Fun(f) => desugar_in_expr(&mut f.body, kinds, &tags)?,
            Item::Val { body, .. } => desugar_in_expr(body, kinds, &tags)?,
        }
    }
    Ok(())
}

struct SuccessTags {
    some: i64,
    ok: i64,
}

impl SuccessTags {
    fn try_from_adts(adts: &[AdtDef]) -> Result<Self, TypeError> {
        Ok(Self {
            some: variant_tag(adts, lumia_hir::OPTION.name, "Some")?,
            ok: variant_tag(adts, lumia_hir::RESULT.name, "Ok")?,
        })
    }
}

fn variant_tag(adts: &[AdtDef], adt: &str, variant: &str) -> Result<i64, TypeError> {
    adts.iter()
        .find(|a| a.name == adt)
        .and_then(|a| a.variants.iter().find(|v| v.name == variant))
        .map(|v| v.tag)
        .ok_or_else(|| {
            TypeError::Message(format!(
                "internal: missing prelude variant {adt}::{variant} for alt desugar"
            ))
        })
}

fn desugar_in_expr(
    expr: &mut Expr,
    kinds: &HashMap<Span, AltKind>,
    tags: &SuccessTags,
) -> Result<(), TypeError> {
    let mut err = None;
    // Post-order: nested alts rewrite before outer ones replace the node.
    for_each_expr_mut(expr, &mut |e| {
        if err.is_some() {
            return;
        }
        let Expr::Alt {
            scrutinee,
            alt,
            span,
        } = e
        else {
            return;
        };
        let Some(kind) = kinds.get(span).copied() else {
            err = Some(TypeError::Message(
                "internal: alt kind missing after inference".into(),
            ));
            return;
        };
        let scrutinee = std::mem::replace(scrutinee, Box::new(Expr::Unit(*span)));
        let alt = std::mem::replace(alt, Box::new(Expr::Unit(*span)));
        let success_tag = match kind {
            AltKind::Option => tags.some,
            AltKind::Result => tags.ok,
        };
        *e = desugar_alt(*scrutinee, *alt, *span, kind, success_tag);
    });
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn desugar_alt(scrutinee: Expr, alt: Expr, span: Span, kind: AltKind, success_tag: i64) -> Expr {
    let s = "__alt_s".to_string();
    let scrut_var = Expr::Var(s.clone().into(), span);
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
        name: s.into(),
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
