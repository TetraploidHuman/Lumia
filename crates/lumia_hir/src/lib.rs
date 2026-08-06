//! High-level IR — named bindings after light desugaring from syntax AST.

use lumia_syntax::{BinOp, Pattern, Span, UnOp, VariantFields};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static CTORS: RefCell<HashMap<String, CtorInfo>> = RefCell::new(HashMap::new());
    /// Product field name → (type name, field index). MVP: names unique per module.
    static PRODUCT_FIELDS: RefCell<HashMap<String, (String, usize)>> = RefCell::new(HashMap::new());
    static PRODUCTS: RefCell<HashMap<String, Vec<String>>> = RefCell::new(HashMap::new());
}

fn with_ctors<R>(ctors: HashMap<String, CtorInfo>, f: impl FnOnce() -> R) -> R {
    CTORS.with(|c| {
        *c.borrow_mut() = ctors;
        let r = f();
        c.borrow_mut().clear();
        r
    })
}

fn with_products<R>(
    products: HashMap<String, Vec<String>>,
    fields: HashMap<String, (String, usize)>,
    f: impl FnOnce() -> R,
) -> R {
    PRODUCTS.with(|p| {
        PRODUCT_FIELDS.with(|pf| {
            *p.borrow_mut() = products;
            *pf.borrow_mut() = fields;
            let r = f();
            p.borrow_mut().clear();
            pf.borrow_mut().clear();
            r
        })
    })
}

fn lookup_ctor(name: &str) -> Option<CtorInfo> {
    CTORS.with(|c| c.borrow().get(name).cloned())
}

fn lookup_product_field(name: &str) -> Option<(String, usize)> {
    PRODUCT_FIELDS.with(|c| c.borrow().get(name).cloned())
}

fn lookup_product(name: &str) -> Option<Vec<String>> {
    PRODUCTS.with(|c| c.borrow().get(name).cloned())
}

/// Lower syntax AST → HIR with desugaring.
pub fn lower_module(m: &lumia_syntax::Module) -> Result<Module, String> {
    let mut adts = Vec::new();
    let mut products = Vec::new();
    let mut ctors = HashMap::new();
    let mut product_map = HashMap::new();
    let mut product_fields = HashMap::new();
    for item in &m.items {
        if let lumia_syntax::Item::Type(t) = item {
            match &t.kind {
                lumia_syntax::TypeKind::Sum(variants) => {
                    let mut vs = Vec::new();
                    for (tag, v) in variants.iter().enumerate() {
                        let arity = match &v.fields {
                            VariantFields::Unit => 0,
                            VariantFields::Positional(n) => *n,
                            VariantFields::Named(fs) => fs.len(),
                        };
                        ctors.insert(
                            v.name.clone(),
                            CtorInfo {
                                adt_name: t.name.clone(),
                                tag: tag as i64,
                                arity,
                            },
                        );
                        vs.push(AdtVariant {
                            name: v.name.clone(),
                            tag: tag as i64,
                            arity,
                        });
                    }
                    adts.push(AdtDef {
                        name: t.name.clone(),
                        variants: vs,
                    });
                }
                lumia_syntax::TypeKind::Product(fields) => {
                    for (i, f) in fields.iter().enumerate() {
                        product_fields.insert(f.clone(), (t.name.clone(), i));
                    }
                    product_map.insert(t.name.clone(), fields.clone());
                    products.push(ProductDef {
                        name: t.name.clone(),
                        fields: fields.clone(),
                    });
                }
            }
        }
    }

    check_module_matches(m, &ctors, &adts)?;

    Ok(with_ctors(ctors, || {
        with_products(product_map, product_fields, || {
            let mut items = Vec::new();
            for item in &m.items {
                match item {
                    lumia_syntax::Item::Val(v) => {
                        let body = lower_expr(&v.body);
                        let body = if let Some(params) = &v.params {
                            Expr::Lambda {
                                params: params.clone(),
                                body: Box::new(body),
                                span: v.span,
                            }
                        } else {
                            body
                        };
                        match body {
                            Expr::Lambda {
                                params,
                                body,
                                span: _,
                            } => {
                                items.push(Item::Fun(Fun {
                                    name: v.name.clone(),
                                    params,
                                    body: *body,
                                    is_main: v.name == "main",
                                }));
                            }
                            other => {
                                if v.name == "main" {
                                    items.push(Item::Fun(Fun {
                                        name: "main".into(),
                                        params: vec![],
                                        body: other,
                                        is_main: true,
                                    }));
                                } else {
                                    items.push(Item::Val {
                                        name: v.name.clone(),
                                        body: other,
                                    });
                                }
                            }
                        }
                    }
                    lumia_syntax::Item::Type(_) => {}
                }
            }
            Module {
                name: m.name.clone(),
                items,
                adts,
                products,
            }
        })
    }))
}

fn check_module_matches(
    m: &lumia_syntax::Module,
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
) -> Result<(), String> {
    for item in &m.items {
        if let lumia_syntax::Item::Val(v) = item {
            check_expr_matches(&v.body, ctors, adts)?;
        }
    }
    Ok(())
}

fn check_expr_matches(
    e: &lumia_syntax::Expr,
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
) -> Result<(), String> {
    use lumia_syntax::Expr as S;
    match e {
        S::Match { arms, .. } => {
            check_match_exhaustiveness(arms, ctors, adts)?;
            for a in arms {
                check_expr_matches(&a.body, ctors, adts)?;
                if let Some(g) = &a.guard {
                    check_expr_matches(g, ctors, adts)?;
                }
            }
        }
        S::Block { stmts, tail, .. } => {
            for s in stmts {
                match s {
                    lumia_syntax::Stmt::Val { expr, .. }
                    | lumia_syntax::Stmt::Var { expr, .. }
                    | lumia_syntax::Stmt::Assign { expr, .. }
                    | lumia_syntax::Stmt::Expr(expr) => check_expr_matches(expr, ctors, adts)?,
                    lumia_syntax::Stmt::ForIn { iter, body, .. }
                    | lumia_syntax::Stmt::ForCond { cond: iter, body, .. } => {
                        check_expr_matches(iter, ctors, adts)?;
                        check_expr_matches(body, ctors, adts)?;
                    }
                    lumia_syntax::Stmt::Break(_) | lumia_syntax::Stmt::Continue(_) => {}
                }
            }
            if let Some(t) = tail {
                check_expr_matches(t, ctors, adts)?;
            }
        }
        S::Lambda { body, .. } => check_expr_matches(body, ctors, adts)?,
        S::Call { callee, args, .. } => {
            check_expr_matches(callee, ctors, adts)?;
            for a in args {
                check_expr_matches(a, ctors, adts)?;
            }
        }
        S::Binary { left, right, .. } | S::Pipeline { left, right, .. } => {
            check_expr_matches(left, ctors, adts)?;
            check_expr_matches(right, ctors, adts)?;
        }
        S::Unary { expr, .. } | S::Field { base: expr, .. } => {
            check_expr_matches(expr, ctors, adts)?
        }
        S::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            check_expr_matches(cond, ctors, adts)?;
            check_expr_matches(then_branch, ctors, adts)?;
            if let Some(e) = else_branch {
                check_expr_matches(e, ctors, adts)?;
            }
        }
        S::ListLit { elems, .. } => {
            for a in elems {
                check_expr_matches(a, ctors, adts)?;
            }
        }
        S::StructLit { fields, .. } => {
            for (_, v) in fields {
                check_expr_matches(v, ctors, adts)?;
            }
        }
        S::With { base, fields, .. } => {
            check_expr_matches(base, ctors, adts)?;
            for (_, v) in fields {
                check_expr_matches(v, ctors, adts)?;
            }
        }
        S::TupleLit { elems, .. } => {
            for a in elems {
                check_expr_matches(a, ctors, adts)?;
            }
        }
        S::Int(..) | S::Float(..) | S::Bool(..) | S::String(..) | S::Char(..) | S::Ident(..) => {}
        S::Interp { parts, .. } => {
            for p in parts {
                if let lumia_syntax::InterpPart::Expr(e) = p {
                    check_expr_matches(e, ctors, adts)?;
                }
            }
        }
    }
    Ok(())
}

fn check_match_exhaustiveness(
    arms: &[lumia_syntax::MatchArm],
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
) -> Result<(), String> {
    use std::collections::HashSet;
    let mut covered: HashMap<String, HashSet<i64>> = HashMap::new();
    let mut catch_all = false;
    let mut saw_sum = false;

    for arm in arms {
        collect_pat_coverage(&arm.pattern, ctors, &mut covered, &mut catch_all, &mut saw_sum);
    }
    if !saw_sum || catch_all {
        return Ok(());
    }
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
            return Err(format!(
                "non-exhaustive match on `{adt_name}`: missing variant(s) {}",
                missing.join(", ")
            ));
        }
    }
    Ok(())
}

fn collect_pat_coverage(
    pat: &Pattern,
    ctors: &HashMap<String, CtorInfo>,
    covered: &mut HashMap<String, std::collections::HashSet<i64>>,
    catch_all: &mut bool,
    saw_sum: &mut bool,
) {
    match pat {
        Pattern::Wildcard(_) => *catch_all = true,
        Pattern::Ident(name, _) => {
            if let Some(c) = ctors.get(name) {
                if c.arity == 0 {
                    *saw_sum = true;
                    covered
                        .entry(c.adt_name.clone())
                        .or_default()
                        .insert(c.tag);
                    return;
                }
            }
            *catch_all = true;
        }
        Pattern::Variant { name, .. } => {
            if let Some(c) = ctors.get(name) {
                *saw_sum = true;
                covered
                    .entry(c.adt_name.clone())
                    .or_default()
                    .insert(c.tag);
            }
        }
        Pattern::Or(pats, _) => {
            for p in pats {
                collect_pat_coverage(p, ctors, covered, catch_all, saw_sum);
            }
        }
        Pattern::Struct { .. }
        | Pattern::Tuple { .. }
        | Pattern::List { .. }
        | Pattern::Int(_, _) => {}
    }
}

#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub items: Vec<Item>,
    /// Sum types declared in this module (product types ignored for now).
    pub adts: Vec<AdtDef>,
    pub products: Vec<ProductDef>,
}

#[derive(Debug, Clone)]
pub struct ProductDef {
    pub name: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AdtDef {
    pub name: String,
    pub variants: Vec<AdtVariant>,
}

#[derive(Debug, Clone)]
pub struct AdtVariant {
    pub name: String,
    pub tag: i64,
    pub arity: usize,
}

#[derive(Debug, Clone)]
pub struct CtorInfo {
    pub adt_name: String,
    pub tag: i64,
    pub arity: usize,
}

#[derive(Debug, Clone)]
pub enum Item {
    Fun(Fun),
    /// Non-function `val` at module level
    Val { name: String, body: Expr },
}

#[derive(Debug, Clone)]
pub struct Fun {
    pub name: String,
    pub params: Vec<String>,
    pub body: Expr,
    /// True if this is the program entry `main`
    pub is_main: bool,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64, Span),
    Float(f64, Span),
    Bool(bool, Span),
    String(String, Span),
    Char(char, Span),
    Unit(Span),
    Var(String, Span),
    Let {
        name: String,
        value: Box<Expr>,
        body: Box<Expr>,
        mutable: bool,
    },
    Assign {
        name: String,
        value: Box<Expr>,
        span: Span,
    },
    Lambda {
        params: Vec<String>,
        body: Box<Expr>,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
        span: Span,
    },
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
        span: Span,
    },
    /// `for cond { body }` — while-style loop (Unit result).
    /// `step` runs after each body (and on `continue`) before re-checking `cond`.
    Loop {
        cond: Box<Expr>,
        body: Box<Expr>,
        step: Option<Box<Expr>>,
        span: Span,
    },
    Break(Span),
    Continue(Span),
    Seq {
        stmts: Vec<Expr>,
        span: Span,
    },
    /// Builtin recognized early
    BuiltinCall {
        name: Builtin,
        args: Vec<Expr>,
        span: Span,
    },
    /// Sum-type constructor: heap `[tag][payload…]`.
    AdtNew {
        adt_name: String,
        variant: String,
        tag: i64,
        args: Vec<Expr>,
        span: Span,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Println,
    PrintlnInt,
    PrintlnStr,
    ListLen,
    ListGet,
    ListSlice,
    ListAppend,
    /// `xs.concat(ys)` → new List.
    ListConcat,
    /// `m.contains(k)` / `s.contains(x)` — runtime dispatches on type_id.
    Contains,
    /// Immutable map upsert: `m.set(k, v)` → new Map.
    MapSet,
    /// Immutable map delete: `m.remove(k)` → new Map.
    MapRemove,
    MapKeys,
    MapValues,
    MapItems,
    Range,
    RangeInclusive,
    /// Format any scalar / String / Char as a heap String (interpolation).
    Show,
    /// ADT tag / payload access (match desugar).
    AdtTag,
    AdtField,
}

fn lower_expr(e: &lumia_syntax::Expr) -> Expr {
    match e {
        lumia_syntax::Expr::Int(n, s) => Expr::Int(*n, *s),
        lumia_syntax::Expr::Float(n, s) => Expr::Float(*n, *s),
        lumia_syntax::Expr::Bool(b, s) => Expr::Bool(*b, *s),
        lumia_syntax::Expr::String(t, s) => Expr::String(t.clone(), *s),
        lumia_syntax::Expr::Interp { parts, span } => lower_interp(parts, *span),
        lumia_syntax::Expr::Char(c, s) => Expr::Char(*c, *s),
        lumia_syntax::Expr::Ident(n, s) => {
            if let Some(c) = lookup_ctor(n) {
                if c.arity == 0 {
                    return Expr::AdtNew {
                        adt_name: c.adt_name,
                        variant: n.clone(),
                        tag: c.tag,
                        args: vec![],
                        span: *s,
                    };
                }
            }
            Expr::Var(n.clone(), *s)
        }
        lumia_syntax::Expr::Block { stmts, tail, span } => {
            lower_block(stmts, tail.as_deref(), *span)
        }
        lumia_syntax::Expr::Lambda { params, body, span } => Expr::Lambda {
            params: params.clone(),
            body: Box::new(lower_expr(body)),
            span: *span,
        },
        lumia_syntax::Expr::Call { callee, args, span } => lower_call(callee, args, *span),
        lumia_syntax::Expr::Binary {
            op,
            left,
            right,
            span,
        } => Expr::Binary {
            op: *op,
            left: Box::new(lower_expr(left)),
            right: Box::new(lower_expr(right)),
            span: *span,
        },
        lumia_syntax::Expr::Unary { op, expr, span } => Expr::Unary {
            op: *op,
            expr: Box::new(lower_expr(expr)),
            span: *span,
        },
        lumia_syntax::Expr::If {
            cond,
            then_branch,
            else_branch,
            span,
        } => Expr::If {
            cond: Box::new(lower_expr(cond)),
            then_branch: Box::new(lower_expr(then_branch)),
            else_branch: Box::new(
                else_branch
                    .as_ref()
                    .map(|e| lower_expr(e))
                    .unwrap_or(Expr::Unit(*span)),
            ),
            span: *span,
        },
        lumia_syntax::Expr::Pipeline { left, right, span } => match right.as_ref() {
            lumia_syntax::Expr::Call { callee, args, .. } => {
                let mut new_args = vec![lower_expr(left)];
                new_args.extend(args.iter().map(lower_expr));
                lower_call_from_parts(lower_expr(callee), new_args, *span)
            }
            other => {
                lower_call_from_parts(lower_expr(other), vec![lower_expr(left)], *span)
            }
        },
        lumia_syntax::Expr::Field { base, field, span } => {
            // `xs.len` → len(xs); product fields → adt_field; else call field(base)
            if field == "len" {
                Expr::BuiltinCall {
                    name: Builtin::ListLen,
                    args: vec![lower_expr(base)],
                    span: *span,
                }
            } else if let Some((_ty, idx)) = lookup_product_field(field) {
                Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![lower_expr(base), Expr::Int(idx as i64, *span)],
                    span: *span,
                }
            } else {
                Expr::Call {
                    callee: Box::new(Expr::Var(field.clone(), *span)),
                    args: vec![lower_expr(base)],
                    span: *span,
                }
            }
        }
        lumia_syntax::Expr::StructLit { name, fields, span } => {
            lower_struct_lit(name, fields, *span)
        }
        lumia_syntax::Expr::With { base, fields, span } => lower_with(base, fields, *span),
        lumia_syntax::Expr::TupleLit { elems, span } => Expr::AdtNew {
            adt_name: "__Tuple".into(),
            variant: String::new(),
            tag: 0,
            args: elems.iter().map(lower_expr).collect(),
            span: *span,
        },
        lumia_syntax::Expr::ListLit { elems, span } => Expr::Call {
            callee: Box::new(Expr::Var("listOf".into(), *span)),
            args: elems.iter().map(lower_expr).collect(),
            span: *span,
        },
        lumia_syntax::Expr::Match {
            scrutinee,
            arms,
            span,
        } => lower_match(scrutinee, arms, *span),
    }
}

fn lower_call(callee: &lumia_syntax::Expr, args: &[lumia_syntax::Expr], span: Span) -> Expr {
    if let lumia_syntax::Expr::Ident(name, _) = callee {
        if name == "println" {
            return Expr::BuiltinCall {
                name: Builtin::Println,
                args: args.iter().map(lower_expr).collect(),
                span,
            };
        }
    }
    if let lumia_syntax::Expr::Field { base, field, .. } = callee {
        let mut call_args = vec![lower_expr(base)];
        call_args.extend(args.iter().map(lower_expr));
        return lower_call_from_parts(Expr::Var(field.clone(), span), call_args, span);
    }
    lower_call_from_parts(
        lower_expr(callee),
        args.iter().map(lower_expr).collect(),
        span,
    )
}

fn lower_call_from_parts(callee: Expr, args: Vec<Expr>, span: Span) -> Expr {
    if let Expr::Var(name, _) = &callee {
        if let Some(c) = lookup_ctor(name) {
            if args.len() == c.arity {
                return Expr::AdtNew {
                    adt_name: c.adt_name,
                    variant: name.clone(),
                    tag: c.tag,
                    args,
                    span,
                };
            }
        }
        match name.as_str() {
            "len" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListLen,
                    args,
                    span,
                };
            }
            "get" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListGet,
                    args,
                    span,
                };
            }
            "map" if args.len() == 2 => {
                return lower_list_map(args[0].clone(), args[1].clone(), span);
            }
            "filter" if args.len() == 2 => {
                return lower_list_filter(args[0].clone(), args[1].clone(), span);
            }
            "fold" if args.len() == 3 => {
                return lower_list_fold(
                    args[0].clone(),
                    args[1].clone(),
                    args[2].clone(),
                    span,
                );
            }
            "contains" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::Contains,
                    args,
                    span,
                };
            }
            "set" if args.len() == 3 => {
                return Expr::BuiltinCall {
                    name: Builtin::MapSet,
                    args,
                    span,
                };
            }
            "remove" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::MapRemove,
                    args,
                    span,
                };
            }
            "keys" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::MapKeys,
                    args,
                    span,
                };
            }
            "values" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::MapValues,
                    args,
                    span,
                };
            }
            "items" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::MapItems,
                    args,
                    span,
                };
            }
            "slice" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListSlice,
                    args,
                    span,
                };
            }
            "concat" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListConcat,
                    args,
                    span,
                };
            }
            "range" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::Range,
                    args,
                    span,
                };
            }
            "rangeInclusive" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::RangeInclusive,
                    args,
                    span,
                };
            }
            "mapOf" => {
                // Flatten `k to v` → k, v for AllocMap flat layout
                let mut flat = vec![];
                for a in args {
                    if let Expr::Call {
                        callee: inner,
                        args: kv,
                        ..
                    } = &a
                    {
                        if let Expr::Var(n, _) = inner.as_ref() {
                            if n == "to" && kv.len() == 2 {
                                flat.push(kv[0].clone());
                                flat.push(kv[1].clone());
                                continue;
                            }
                        }
                    }
                    flat.push(a);
                }
                return Expr::Call {
                    callee: Box::new(callee),
                    args: flat,
                    span,
                };
            }
            _ => {}
        }
    }
    Expr::Call {
        callee: Box::new(callee),
        args,
        span,
    }
}

fn lower_struct_lit(name: &str, fields: &[(String, lumia_syntax::Expr)], span: Span) -> Expr {
    let Some(order) = lookup_product(name) else {
        // Unknown product — leave as call-shaped fallback
        return Expr::Call {
            callee: Box::new(Expr::Var(name.into(), span)),
            args: fields.iter().map(|(_, e)| lower_expr(e)).collect(),
            span,
        };
    };
    let mut by_name: HashMap<String, Expr> = HashMap::new();
    for (f, e) in fields {
        by_name.insert(f.clone(), lower_expr(e));
    }
    let mut args = Vec::with_capacity(order.len());
    for f in &order {
        if let Some(e) = by_name.remove(f) {
            args.push(e);
        } else {
            args.push(Expr::Int(0, span)); // missing field → 0 MVP
        }
    }
    Expr::AdtNew {
        adt_name: name.into(),
        variant: name.into(),
        tag: 0,
        args,
        span,
    }
}

fn lower_with(base: &lumia_syntax::Expr, fields: &[(String, lumia_syntax::Expr)], span: Span) -> Expr {
    // Infer product from first updated field name (MVP: unique field names).
    let Some((type_name, _)) = fields
        .first()
        .and_then(|(f, _)| lookup_product_field(f))
    else {
        return lower_expr(base);
    };
    let Some(order) = lookup_product(&type_name) else {
        return lower_expr(base);
    };
    let base_e = lower_expr(base);
    // Bind base once
        let tmp = format!("__with_{}", span.start.0);
    let mut by_name: HashMap<String, Expr> = HashMap::new();
    for (f, e) in fields {
        by_name.insert(f.clone(), lower_expr(e));
    }
    let mut args = Vec::with_capacity(order.len());
    for (i, f) in order.iter().enumerate() {
        if let Some(e) = by_name.remove(f) {
            args.push(e);
        } else {
            args.push(Expr::BuiltinCall {
                name: Builtin::AdtField,
                args: vec![
                    Expr::Var(tmp.clone(), span),
                    Expr::Int(i as i64, span),
                ],
                span,
            });
        }
    }
    Expr::Let {
        name: tmp,
        value: Box::new(base_e),
        body: Box::new(Expr::AdtNew {
            adt_name: type_name,
            variant: String::new(),
            tag: 0,
            args,
            span,
        }),
        mutable: false,
    }
}

/// Integer / wildcard / or-pattern match → nested `if` chain.
fn lower_match(
    scrutinee: &lumia_syntax::Expr,
    arms: &[lumia_syntax::MatchArm],
    span: Span,
) -> Expr {
    let scrut = "__match_s".to_string();
    let body = fold_match_arms(arms, &scrut, span);
    Expr::Let {
        name: scrut,
        value: Box::new(lower_expr(scrutinee)),
        body: Box::new(body),
        mutable: false,
    }
}

fn fold_match_arms(arms: &[lumia_syntax::MatchArm], scrut: &str, span: Span) -> Expr {
    if arms.is_empty() {
        return Expr::Unit(span);
    }
    let (arm, rest) = arms.split_first().unwrap();
    let (pat_cond, binds) = pattern_cond(&arm.pattern, scrut, span);
    let cond = if let Some(g) = &arm.guard {
        // Pattern bindings must be in scope for the guard (`x if x > 0`).
        let mut guard_e = lower_expr(g);
        for (name, val) in binds.iter().rev() {
            guard_e = Expr::Let {
                name: name.clone(),
                value: Box::new(val.clone()),
                body: Box::new(guard_e),
                mutable: false,
            };
        }
        Expr::Binary {
            op: BinOp::And,
            left: Box::new(pat_cond),
            right: Box::new(guard_e),
            span,
        }
    } else {
        pat_cond
    };
    let mut then_body = lower_expr(&arm.body);
    for (name, val) in binds.into_iter().rev() {
        then_body = Expr::Let {
            name,
            value: Box::new(val),
            body: Box::new(then_body),
            mutable: false,
        };
    }
    // Last arm: no else (MVP assumes match is exhaustive / last arm is catch-all).
    if rest.is_empty() {
        return then_body;
    }
    let else_body = fold_match_arms(rest, scrut, span);
    Expr::If {
        cond: Box::new(cond),
        then_branch: Box::new(then_body),
        else_branch: Box::new(else_body),
        span,
    }
}

fn pattern_cond(pat: &Pattern, scrut: &str, span: Span) -> (Expr, Vec<(String, Expr)>) {
    match pat {
        Pattern::Wildcard(_) => (Expr::Bool(true, span), vec![]),
        Pattern::Int(n, s) => (
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(Expr::Var(scrut.into(), span)),
                right: Box::new(Expr::Int(*n, *s)),
                span,
            },
            vec![],
        ),
        Pattern::Ident(name, _) => {
            if let Some(c) = lookup_ctor(name) {
                if c.arity == 0 {
                    let tag = Expr::BuiltinCall {
                        name: Builtin::AdtTag,
                        args: vec![Expr::Var(scrut.into(), span)],
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
            (
                Expr::Bool(true, span),
                vec![(name.clone(), Expr::Var(scrut.into(), span))],
            )
        }
        Pattern::Or(pats, _) => {
            let mut cond = Expr::Bool(false, span);
            let mut binds = vec![];
            for p in pats {
                let (c, b) = pattern_cond(p, scrut, span);
                if !b.is_empty() && binds.is_empty() {
                    binds = b;
                }
                cond = Expr::Binary {
                    op: BinOp::Or,
                    left: Box::new(cond),
                    right: Box::new(c),
                    span,
                };
            }
            (cond, binds)
        }
        Pattern::Variant { name, args, .. } => {
            let Some(c) = lookup_ctor(name) else {
                return (Expr::Bool(false, span), vec![]);
            };
            let tag = Expr::BuiltinCall {
                name: Builtin::AdtTag,
                args: vec![Expr::Var(scrut.into(), span)],
                span,
            };
            let mut cond = Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(tag),
                right: Box::new(Expr::Int(c.tag, span)),
                span,
            };
            let mut binds = vec![];
            for (i, ep) in args.iter().enumerate() {
                let field = Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![
                        Expr::Var(scrut.into(), span),
                        Expr::Int(i as i64, span),
                    ],
                    span,
                };
                match ep {
                    Pattern::Ident(n, _) => binds.push((n.clone(), field)),
                    Pattern::Wildcard(_) => {}
                    Pattern::Int(n, s) => {
                        cond = Expr::Binary {
                            op: BinOp::And,
                            left: Box::new(cond),
                            right: Box::new(Expr::Binary {
                                op: BinOp::Eq,
                                left: Box::new(field),
                                right: Box::new(Expr::Int(*n, *s)),
                                span,
                            }),
                            span,
                        };
                    }
                    _ => {}
                }
            }
            (cond, binds)
        }
        Pattern::Struct { name, fields, .. } => {
            let Some(order) = lookup_product(name) else {
                return (Expr::Bool(false, span), vec![]);
            };
            let mut cond = Expr::Bool(true, span);
            let mut binds = vec![];
            for (fname, sub) in fields {
                let Some(idx) = order.iter().position(|f| f == fname) else {
                    continue;
                };
                let field = Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![
                        Expr::Var(scrut.into(), span),
                        Expr::Int(idx as i64, span),
                    ],
                    span,
                };
                match sub {
                    Pattern::Ident(n, _) => binds.push((n.clone(), field)),
                    Pattern::Wildcard(_) => {}
                    Pattern::Int(n, s) => {
                        cond = Expr::Binary {
                            op: BinOp::And,
                            left: Box::new(cond),
                            right: Box::new(Expr::Binary {
                                op: BinOp::Eq,
                                left: Box::new(field),
                                right: Box::new(Expr::Int(*n, *s)),
                                span,
                            }),
                            span,
                        };
                    }
                    _ => {}
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
                    args: vec![
                        Expr::Var(scrut.into(), span),
                        Expr::Int(i as i64, span),
                    ],
                    span,
                };
                match ep {
                    Pattern::Ident(n, _) => binds.push((n.clone(), field)),
                    Pattern::Wildcard(_) => {}
                    Pattern::Int(n, s) => {
                        cond = Expr::Binary {
                            op: BinOp::And,
                            left: Box::new(cond),
                            right: Box::new(Expr::Binary {
                                op: BinOp::Eq,
                                left: Box::new(field),
                                right: Box::new(Expr::Int(*n, *s)),
                                span,
                            }),
                            span,
                        };
                    }
                    _ => {}
                }
            }
            (cond, binds)
        }
        Pattern::List { elems, rest, .. } => {
            let len = Expr::BuiltinCall {
                name: Builtin::ListLen,
                args: vec![Expr::Var(scrut.into(), span)],
                span,
            };
            let min = elems.len() as i64;
            let cond = if rest.is_some() {
                // len >= min
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
                    args: vec![
                        Expr::Var(scrut.into(), span),
                        Expr::Int(i as i64, span),
                    ],
                    span,
                };
                match ep {
                    Pattern::Ident(name, _) => binds.push((name.clone(), get)),
                    Pattern::Wildcard(_) => {}
                    Pattern::Int(n, s) => {
                        // refine cond: get(i) == n
                        // fold into cond via And — handled by wrapping
                        let eq = Expr::Binary {
                            op: BinOp::Eq,
                            left: Box::new(get),
                            right: Box::new(Expr::Int(*n, *s)),
                            span,
                        };
                        // We'll And later — push as synthetic via cond update below
                        binds.push((format!("__pat_eq_{i}"), eq));
                    }
                    _ => {}
                }
            }
            // Convert Int equality "binds" into cond Ands
            let mut cond = cond;
            let mut real_binds = vec![];
            for (name, val) in binds {
                if name.starts_with("__pat_eq_") {
                    cond = Expr::Binary {
                        op: BinOp::And,
                        left: Box::new(cond),
                        right: Box::new(val),
                        span,
                    };
                } else {
                    real_binds.push((name, val));
                }
            }
            if let Some(rname) = rest {
                let slice = Expr::BuiltinCall {
                    name: Builtin::ListSlice,
                    args: vec![
                        Expr::Var(scrut.into(), span),
                        Expr::Int(min, span),
                    ],
                    span,
                };
                real_binds.push((rname.clone(), slice));
            }
            (cond, real_binds)
        }
    }
}

fn lower_block(
    stmts: &[lumia_syntax::Stmt],
    tail: Option<&lumia_syntax::Expr>,
    span: Span,
) -> Expr {
    fn fold(stmts: &[lumia_syntax::Stmt], tail: Option<&lumia_syntax::Expr>, span: Span) -> Expr {
        if stmts.is_empty() {
            return match tail {
                Some(e) => lower_expr(e),
                None => Expr::Unit(span),
            };
        }
        let (first, rest) = stmts.split_first().unwrap();
        match first {
            lumia_syntax::Stmt::Val { name, expr, span: _s } => Expr::Let {
                name: name.clone(),
                value: Box::new(lower_expr(expr)),
                body: Box::new(fold(rest, tail, span)),
                mutable: false,
            },
            lumia_syntax::Stmt::Var { name, expr, span: s } => {
                let _ = s;
                Expr::Let {
                    name: name.clone(),
                    value: Box::new(lower_expr(expr)),
                    body: Box::new(fold(rest, tail, span)),
                    mutable: true,
                }
            }
            lumia_syntax::Stmt::Assign { name, expr, span: s } => {
                let assign = Expr::Assign {
                    name: name.clone(),
                    value: Box::new(lower_expr(expr)),
                    span: *s,
                };
                let rest_e = fold(rest, tail, span);
                Expr::Seq {
                    stmts: vec![assign, rest_e],
                    span: *s,
                }
            }
            lumia_syntax::Stmt::Expr(e) => {
                let e = lower_expr(e);
                let rest_e = fold(rest, tail, span);
                Expr::Seq {
                    stmts: vec![e, rest_e],
                    span,
                }
            }
            lumia_syntax::Stmt::Break(s) => {
                let rest_e = fold(rest, tail, span);
                Expr::Seq {
                    stmts: vec![Expr::Break(*s), rest_e],
                    span: *s,
                }
            }
            lumia_syntax::Stmt::Continue(s) => {
                let rest_e = fold(rest, tail, span);
                Expr::Seq {
                    stmts: vec![Expr::Continue(*s), rest_e],
                    span: *s,
                }
            }
            lumia_syntax::Stmt::ForCond { cond, body, span: s } => {
                let loop_e = Expr::Loop {
                    cond: Box::new(lower_expr(cond)),
                    body: Box::new(lower_expr(body)),
                    step: None,
                    span: *s,
                };
                let rest_e = fold(rest, tail, span);
                Expr::Seq {
                    stmts: vec![loop_e, rest_e],
                    span: *s,
                }
            }
            lumia_syntax::Stmt::ForIn {
                binding,
                iter,
                body,
                span: s,
            } => {
                let loop_e = lower_for_in(binding, iter, body, *s);
                let rest_e = fold(rest, tail, span);
                Expr::Seq {
                    stmts: vec![loop_e, rest_e],
                    span: *s,
                }
            }
        }
    }
    fold(stmts, tail, span)
}

/// `"a${x}b"` → `"a".concat(show(x)).concat("b")` (via builtins).
fn lower_interp(parts: &[lumia_syntax::InterpPart], span: Span) -> Expr {
    let mut pieces: Vec<Expr> = Vec::new();
    for p in parts {
        match p {
            lumia_syntax::InterpPart::Lit(s) => {
                pieces.push(Expr::String(s.clone(), span));
            }
            lumia_syntax::InterpPart::Expr(e) => {
                pieces.push(Expr::BuiltinCall {
                    name: Builtin::Show,
                    args: vec![lower_expr(e)],
                    span,
                });
            }
        }
    }
    if pieces.is_empty() {
        return Expr::String(String::new(), span);
    }
    let mut acc = pieces.remove(0);
    for p in pieces {
        acc = Expr::BuiltinCall {
            name: Builtin::ListConcat,
            args: vec![acc, p],
            span,
        };
    }
    acc
}

/// `xs.map(f)` → accumulate via append.
/// Literal `{ x -> e }` is inlined; any other function value is called each step.
fn lower_list_map(list: Expr, f: Expr, span: Span) -> Expr {
    match &f {
        Expr::Lambda { params, body, .. } if params.len() == 1 => {
            lower_list_map_inline(list, params[0].clone(), *body.clone(), span)
        }
        _ => {
            let f_name = format!("__map_f_{}", span.start.0);
            let x = format!("__map_x_{}", span.start.0);
            lower_list_map_call(list, f, f_name, x, span)
        }
    }
}

fn lower_list_map_inline(list: Expr, x: String, body: Expr, span: Span) -> Expr {
    let acc = format!("__map_acc_{}", span.start.0);
    let xs = format!("__map_xs_{}", span.start.0);
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListAppend,
            args: vec![Expr::Var(acc.clone(), span), body],
            span,
        }),
        span,
    };
    Expr::Let {
        name: xs.clone(),
        value: Box::new(list),
        body: Box::new(Expr::Let {
            name: acc.clone(),
            value: Box::new(Expr::Call {
                callee: Box::new(Expr::Var("listOf".into(), span)),
                args: vec![],
                span,
            }),
            body: Box::new(Expr::Seq {
                stmts: vec![
                    list_for_in(&x, Expr::Var(xs, span), step, span),
                    Expr::Var(acc, span),
                ],
                span,
            }),
            mutable: true,
        }),
        mutable: false,
    }
}

fn lower_list_map_call(list: Expr, f: Expr, f_name: String, x: String, span: Span) -> Expr {
    let acc = format!("__map_acc_{}", span.start.0);
    let xs = format!("__map_xs_{}", span.start.0);
    let mapped = Expr::Call {
        callee: Box::new(Expr::Var(f_name.clone(), span)),
        args: vec![Expr::Var(x.clone(), span)],
        span,
    };
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListAppend,
            args: vec![Expr::Var(acc.clone(), span), mapped],
            span,
        }),
        span,
    };
    Expr::Let {
        name: f_name,
        value: Box::new(f),
        body: Box::new(Expr::Let {
            name: xs.clone(),
            value: Box::new(list),
            body: Box::new(Expr::Let {
                name: acc.clone(),
                value: Box::new(Expr::Call {
                    callee: Box::new(Expr::Var("listOf".into(), span)),
                    args: vec![],
                    span,
                }),
                body: Box::new(Expr::Seq {
                    stmts: vec![
                        list_for_in(&x, Expr::Var(xs, span), step, span),
                        Expr::Var(acc, span),
                    ],
                    span,
                }),
                mutable: true,
            }),
            mutable: false,
        }),
        mutable: false,
    }
}

fn lower_list_filter(list: Expr, f: Expr, span: Span) -> Expr {
    match &f {
        Expr::Lambda { params, body, .. } if params.len() == 1 => {
            return lower_list_filter_inline(list, params[0].clone(), *body.clone(), span);
        }
        _ => {}
    }
    let f_name = format!("__flt_f_{}", span.start.0);
    let x = format!("__flt_x_{}", span.start.0);
    lower_list_filter_call(list, f, f_name, x, span)
}

fn lower_list_filter_inline(list: Expr, x: String, body: Expr, span: Span) -> Expr {
    let acc = format!("__flt_acc_{}", span.start.0);
    let xs = format!("__flt_xs_{}", span.start.0);
    let append = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListAppend,
            args: vec![
                Expr::Var(acc.clone(), span),
                Expr::Var(x.clone(), span),
            ],
            span,
        }),
        span,
    };
    let step = Expr::If {
        cond: Box::new(body),
        then_branch: Box::new(append),
        else_branch: Box::new(Expr::Unit(span)),
        span,
    };
    Expr::Let {
        name: xs.clone(),
        value: Box::new(list),
        body: Box::new(Expr::Let {
            name: acc.clone(),
            value: Box::new(Expr::Call {
                callee: Box::new(Expr::Var("listOf".into(), span)),
                args: vec![],
                span,
            }),
            body: Box::new(Expr::Seq {
                stmts: vec![
                    list_for_in(&x, Expr::Var(xs, span), step, span),
                    Expr::Var(acc, span),
                ],
                span,
            }),
            mutable: true,
        }),
        mutable: false,
    }
}

fn lower_list_filter_call(list: Expr, f: Expr, f_name: String, x: String, span: Span) -> Expr {
    let acc = format!("__flt_acc_{}", span.start.0);
    let xs = format!("__flt_xs_{}", span.start.0);
    let pred = Expr::Call {
        callee: Box::new(Expr::Var(f_name.clone(), span)),
        args: vec![Expr::Var(x.clone(), span)],
        span,
    };
    let append = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListAppend,
            args: vec![
                Expr::Var(acc.clone(), span),
                Expr::Var(x.clone(), span),
            ],
            span,
        }),
        span,
    };
    let step = Expr::If {
        cond: Box::new(pred),
        then_branch: Box::new(append),
        else_branch: Box::new(Expr::Unit(span)),
        span,
    };
    Expr::Let {
        name: f_name,
        value: Box::new(f),
        body: Box::new(Expr::Let {
            name: xs.clone(),
            value: Box::new(list),
            body: Box::new(Expr::Let {
                name: acc.clone(),
                value: Box::new(Expr::Call {
                    callee: Box::new(Expr::Var("listOf".into(), span)),
                    args: vec![],
                    span,
                }),
                body: Box::new(Expr::Seq {
                    stmts: vec![
                        list_for_in(&x, Expr::Var(xs, span), step, span),
                        Expr::Var(acc, span),
                    ],
                    span,
                }),
                mutable: true,
            }),
            mutable: false,
        }),
        mutable: false,
    }
}

fn lower_list_fold(list: Expr, init: Expr, f: Expr, span: Span) -> Expr {
    match &f {
        Expr::Lambda { params, body, .. } if params.len() == 2 => {
            return lower_list_fold_inline(
                list,
                init,
                params[0].clone(),
                params[1].clone(),
                *body.clone(),
                span,
            );
        }
        _ => {}
    }
    let f_name = format!("__fold_f_{}", span.start.0);
    let x = format!("__fold_x_{}", span.start.0);
    let acc = format!("__fold_acc_{}", span.start.0);
    lower_list_fold_call(list, init, f, f_name, acc, x, span)
}

fn lower_list_fold_inline(
    list: Expr,
    init: Expr,
    acc: String,
    x: String,
    body: Expr,
    span: Span,
) -> Expr {
    let xs = format!("__fold_xs_{}", span.start.0);
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(body),
        span,
    };
    Expr::Let {
        name: xs.clone(),
        value: Box::new(list),
        body: Box::new(Expr::Let {
            name: acc.clone(),
            value: Box::new(init),
            body: Box::new(Expr::Seq {
                stmts: vec![
                    list_for_in(&x, Expr::Var(xs, span), step, span),
                    Expr::Var(acc, span),
                ],
                span,
            }),
            mutable: true,
        }),
        mutable: false,
    }
}

fn lower_list_fold_call(
    list: Expr,
    init: Expr,
    f: Expr,
    f_name: String,
    acc: String,
    x: String,
    span: Span,
) -> Expr {
    let xs = format!("__fold_xs_{}", span.start.0);
    let applied = Expr::Call {
        callee: Box::new(Expr::Var(f_name.clone(), span)),
        args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
        span,
    };
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(applied),
        span,
    };
    Expr::Let {
        name: f_name,
        value: Box::new(f),
        body: Box::new(Expr::Let {
            name: xs.clone(),
            value: Box::new(list),
            body: Box::new(Expr::Let {
                name: acc.clone(),
                value: Box::new(init),
                body: Box::new(Expr::Seq {
                    stmts: vec![
                        list_for_in(&x, Expr::Var(xs, span), step, span),
                        Expr::Var(acc, span),
                    ],
                    span,
                }),
                mutable: true,
            }),
            mutable: false,
        }),
        mutable: false,
    }
}

/// `for x in range(a,b)` → counter loop; otherwise list len/get loop.
fn lower_for_in(
    binding: &str,
    iter: &lumia_syntax::Expr,
    body: &lumia_syntax::Expr,
    span: Span,
) -> Expr {
    let lowered_iter = lower_expr(iter);
    if let Expr::BuiltinCall { name, args, .. } = &lowered_iter {
        if matches!(name, Builtin::Range | Builtin::RangeInclusive) && args.len() == 2 {
            let inclusive = matches!(name, Builtin::RangeInclusive);
            let start = args[0].clone();
            let end = args[1].clone();
            return counter_for_in(binding, start, end, inclusive, lower_expr(body), span);
        }
    }
    list_for_in(binding, lowered_iter, lower_expr(body), span)
}

fn counter_for_in(
    binding: &str,
    start: Expr,
    end: Expr,
    inclusive: bool,
    body: Expr,
    span: Span,
) -> Expr {
    let i = "__i".to_string();
    let cmp = if inclusive { BinOp::Le } else { BinOp::Lt };
    let cond = Expr::Binary {
        op: cmp,
        left: Box::new(Expr::Var(i.clone(), span)),
        right: Box::new(end),
        span,
    };
    let step = Expr::Assign {
        name: i.clone(),
        value: Box::new(Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::Var(i.clone(), span)),
            right: Box::new(Expr::Int(1, span)),
            span,
        }),
        span,
    };
    let body = Expr::Let {
        name: binding.into(),
        value: Box::new(Expr::Var(i.clone(), span)),
        body: Box::new(body),
        mutable: false,
    };
    Expr::Let {
        name: i,
        value: Box::new(start),
        body: Box::new(Expr::Loop {
            cond: Box::new(cond),
            body: Box::new(body),
            step: Some(Box::new(step)),
            span,
        }),
        mutable: true,
    }
}

fn list_for_in(binding: &str, list: Expr, body: Expr, span: Span) -> Expr {
    let xs = format!("__xs_{}", span.start.0);
    let i = format!("__i_{}", span.start.0);
    let n = format!("__n_{}", span.start.0);
    let cond = Expr::Binary {
        op: BinOp::Lt,
        left: Box::new(Expr::Var(i.clone(), span)),
        right: Box::new(Expr::Var(n.clone(), span)),
        span,
    };
    let get = Expr::BuiltinCall {
        name: Builtin::ListGet,
        args: vec![
            Expr::Var(xs.clone(), span),
            Expr::Var(i.clone(), span),
        ],
        span,
    };
    let step = Expr::Assign {
        name: i.clone(),
        value: Box::new(Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::Var(i.clone(), span)),
            right: Box::new(Expr::Int(1, span)),
            span,
        }),
        span,
    };
    let body = Expr::Let {
        name: binding.into(),
        value: Box::new(get),
        body: Box::new(body),
        mutable: false,
    };
    let loop_e = Expr::Loop {
        cond: Box::new(cond),
        body: Box::new(body),
        step: Some(Box::new(step)),
        span,
    };
    Expr::Let {
        name: xs.clone(),
        value: Box::new(list),
        body: Box::new(Expr::Let {
            name: n,
            value: Box::new(Expr::BuiltinCall {
                name: Builtin::ListLen,
                args: vec![Expr::Var(xs, span)],
                span,
            }),
            body: Box::new(Expr::Let {
                name: i,
                value: Box::new(Expr::Int(0, span)),
                body: Box::new(loop_e),
                mutable: true,
            }),
            mutable: false,
        }),
        mutable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::lower_module;
    use lumia_syntax::parse_module;

    #[test]
    fn exhaustiveness_rejects_missing_variant() {
        let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
    o match {
        None -> { 0 }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err();
        assert!(err.contains("non-exhaustive"), "{err}");
        assert!(err.contains("Some"), "{err}");
    }

    #[test]
    fn exhaustiveness_accepts_full_option() {
        let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
    o match {
        None -> { 0 }
        Some(n) -> { n }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        assert!(lower_module(&ast).is_ok());
    }
}
