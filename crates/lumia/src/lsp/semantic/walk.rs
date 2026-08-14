//! AST walk that paints declarations, expressions, and patterns.

use super::overlay::find_word;
use super::token::{
    push, AbsToken, MOD_DECL, MOD_DEFAULT_LIB, MOD_READONLY, TY_ENUM, TY_ENUM_MEMBER, TY_FUNCTION,
    TY_METHOD, TY_PARAMETER, TY_PROPERTY, TY_STRUCT, TY_TYPE, TY_VARIABLE,
};
use crate::lsp::state::Analysis;
use lumia_hir::{surface_names, SurfaceRole};
use lumia_syntax::{
    Expr, ForBinding, ImportNames, Item, MatchArm, MatchCondArm, Module, Pattern, Stmt, TypeKind,
    ValItem,
};
use lumia_ty::Type;

pub(super) fn collect_module(a: &Analysis, module: &Module, src: &str, out: &mut Vec<AbsToken>) {
    // Module name after `module` (legend has no `namespace`; paint as type).
    if let Some((s, e)) = find_word(src, &module.name, module.span.start.0 as usize, src.len()) {
        push(out, s, e, TY_TYPE, 0);
    }
    for imp in &module.imports {
        collect_import(a, imp, src, out);
    }
    for item in &module.items {
        collect_item(a, item, src, out);
    }
}

fn collect_import(a: &Analysis, imp: &lumia_syntax::Import, src: &str, out: &mut Vec<AbsToken>) {
    let start = imp.span.start.0 as usize;
    let end = imp.span.end.0 as usize;
    let mut cursor = start;
    for seg in &imp.path {
        if let Some((s, e)) = find_word(src, seg, cursor, end) {
            push(out, s, e, TY_TYPE, 0);
            cursor = e;
        }
    }
    match &imp.names {
        ImportNames::All => {}
        ImportNames::Single(n) => paint_imported_name(a, src, n, cursor, end, out),
        ImportNames::Selective(names) => {
            for n in names {
                paint_imported_name(a, src, n, cursor, end, out);
            }
        }
    }
}

fn paint_imported_name(
    a: &Analysis,
    src: &str,
    n: &lumia_syntax::ImportedName,
    from: usize,
    to: usize,
    out: &mut Vec<AbsToken>,
) {
    let local = n.local();
    // Prefer painting the local binding name (alias when present).
    if let Some((s, e)) = find_word(src, local, from, to) {
        let (ty, mods) = classify_ident(a, local, &Default::default());
        push(out, s, e, ty, mods | MOD_DEFAULT_LIB);
    } else if local != n.name.as_str() {
        if let Some((s, e)) = find_word(src, &n.name, from, to) {
            let (ty, mods) = classify_ident(a, &n.name, &Default::default());
            push(out, s, e, ty, mods | MOD_DEFAULT_LIB);
        }
    }
}

fn collect_item(a: &Analysis, item: &Item, src: &str, out: &mut Vec<AbsToken>) {
    match item {
        Item::Val(v) => collect_val(a, v, src, out),
        Item::Type(t) => {
            let start = t.span.start.0 as usize;
            let end = t.span.end.0 as usize;
            if let Some((s, e)) = find_word(src, &t.name, start, end) {
                let ty = match &t.kind {
                    TypeKind::Product(_) => TY_STRUCT,
                    TypeKind::Sum(_) => TY_ENUM,
                };
                push(out, s, e, ty, MOD_DECL);
            }
            match &t.kind {
                TypeKind::Product(fields) => {
                    for f in fields {
                        if let Some((s, e)) = find_word(src, f, start, end) {
                            push(out, s, e, TY_PROPERTY, MOD_DECL);
                        }
                    }
                }
                TypeKind::Sum(variants) => {
                    for v in variants {
                        if let Some((s, e)) = find_word(src, &v.name, start, end) {
                            push(out, s, e, TY_ENUM_MEMBER, MOD_DECL);
                        }
                    }
                }
            }
        }
        Item::Foreign(f) => {
            let start = f.span.start.0 as usize;
            let end = f.span.end.0 as usize;
            if let Some((s, e)) = find_word(src, &f.name, start, end) {
                push(out, s, e, TY_FUNCTION, MOD_DECL | MOD_DEFAULT_LIB);
            }
            for (pname, pty) in &f.params {
                if let Some((s, e)) = find_word(src, pname, start, end) {
                    push(out, s, e, TY_PARAMETER, MOD_DECL);
                }
                if let Some((s, e)) = find_word(src, pty, start, end) {
                    push(out, s, e, TY_TYPE, 0);
                }
            }
            if let Some((s, e)) = find_word(src, &f.ret, start, end) {
                push(out, s, e, TY_TYPE, 0);
            }
        }
        Item::Trait(t) => {
            let start = t.span.start.0 as usize;
            let end = t.span.end.0 as usize;
            if let Some((s, e)) = find_word(src, &t.name, start, end) {
                push(out, s, e, TY_TYPE, MOD_DECL);
            }
            for r in &t.requires {
                if let Some((s, e)) = find_word(src, r, start, end) {
                    push(out, s, e, TY_TYPE, 0);
                }
            }
            for m in &t.methods {
                collect_val(a, m, src, out);
            }
        }
        Item::Instance(i) => {
            let start = i.span.start.0 as usize;
            let end = i.span.end.0 as usize;
            if let Some((s, e)) = find_word(src, &i.trait_name, start, end) {
                push(out, s, e, TY_TYPE, 0);
            }
            if let Some((s, e)) = find_word(src, &i.type_name, start, end) {
                let ty = if a.typed.module.adts.iter().any(|x| x.name == i.type_name) {
                    TY_ENUM
                } else {
                    TY_STRUCT
                };
                push(out, s, e, ty, 0);
            }
            for m in &i.methods {
                collect_val(a, m, src, out);
            }
        }
    }
}

fn collect_val(a: &Analysis, v: &ValItem, src: &str, out: &mut Vec<AbsToken>) {
    let start = v.span.start.0 as usize;
    let body_start = v.body.span().start.0 as usize;
    let name_end = body_start.min(src.len());
    if let Some((s, e)) = find_word(src, &v.name, start, name_end) {
        let is_fn = v.params.is_some()
            || matches!(v.body, Expr::Lambda { .. })
            || matches!(a.typed.fun_types.get(&v.name), Some(Type::Fun(..)));
        let (ty, mods) = if is_fn {
            (TY_FUNCTION, MOD_DECL)
        } else {
            // Top-level and nested `val` are both immutable bindings.
            (TY_VARIABLE, MOD_DECL | MOD_READONLY)
        };
        push(out, s, e, ty, mods);
    }
    if let Some(params) = &v.params {
        let mut cursor = start;
        for (p, _) in params {
            if let Some((s, e)) = find_word(src, p, cursor, name_end) {
                push(out, s, e, TY_PARAMETER, MOD_DECL);
                cursor = e;
            }
        }
    }
    let bare: Vec<String> = v
        .params
        .as_ref()
        .map(|ps| ps.iter().map(|(n, _)| n.clone()).collect())
        .unwrap_or_default();
    let mut params = params_set(&bare);
    if let Expr::Lambda {
        params: lp, body, ..
    } = &v.body
    {
        // `val f = { x -> … }` — paint lambda params; body walk adds them too.
        paint_lambda_params(src, v.body.span().start.0 as usize, lp, out);
        for p in lp {
            params.insert(p.clone());
        }
        collect_expr(a, body, src, &params, out);
    } else {
        collect_expr(a, &v.body, src, &params, out);
    }
}

fn params_set(names: &[String]) -> rustc_hash::FxHashSet<String> {
    names.iter().cloned().collect()
}

fn paint_lambda_params(src: &str, lam_start: usize, params: &[String], out: &mut Vec<AbsToken>) {
    let end = src.len().min(lam_start + 256);
    let mut cursor = lam_start;
    for p in params {
        if let Some((s, e)) = find_word(src, p, cursor, end) {
            push(out, s, e, TY_PARAMETER, MOD_DECL);
            cursor = e;
        }
    }
}

fn collect_expr(
    a: &Analysis,
    expr: &Expr,
    src: &str,
    params: &rustc_hash::FxHashSet<String>,
    out: &mut Vec<AbsToken>,
) {
    match expr {
        Expr::Ident(name, sp) => {
            let (ty, mods) = classify_ident(a, name, params);
            push(out, sp.start.0 as usize, sp.end.0 as usize, ty, mods);
        }
        Expr::Interp { parts, .. } => {
            for p in parts {
                if let lumia_syntax::InterpPart::Expr(e) = p {
                    collect_expr(a, e, src, params, out);
                }
            }
        }
        Expr::Block { stmts, tail, .. } => {
            for s in stmts {
                collect_stmt(a, s, src, params, out);
            }
            if let Some(t) = tail {
                collect_expr(a, t, src, params, out);
            }
        }
        Expr::Lambda {
            params: lp,
            body,
            span,
            ..
        } => {
            paint_lambda_params(src, span.start.0 as usize, lp, out);
            let mut nested = params.clone();
            for p in lp {
                nested.insert(p.clone());
            }
            collect_expr(a, body, src, &nested, out);
        }
        Expr::Call { callee, args, .. } => {
            if let Expr::Field { base, field, span } = callee.as_ref() {
                collect_expr(a, base, src, params, out);
                paint_field_name(a, src, base, field, *span, out, true);
            } else {
                collect_expr(a, callee, src, params, out);
            }
            for arg in args {
                collect_expr(a, arg, src, params, out);
            }
        }
        Expr::Field { base, field, span } => {
            collect_expr(a, base, src, params, out);
            paint_field_name(a, src, base, field, *span, out, false);
        }
        Expr::Binary { left, right, .. } | Expr::Pipeline { left, right, .. } => {
            collect_expr(a, left, src, params, out);
            collect_expr(a, right, src, params, out);
        }
        Expr::Unary { expr: inner, .. } | Expr::Return { value: inner, .. } => {
            collect_expr(a, inner, src, params, out);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_expr(a, cond, src, params, out);
            collect_expr(a, then_branch, src, params, out);
            if let Some(e) = else_branch {
                collect_expr(a, e, src, params, out);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_expr(a, scrutinee, src, params, out);
            for arm in arms {
                collect_match_arm(a, arm, src, params, out);
            }
        }
        Expr::MatchCond { arms, .. } => {
            for arm in arms {
                collect_match_cond_arm(a, arm, src, params, out);
            }
        }
        Expr::Alt { scrutinee, alt, .. } => {
            collect_expr(a, scrutinee, src, params, out);
            collect_expr(a, alt, src, params, out);
        }
        Expr::ListLit { elems, .. } | Expr::TupleLit { elems, .. } => {
            for e in elems {
                collect_expr(a, e, src, params, out);
            }
        }
        Expr::StructLit { name, fields, span } => {
            let start = span.start.0 as usize;
            let end = span.end.0 as usize;
            if let Some((s, e)) = find_word(src, name, start, end) {
                let ty = if a.typed.module.adts.iter().any(|x| x.name == *name) {
                    TY_ENUM
                } else {
                    TY_STRUCT
                };
                push(out, s, e, ty, 0);
            }
            for (fname, fexpr) in fields {
                if let Some((s, e)) = find_word(src, fname, start, fexpr.span().start.0 as usize) {
                    push(out, s, e, TY_PROPERTY, 0);
                }
                collect_expr(a, fexpr, src, params, out);
            }
        }
        Expr::With { base, fields, .. } => {
            collect_expr(a, base, src, params, out);
            for (fname, fexpr) in fields {
                let fs = fexpr.span().start.0 as usize;
                if let Some((s, e)) = find_word(src, fname, fs.saturating_sub(64), fs) {
                    push(out, s, e, TY_PROPERTY, 0);
                }
                collect_expr(a, fexpr, src, params, out);
            }
        }
        Expr::Scope {
            scheduler, body, ..
        } => {
            if let Some(s) = scheduler {
                collect_expr(a, s, src, params, out);
            }
            collect_expr(a, body, src, params, out);
        }
        Expr::Spawn { body, .. } => collect_expr(a, body, src, params, out),
        Expr::Int(..) | Expr::Float(..) | Expr::Bool(..) | Expr::String(..) | Expr::Char(..) => {}
    }
}

fn paint_field_name(
    a: &Analysis,
    src: &str,
    base: &Expr,
    field: &str,
    span: lumia_syntax::Span,
    out: &mut Vec<AbsToken>,
    is_call: bool,
) {
    let start = span.start.0 as usize;
    let end = span.end.0 as usize;
    // Prefer text after base: `recv.field`
    let from = base.span().end.0 as usize;
    let (s, e) = find_word(src, field, from, end)
        .or_else(|| find_word(src, field, start, end))
        .unwrap_or((0, 0));
    if e <= s {
        return;
    }
    let is_variant = if let Expr::Ident(n, _) = base {
        a.typed
            .module
            .adts
            .iter()
            .any(|adt| adt.name == *n && adt.variants.iter().any(|v| v.name == field))
    } else {
        false
    };
    let is_surface_method =
        surface_names().any(|sn| sn.name == field && sn.role == SurfaceRole::Method);
    let (ty, mods) = if is_variant {
        (TY_ENUM_MEMBER, 0)
    } else if is_call || is_surface_method {
        (
            TY_METHOD,
            if is_surface_method {
                MOD_DEFAULT_LIB
            } else {
                0
            },
        )
    } else {
        (TY_PROPERTY, 0)
    };
    push(out, s, e, ty, mods);
}

fn collect_stmt(
    a: &Analysis,
    stmt: &Stmt,
    src: &str,
    params: &rustc_hash::FxHashSet<String>,
    out: &mut Vec<AbsToken>,
) {
    match stmt {
        Stmt::Val { pat, expr, .. } => {
            collect_pattern(a, pat, src, out);
            collect_expr(a, expr, src, params, out);
        }
        Stmt::Var {
            name, expr, span, ..
        } => {
            let start = span.start.0 as usize;
            let before = expr.span().start.0 as usize;
            if let Some((s, e)) = find_word(src, name, start, before) {
                push(out, s, e, TY_VARIABLE, MOD_DECL);
            }
            collect_expr(a, expr, src, params, out);
        }
        Stmt::Assign { name, expr, span } => {
            let start = span.start.0 as usize;
            let before = expr.span().start.0 as usize;
            if let Some((s, e)) = find_word(src, name, start, before) {
                push(out, s, e, TY_VARIABLE, 0);
            }
            collect_expr(a, expr, src, params, out);
        }
        Stmt::Expr(e) => collect_expr(a, e, src, params, out),
        Stmt::ForIn {
            binding,
            iter,
            body,
            span,
        } => {
            let start = span.start.0 as usize;
            let before = iter.span().start.0 as usize;
            match binding {
                ForBinding::Name(n) => {
                    if let Some((s, e)) = find_word(src, n, start, before) {
                        push(out, s, e, TY_VARIABLE, MOD_DECL);
                    }
                }
                ForBinding::Pair(a0, b0) => {
                    let mut cursor = start;
                    for n in [a0, b0] {
                        if let Some((s, e)) = find_word(src, n, cursor, before) {
                            push(out, s, e, TY_VARIABLE, MOD_DECL);
                            cursor = e;
                        }
                    }
                }
            }
            collect_expr(a, iter, src, params, out);
            collect_expr(a, body, src, params, out);
        }
        Stmt::ForCond { cond, body, .. } => {
            collect_expr(a, cond, src, params, out);
            collect_expr(a, body, src, params, out);
        }
        Stmt::Break(_) | Stmt::Continue(_) => {}
    }
}

fn collect_match_arm(
    a: &Analysis,
    arm: &MatchArm,
    src: &str,
    params: &rustc_hash::FxHashSet<String>,
    out: &mut Vec<AbsToken>,
) {
    collect_pattern(a, &arm.pattern, src, out);
    if let Some(g) = &arm.guard {
        collect_expr(a, g, src, params, out);
    }
    collect_expr(a, &arm.body, src, params, out);
}

fn collect_match_cond_arm(
    a: &Analysis,
    arm: &MatchCondArm,
    src: &str,
    params: &rustc_hash::FxHashSet<String>,
    out: &mut Vec<AbsToken>,
) {
    if let Some(c) = &arm.cond {
        collect_expr(a, c, src, params, out);
    }
    collect_expr(a, &arm.body, src, params, out);
}

fn collect_pattern(a: &Analysis, pat: &Pattern, src: &str, out: &mut Vec<AbsToken>) {
    match pat {
        Pattern::Ident(name, sp) => {
            let is_variant = a
                .typed
                .module
                .adts
                .iter()
                .any(|adt| adt.variants.iter().any(|v| v.name == *name));
            let ty = if is_variant {
                TY_ENUM_MEMBER
            } else {
                TY_VARIABLE
            };
            let mods = if is_variant { 0 } else { MOD_DECL };
            push(out, sp.start.0 as usize, sp.end.0 as usize, ty, mods);
        }
        Pattern::Variant { name, args, span } => {
            let start = span.start.0 as usize;
            let end = span.end.0 as usize;
            if let Some((s, e)) = find_word(src, name, start, end) {
                push(out, s, e, TY_ENUM_MEMBER, 0);
            }
            for arg in args {
                collect_pattern(a, arg, src, out);
            }
        }
        Pattern::Struct { name, fields, span } => {
            let start = span.start.0 as usize;
            let end = span.end.0 as usize;
            if let Some((s, e)) = find_word(src, name, start, end) {
                let is_variant = a
                    .typed
                    .module
                    .adts
                    .iter()
                    .any(|adt| adt.variants.iter().any(|v| v.name == *name));
                push(
                    out,
                    s,
                    e,
                    if is_variant {
                        TY_ENUM_MEMBER
                    } else {
                        TY_STRUCT
                    },
                    0,
                );
            }
            for (fname, fpat) in fields {
                if let Some((s, e)) = find_word(src, fname, start, fpat_span_start(fpat)) {
                    push(out, s, e, TY_PROPERTY, 0);
                }
                collect_pattern(a, fpat, src, out);
            }
        }
        Pattern::Tuple { elems, .. } | Pattern::Or(elems, _) => {
            for e in elems {
                collect_pattern(a, e, src, out);
            }
        }
        Pattern::List { elems, rest, span } => {
            for e in elems {
                collect_pattern(a, e, src, out);
            }
            if let Some(r) = rest {
                if let Some((s, e)) = find_word(src, r, span.start.0 as usize, span.end.0 as usize)
                {
                    push(out, s, e, TY_VARIABLE, MOD_DECL);
                }
            }
        }
        Pattern::Wildcard(_)
        | Pattern::Int(..)
        | Pattern::Float(..)
        | Pattern::Bool(..)
        | Pattern::Char(..)
        | Pattern::String(..) => {}
    }
}

fn fpat_span_start(p: &Pattern) -> usize {
    match p {
        Pattern::Wildcard(s)
        | Pattern::Int(_, s)
        | Pattern::Float(_, s)
        | Pattern::Bool(_, s)
        | Pattern::Char(_, s)
        | Pattern::String(_, s)
        | Pattern::Ident(_, s)
        | Pattern::Variant { span: s, .. }
        | Pattern::Struct { span: s, .. }
        | Pattern::Tuple { span: s, .. }
        | Pattern::List { span: s, .. }
        | Pattern::Or(_, s) => s.start.0 as usize,
    }
}

fn classify_ident(a: &Analysis, name: &str, params: &rustc_hash::FxHashSet<String>) -> (u32, u32) {
    if params.contains(name) {
        return (TY_PARAMETER, 0);
    }
    if a.typed.module.products.iter().any(|p| p.name == name) {
        return (TY_STRUCT, 0);
    }
    if a.typed.module.adts.iter().any(|adt| adt.name == name) {
        return (TY_ENUM, 0);
    }
    if a.typed
        .module
        .adts
        .iter()
        .any(|adt| adt.variants.iter().any(|v| v.name == name))
    {
        return (TY_ENUM_MEMBER, 0);
    }
    if let Some(ty) = a.typed.fun_types.get(name) {
        return if matches!(ty, Type::Fun(..)) {
            (TY_FUNCTION, 0)
        } else {
            (TY_VARIABLE, MOD_READONLY)
        };
    }
    if surface_names().any(|s| s.name == name && s.role == SurfaceRole::Free) {
        return (TY_FUNCTION, MOD_DEFAULT_LIB);
    }
    (TY_VARIABLE, 0)
}
