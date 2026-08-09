//! Match exhaustiveness checking and pattern desugaring helpers.

use crate::ast::{AdtDef, Builtin, CtorInfo, Expr};
use crate::lower::{LowerCtx, LowerError};
use lumia_syntax::{BinOp, Pattern, Span};
use rustc_hash::FxHashMap as HashMap;

/// Short-circuit `and` as `if left { right } else { false }` (avoids OOB field/get).
pub(crate) fn short_and(left: Expr, right: Expr, span: Span) -> Expr {
    Expr::If {
        cond: Box::new(left),
        then_branch: Box::new(right),
        else_branch: Box::new(Expr::Bool(false, span)),
        span,
    }
}

pub(crate) fn short_or(left: Expr, right: Expr, span: Span) -> Expr {
    Expr::If {
        cond: Box::new(left),
        then_branch: Box::new(Expr::Bool(true, span)),
        else_branch: Box::new(right),
        span,
    }
}

pub(crate) fn check_module_matches(
    m: &lumia_syntax::Module,
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
    products: &HashMap<String, Vec<String>>,
) -> Result<(), LowerError> {
    for item in &m.items {
        if let lumia_syntax::Item::Val(v) = item {
            check_expr_matches(&v.body, ctors, adts, products)?;
        }
    }
    Ok(())
}

pub(crate) fn check_expr_matches(
    e: &lumia_syntax::Expr,
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
    products: &HashMap<String, Vec<String>>,
) -> Result<(), LowerError> {
    use lumia_syntax::Expr as S;
    match e {
        S::Match { arms, span, .. } => {
            check_match_exhaustiveness(arms, ctors, adts, products).map_err(|mut e| {
                e.span = *span;
                e
            })?;
            for a in arms {
                check_expr_matches(&a.body, ctors, adts, products)?;
                if let Some(g) = &a.guard {
                    check_expr_matches(g, ctors, adts, products)?;
                }
            }
        }
        S::MatchCond { arms, span, .. } => {
            if !arms.iter().any(|a| a.cond.is_none()) {
                return Err(LowerError {
                    message: "subjectless `match { }` used as expression requires a `_` arm".into(),
                    span: *span,
                });
            }
            // `_` must be last (Kotlin else is last)
            if let Some((last, rest)) = arms.split_last() {
                if last.cond.is_some() || rest.iter().any(|a| a.cond.is_none()) {
                    return Err(LowerError {
                        message: "subjectless `match { }`: `_` arm must be last and unique".into(),
                        span: *span,
                    });
                }
            }
            for a in arms {
                if let Some(c) = &a.cond {
                    check_expr_matches(c, ctors, adts, products)?;
                }
                check_expr_matches(&a.body, ctors, adts, products)?;
            }
        }
        S::Block { stmts, tail, .. } => {
            for s in stmts {
                match s {
                    lumia_syntax::Stmt::Val { expr, .. }
                    | lumia_syntax::Stmt::Var { expr, .. }
                    | lumia_syntax::Stmt::Assign { expr, .. }
                    | lumia_syntax::Stmt::Expr(expr) => {
                        check_expr_matches(expr, ctors, adts, products)?
                    }
                    lumia_syntax::Stmt::ForIn { iter, body, .. }
                    | lumia_syntax::Stmt::ForCond {
                        cond: iter, body, ..
                    } => {
                        check_expr_matches(iter, ctors, adts, products)?;
                        check_expr_matches(body, ctors, adts, products)?;
                    }
                    lumia_syntax::Stmt::Break(_) | lumia_syntax::Stmt::Continue(_) => {}
                }
            }
            if let Some(t) = tail {
                check_expr_matches(t, ctors, adts, products)?;
            }
        }
        S::Lambda { body, .. } => check_expr_matches(body, ctors, adts, products)?,
        S::Call { callee, args, .. } => {
            check_expr_matches(callee, ctors, adts, products)?;
            for a in args {
                check_expr_matches(a, ctors, adts, products)?;
            }
        }
        S::Binary { left, right, .. } | S::Pipeline { left, right, .. } => {
            check_expr_matches(left, ctors, adts, products)?;
            check_expr_matches(right, ctors, adts, products)?;
        }
        S::Unary { expr, .. } | S::Field { base: expr, .. } | S::Return { value: expr, .. } => {
            check_expr_matches(expr, ctors, adts, products)?
        }
        S::Alt { scrutinee, alt, .. } => {
            check_expr_matches(scrutinee, ctors, adts, products)?;
            check_expr_matches(alt, ctors, adts, products)?;
        }
        S::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            check_expr_matches(cond, ctors, adts, products)?;
            check_expr_matches(then_branch, ctors, adts, products)?;
            if let Some(e) = else_branch {
                check_expr_matches(e, ctors, adts, products)?;
            }
        }
        S::ListLit { elems, .. } => {
            for a in elems {
                check_expr_matches(a, ctors, adts, products)?;
            }
        }
        S::StructLit { fields, .. } => {
            for (_, v) in fields {
                check_expr_matches(v, ctors, adts, products)?;
            }
        }
        S::With { base, fields, .. } => {
            check_expr_matches(base, ctors, adts, products)?;
            for (_, v) in fields {
                check_expr_matches(v, ctors, adts, products)?;
            }
        }
        S::TupleLit { elems, .. } => {
            for a in elems {
                check_expr_matches(a, ctors, adts, products)?;
            }
        }
        S::Int(..) | S::Float(..) | S::Bool(..) | S::String(..) | S::Char(..) | S::Ident(..) => {}
        S::Interp { parts, .. } => {
            for p in parts {
                if let lumia_syntax::InterpPart::Expr(e) = p {
                    check_expr_matches(e, ctors, adts, products)?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn check_match_exhaustiveness(
    arms: &[lumia_syntax::MatchArm],
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
    products: &HashMap<String, Vec<String>>,
) -> Result<(), LowerError> {
    let pats: Vec<&Pattern> = arms
        .iter()
        // Guards refine payloads; a guarded arm does not exhaust a constructor.
        .filter(|a| a.guard.is_none())
        .map(|a| &a.pattern)
        .collect();
    check_pats_cover(&pats, ctors, adts, products, "")
}

fn flatten_or<'a>(pat: &'a Pattern, out: &mut Vec<&'a Pattern>) {
    match pat {
        Pattern::Or(ps, _) => {
            for p in ps {
                flatten_or(p, out);
            }
        }
        other => out.push(other),
    }
}

/// Irrefutable at this level for coverage: `_`, binders, or products/tuples whose
/// fields are all catch-alls. Nullary ctor names (`None`) are refutable.
pub(crate) fn coverage_catch_all(pat: &Pattern, ctors: &HashMap<String, CtorInfo>) -> bool {
    match pat {
        Pattern::Wildcard(_) => true,
        Pattern::Ident(name, _) => ctors.get(name).is_none_or(|c| c.arity != 0),
        Pattern::Or(ps, _) => ps.iter().any(|p| coverage_catch_all(p, ctors)),
        Pattern::Struct { fields, .. } => {
            fields.iter().all(|(_, sub)| coverage_catch_all(sub, ctors))
        }
        Pattern::Tuple { elems, .. } => elems.iter().all(|e| coverage_catch_all(e, ctors)),
        Pattern::Variant { .. }
        | Pattern::List { .. }
        | Pattern::Int(_, _)
        | Pattern::Float(_, _)
        | Pattern::Bool(_, _)
        | Pattern::Char(_, _)
        | Pattern::String(_, _) => false,
    }
}

/// Whether `pats` (alternatives) cover all values at this pattern depth.
/// Recurses into variant payloads, product fields, and tuple elements.
pub(crate) fn check_pats_cover(
    pats: &[&Pattern],
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
    products: &HashMap<String, Vec<String>>,
    path: &str,
) -> Result<(), LowerError> {
    use std::collections::HashSet;

    let mut flat = Vec::new();
    for p in pats {
        flatten_or(p, &mut flat);
    }
    // Empty after filtering (no arms, or only guarded arms) is never exhaustive.
    if flat.is_empty() {
        let where_ = if path.is_empty() {
            "scrutinee".into()
        } else {
            path.to_string()
        };
        return Err(LowerError::message_only(format!(
            "non-exhaustive match on {where_}: no covering arm (empty match or only guarded arms)"
        )));
    }
    if flat.iter().any(|p| coverage_catch_all(p, ctors)) {
        return Ok(());
    }

    let mut covered: HashMap<String, HashSet<i64>> = HashMap::default();
    let mut ctor_args: HashMap<String, Vec<Vec<&Pattern>>> = HashMap::default();
    let mut product_fields: HashMap<String, HashMap<String, Vec<&Pattern>>> = HashMap::default();
    let mut tuple_rows: Vec<Vec<&Pattern>> = Vec::new();
    let mut list_pats: Vec<&Pattern> = Vec::new();
    let mut saw_sum = false;
    let mut saw_product = false;
    let mut saw_list = false;
    let mut saw_int = false;
    let mut saw_float = false;
    let mut saw_bool = false;
    let mut bool_true = false;
    let mut bool_false = false;
    let mut saw_open_lit = false; // Char / String: open domains need `_`

    for p in &flat {
        match *p {
            Pattern::Ident(name, _) => {
                if let Some(c) = ctors.get(name) {
                    if c.arity == 0 {
                        saw_sum = true;
                        covered.entry(c.adt_name.clone()).or_default().insert(c.tag);
                    }
                }
            }
            Pattern::Variant { name, args, .. } => {
                if let Some(c) = ctors.get(name) {
                    saw_sum = true;
                    covered.entry(c.adt_name.clone()).or_default().insert(c.tag);
                    ctor_args
                        .entry(name.clone())
                        .or_default()
                        .push(args.iter().collect());
                }
            }
            Pattern::Struct { name, fields, .. } => {
                saw_product = true;
                let entry = product_fields.entry(name.clone()).or_default();
                for (fname, sub) in fields {
                    entry.entry(fname.clone()).or_default().push(sub);
                }
            }
            Pattern::Tuple { elems, .. } => {
                saw_product = true;
                tuple_rows.push(elems.iter().collect());
            }
            Pattern::List { .. } => {
                saw_list = true;
                list_pats.push(*p);
            }
            Pattern::Int(_, _) => {
                saw_int = true;
            }
            Pattern::Float(_, _) => {
                saw_float = true;
            }
            Pattern::Bool(b, _) => {
                saw_bool = true;
                if *b {
                    bool_true = true;
                } else {
                    bool_false = true;
                }
            }
            Pattern::Char(_, _) | Pattern::String(_, _) => {
                saw_open_lit = true;
            }
            Pattern::Wildcard(_) | Pattern::Or(_, _) => {}
        }
    }

    if saw_sum {
        for (adt_name, tags) in &covered {
            let Some(def) = adts.iter().find(|a| a.name == *adt_name) else {
                continue;
            };
            let missing: Vec<&str> = def
                .variants
                .iter()
                .filter(|v| !tags.contains(&v.tag))
                .map(|v| v.name.as_str())
                .collect();
            if !missing.is_empty() {
                let where_ = if path.is_empty() {
                    format!("`{adt_name}`")
                } else {
                    format!("`{adt_name}` (in {path})")
                };
                return Err(LowerError::message_only(format!(
                    "non-exhaustive match on {where_}: missing variant(s) {}",
                    missing.join(", ")
                )));
            }
            for v in &def.variants {
                if v.arity == 0 {
                    continue;
                }
                let Some(rows) = ctor_args.get(&v.name) else {
                    continue;
                };
                for slot in 0..v.arity {
                    let col: Vec<&Pattern> =
                        rows.iter().filter_map(|r| r.get(slot).copied()).collect();
                    if col.len() != rows.len() {
                        continue;
                    }
                    let nested = if path.is_empty() {
                        v.name.clone()
                    } else {
                        format!("{path}.{}", v.name)
                    };
                    check_pats_cover(&col, ctors, adts, products, &nested)?;
                }
            }
        }
    }

    if saw_product {
        for (pname, fields) in &product_fields {
            let order = products.get(pname).cloned().unwrap_or_default();
            for fname in &order {
                let Some(subs) = fields.get(fname) else {
                    continue;
                };
                let nested = if path.is_empty() {
                    format!("{pname}.{fname}")
                } else {
                    format!("{path}.{pname}.{fname}")
                };
                check_pats_cover(subs, ctors, adts, products, &nested)?;
            }
        }
        if !tuple_rows.is_empty() {
            let arity = tuple_rows[0].len();
            if tuple_rows.iter().all(|r| r.len() == arity) {
                for slot in 0..arity {
                    let col: Vec<&Pattern> = tuple_rows
                        .iter()
                        .filter_map(|r| r.get(slot).copied())
                        .collect();
                    let nested = if path.is_empty() {
                        format!(".{}", slot)
                    } else {
                        format!("{path}.{}", slot)
                    };
                    check_pats_cover(&col, ctors, adts, products, &nested)?;
                }
            }
        }
    }

    // Bool is a closed 2-value domain: both `true` and `false` cover it.
    if !saw_sum && !saw_product && saw_bool && !saw_int && !saw_float && !saw_list && !saw_open_lit
    {
        if bool_true && bool_false {
            return Ok(());
        }
        let where_ = if path.is_empty() {
            "Bool".into()
        } else {
            format!("Bool (in {path})")
        };
        let missing = match (bool_true, bool_false) {
            (false, false) => "true, false",
            (true, false) => "false",
            (false, true) => "true",
            (true, true) => unreachable!(),
        };
        return Err(LowerError::message_only(format!(
            "non-exhaustive match on {where_}: missing {missing} (or `_`)"
        )));
    }

    // Int / Float / Char / String / List have infinite (or open) domains: without
    // a catch-all binder/`_`, finite literal arms are never enough. List is
    // exhaustive only when every length is covered (`[]` + `[…, ..rest]` style).
    if !saw_sum && !saw_product && (saw_int || saw_float || saw_list || saw_open_lit) {
        let where_ = if path.is_empty() {
            "scrutinee".into()
        } else {
            path.to_string()
        };
        if saw_list {
            if !list_patterns_exhaustive(&list_pats) {
                return Err(LowerError::message_only(format!(
                    "non-exhaustive match on List (in {where_}): add `[]` / `[..rest]` arms or `_`"
                )));
            }
            // Nested element columns (fixed prefix positions).
            let max_fixed = list_pats
                .iter()
                .filter_map(|p| match p {
                    Pattern::List { elems, .. } => Some(elems.len()),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            for slot in 0..max_fixed {
                let col: Vec<&Pattern> = list_pats
                    .iter()
                    .filter_map(|p| match p {
                        Pattern::List { elems, .. } => elems.get(slot),
                        _ => None,
                    })
                    .collect();
                if col.len() == list_pats.len() {
                    let nested = if path.is_empty() {
                        format!("[{slot}]")
                    } else {
                        format!("{path}[{slot}]")
                    };
                    check_pats_cover(&col, ctors, adts, products, &nested)?;
                }
            }
        } else if saw_int {
            return Err(LowerError::message_only(format!(
                "non-exhaustive match on Int (in {where_}): integer literals need a `_` arm"
            )));
        } else if saw_float {
            return Err(LowerError::message_only(format!(
                "non-exhaustive match on Float (in {where_}): float literals need a `_` arm"
            )));
        } else if saw_open_lit {
            return Err(LowerError::message_only(format!(
                "non-exhaustive match on Char/String (in {where_}): literal arms need a `_` arm"
            )));
        }
    }

    Ok(())
}

/// `[]` covers length 0; `[e0,…,ek-1, ..rest]` covers all lengths `>= k`.
/// Together they must cover `0..`.
pub(crate) fn list_patterns_exhaustive(pats: &[&Pattern]) -> bool {
    use std::collections::HashSet;
    let mut exact: HashSet<usize> = HashSet::new();
    let mut rest_mins: Vec<usize> = Vec::new();
    for p in pats {
        match p {
            Pattern::List { elems, rest, .. } => {
                if rest.is_some() {
                    rest_mins.push(elems.len());
                } else {
                    exact.insert(elems.len());
                }
            }
            _ => return false,
        }
    }
    let Some(min_rest) = rest_mins.into_iter().min() else {
        // Only fixed-length arms — infinitely many lengths remain.
        return false;
    };
    (0..min_rest).all(|len| exact.contains(&len))
}

/// Last-arm elision: only `_` / binders (and all-irrefutable `or`) may skip the
/// tag test + `MatchFail`. Nullary ctor names like `None` are refutable — same
/// rule as [`coverage_catch_all`].
pub(crate) fn pattern_irrefutable(ctx: &LowerCtx, pat: &Pattern) -> bool {
    match pat {
        Pattern::Wildcard(_) => true,
        Pattern::Ident(name, _) => ctx.lookup_ctor(name).is_none_or(|c| c.arity != 0),
        Pattern::Or(ps, _) => !ps.is_empty() && ps.iter().all(|p| pattern_irrefutable(ctx, p)),
        Pattern::Tuple { elems, .. } => elems.iter().all(|p| pattern_irrefutable(ctx, p)),
        Pattern::Struct { fields, .. } => fields.iter().all(|(_, p)| pattern_irrefutable(ctx, p)),
        // Variants / lists / constants are refutable — not allowed in `val` bindings.
        _ => false,
    }
}

/// Build match condition + binder equations for `pat` against scrutinee expression `scrut`.
/// Nested patterns compose field/get paths (no temps), so binders stay valid in the arm body.
pub(crate) fn pattern_cond(
    ctx: &LowerCtx,
    pat: &Pattern,
    scrut: &Expr,
    span: Span,
) -> (Expr, Vec<(String, Expr)>) {
    match pat {
        Pattern::Wildcard(_) => (Expr::Bool(true, span), vec![]),
        Pattern::Int(n, s) => (
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(scrut.clone()),
                right: Box::new(Expr::Int(*n, *s)),
                span,
            },
            vec![],
        ),
        Pattern::Float(n, s) => (
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(scrut.clone()),
                right: Box::new(Expr::Float(*n, *s)),
                span,
            },
            vec![],
        ),
        Pattern::Bool(b, s) => (
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(scrut.clone()),
                right: Box::new(Expr::Bool(*b, *s)),
                span,
            },
            vec![],
        ),
        Pattern::Char(c, s) => (
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(scrut.clone()),
                right: Box::new(Expr::Char(*c, *s)),
                span,
            },
            vec![],
        ),
        Pattern::String(t, s) => (
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(scrut.clone()),
                right: Box::new(Expr::String(t.clone(), *s)),
                span,
            },
            vec![],
        ),
        Pattern::Ident(name, _) => {
            if let Some(c) = ctx.lookup_ctor(name) {
                if c.arity == 0 {
                    let tag = Expr::BuiltinCall {
                        name: Builtin::AdtTag,
                        args: vec![scrut.clone()],
                        span,
                    };
                    return (
                        Expr::Binary {
                            op: BinOp::Eq,
                            left: Box::new(tag),
                            right: Box::new(Expr::Int(c.tag, span)),
                            span,
                        },
                        vec![],
                    );
                }
            }
            (Expr::Bool(true, span), vec![(name.clone(), scrut.clone())])
        }
        Pattern::Or(pats, _) => {
            // Nested or-patterns with binders are ambiguous; top-level or is expanded.
            let mut cond = Expr::Bool(false, span);
            let mut binds = vec![];
            for p in pats {
                let (c, b) = pattern_cond(ctx, p, scrut, span);
                if !b.is_empty() {
                    ctx.set_err(
                        "nested or-pattern with bindings is not supported; use separate match arms"
                            .into(),
                        span,
                    );
                }
                if binds.is_empty() {
                    binds = b;
                }
                cond = short_or(cond, c, span);
            }
            (cond, binds)
        }
        Pattern::Variant { name, args, .. } => {
            let Some(c) = ctx.lookup_ctor(name) else {
                ctx.set_err(format!("unknown variant `{name}` in pattern"), span);
                return (Expr::Bool(false, span), vec![]);
            };
            if args.len() != c.arity {
                ctx.set_err(
                    format!(
                        "variant `{name}` expects {} field(s), pattern has {}",
                        c.arity,
                        args.len()
                    ),
                    span,
                );
            }
            let tag = Expr::BuiltinCall {
                name: Builtin::AdtTag,
                args: vec![scrut.clone()],
                span,
            };
            let mut cond = Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(tag),
                right: Box::new(Expr::Int(c.tag, span)),
                span,
            };
            let mut binds = vec![];
            let nfields = args.len().min(c.arity);
            for (i, ep) in args.iter().take(nfields).enumerate() {
                // Result/Option: pass ctor so ty maps Ok→T / Err→E / Some→T.
                // Other sum ctors keep 2-arg field (params[idx]); product names
                // would incorrectly fail nominal checks if we passed the ctor.
                let mut field_args = vec![scrut.clone(), Expr::Int(i as i64, span)];
                if matches!(name.as_str(), "Ok" | "Err" | "Some") {
                    field_args.push(Expr::String(name.clone(), span));
                }
                let field = Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: field_args,
                    span,
                };
                match ep {
                    Pattern::Ident(n, _) if ctx.lookup_ctor(n).is_none_or(|c| c.arity != 0) => {
                        // Binder (not a nullary ctor name).
                        binds.push((n.clone(), field));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(ctx, sub, &field, span);
                        cond = short_and(cond, sub_cond, span);
                        binds.extend(sub_binds);
                    }
                }
            }
            (cond, binds)
        }
        Pattern::Struct { name, fields, .. } => {
            let Some(order) = ctx.lookup_product(name) else {
                ctx.set_err(
                    format!("unknown product type `{name}` in struct pattern"),
                    span,
                );
                return (Expr::Bool(false, span), vec![]);
            };
            let mut cond = Expr::Bool(true, span);
            let mut binds = vec![];
            for (fname, sub) in fields {
                let Some(idx) = order.iter().position(|f| f == fname) else {
                    ctx.set_err(
                        format!("unknown field `{fname}` in `{name}` struct pattern"),
                        span,
                    );
                    continue;
                };
                // Nominal product name so ty rejects `Rect` matched as `Point`.
                let field = Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![
                        scrut.clone(),
                        Expr::Int(idx as i64, span),
                        Expr::String(name.clone(), span),
                    ],
                    span,
                };
                match sub {
                    Pattern::Ident(n, _) if ctx.lookup_ctor(n).is_none_or(|c| c.arity != 0) => {
                        binds.push((n.clone(), field));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(ctx, sub, &field, span);
                        cond = short_and(cond, sub_cond, span);
                        binds.extend(sub_binds);
                    }
                }
            }
            (cond, binds)
        }
        Pattern::Tuple { elems, .. } => {
            let mut cond = Expr::Bool(true, span);
            let mut binds = vec![];
            for (i, ep) in elems.iter().enumerate() {
                let field = Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![scrut.clone(), Expr::Int(i as i64, span)],
                    span,
                };
                match ep {
                    Pattern::Ident(n, _) if ctx.lookup_ctor(n).is_none_or(|c| c.arity != 0) => {
                        binds.push((n.clone(), field));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(ctx, sub, &field, span);
                        cond = short_and(cond, sub_cond, span);
                        binds.extend(sub_binds);
                    }
                }
            }
            (cond, binds)
        }
        Pattern::List { elems, rest, .. } => {
            let len = Expr::BuiltinCall {
                name: Builtin::ListLen,
                args: vec![scrut.clone()],
                span,
            };
            let min = elems.len() as i64;
            let mut cond = if rest.is_some() {
                Expr::Binary {
                    op: BinOp::Ge,
                    left: Box::new(len),
                    right: Box::new(Expr::Int(min, span)),
                    span,
                }
            } else {
                Expr::Binary {
                    op: BinOp::Eq,
                    left: Box::new(len),
                    right: Box::new(Expr::Int(min, span)),
                    span,
                }
            };
            let mut binds = vec![];
            for (i, ep) in elems.iter().enumerate() {
                let get = Expr::BuiltinCall {
                    name: Builtin::ListGet,
                    args: vec![scrut.clone(), Expr::Int(i as i64, span)],
                    span,
                };
                match ep {
                    Pattern::Ident(name, _)
                        if ctx.lookup_ctor(name).is_none_or(|c| c.arity != 0) =>
                    {
                        binds.push((name.clone(), get));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(ctx, sub, &get, span);
                        cond = short_and(cond, sub_cond, span);
                        binds.extend(sub_binds);
                    }
                }
            }
            if let Some(rname) = rest {
                let slice = Expr::BuiltinCall {
                    name: Builtin::ListSlice,
                    args: vec![scrut.clone(), Expr::Int(min, span)],
                    span,
                };
                binds.push((rname.clone(), slice));
            }
            (cond, binds)
        }
    }
}
