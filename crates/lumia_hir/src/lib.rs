//! High-level IR — named bindings after light desugaring from syntax AST.

use lumia_syntax::{BinOp, Pattern, Span, UnOp, VariantFields};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

thread_local! {
    static CTORS: RefCell<HashMap<String, CtorInfo>> = RefCell::new(HashMap::new());
    /// Product field name → (type name, field index). Names shared by ≥2 products
    /// are omitted here and listed in `AMBIGUOUS_PRODUCT_FIELDS`.
    static PRODUCT_FIELDS: RefCell<HashMap<String, (String, usize)>> = RefCell::new(HashMap::new());
    static AMBIGUOUS_PRODUCT_FIELDS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static PRODUCTS: RefCell<HashMap<String, Vec<String>>> = RefCell::new(HashMap::new());
    static LOWER_ERR: RefCell<Option<LowerError>> = const { RefCell::new(None) };
    /// Capture-free top-level function names (safe FunRef for parallel map).
    static TOPLEVEL_FUNS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Lowering / exhaustiveness failure with optional source span.
#[derive(Debug, Clone)]
pub struct LowerError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LowerError {}

/// Short-circuit `and` as `if left { right } else { false }` (avoids OOB field/get).
fn short_and(left: Expr, right: Expr, span: Span) -> Expr {
    Expr::If {
        cond: Box::new(left),
        then_branch: Box::new(right),
        else_branch: Box::new(Expr::Bool(false, span)),
        span,
    }
}

fn short_or(left: Expr, right: Expr, span: Span) -> Expr {
    Expr::If {
        cond: Box::new(left),
        then_branch: Box::new(Expr::Bool(true, span)),
        else_branch: Box::new(right),
        span,
    }
}

fn set_lower_err(msg: String, span: Span) {
    LOWER_ERR.with(|slot| {
        if slot.borrow().is_none() {
            *slot.borrow_mut() = Some(LowerError {
                message: msg,
                span,
            });
        }
    });
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
    ambiguous: HashSet<String>,
    f: impl FnOnce() -> R,
) -> R {
    PRODUCTS.with(|p| {
        PRODUCT_FIELDS.with(|pf| {
            AMBIGUOUS_PRODUCT_FIELDS.with(|af| {
                *p.borrow_mut() = products;
                *pf.borrow_mut() = fields;
                *af.borrow_mut() = ambiguous;
                let r = f();
                p.borrow_mut().clear();
                pf.borrow_mut().clear();
                af.borrow_mut().clear();
                r
            })
        })
    })
}

fn lookup_ctor(name: &str) -> Option<CtorInfo> {
    CTORS.with(|c| c.borrow().get(name).cloned())
}

fn lookup_product_field(name: &str) -> Option<(String, usize)> {
    PRODUCT_FIELDS.with(|c| c.borrow().get(name).cloned())
}

fn is_ambiguous_product_field(name: &str) -> bool {
    AMBIGUOUS_PRODUCT_FIELDS.with(|c| c.borrow().contains(name))
}

fn lookup_product(name: &str) -> Option<Vec<String>> {
    PRODUCTS.with(|c| c.borrow().get(name).cloned())
}

/// If `name` is absent, register a sum type with the given `(variant, arity)` list (tags = index).
fn ensure_prelude_adt(
    adts: &mut Vec<AdtDef>,
    ctors: &mut HashMap<String, CtorInfo>,
    name: &str,
    variants: &[(&str, usize)],
) {
    if adts.iter().any(|a| a.name == name) {
        return;
    }
    let mut vs = Vec::new();
    for (tag, (vname, arity)) in variants.iter().enumerate() {
        if ctors.contains_key(*vname) {
            // User already bound this ctor name to another ADT — skip prelude.
            return;
        }
        ctors.insert(
            (*vname).into(),
            CtorInfo {
                adt_name: name.into(),
                tag: tag as i64,
                arity: *arity,
            },
        );
        vs.push(AdtVariant {
            name: (*vname).into(),
            tag: tag as i64,
            arity: *arity,
        });
    }
    adts.push(AdtDef {
        name: name.into(),
        variants: vs,
    });
}

/// Lower syntax AST → HIR with desugaring.
pub fn lower_module(m: &lumia_syntax::Module) -> Result<Module, LowerError> {
    let mut adts = Vec::new();
    let mut products = Vec::new();
    let mut ctors = HashMap::new();
    let mut product_map = HashMap::new();
    let mut product_fields = HashMap::new();
    let mut ambiguous_product_fields: HashSet<String> = HashSet::new();
    // Builtin trait prerequisites (DESIGN §3.6); user `trait` decls may extend.
    let mut trait_requires: HashMap<String, Vec<String>> = HashMap::from([
        ("Ord".into(), vec!["Eq".into()]),
        ("Eq".into(), vec![]),
        ("Hash".into(), vec![]),
        ("Show".into(), vec![]),
    ]);
    let mut instances: HashSet<(String, String)> = HashSet::new();
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
                        match product_fields.get(f) {
                            Some((prev, _)) if prev != &t.name => {
                                // Same field name on two products → `with { f = … }` is ambiguous.
                                ambiguous_product_fields.insert(f.clone());
                            }
                            None => {
                                product_fields.insert(f.clone(), (t.name.clone(), i));
                            }
                            Some(_) => {} // same type re-decl shouldn't happen
                        }
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

    // Prelude ADTs: inject Option / Result when the module does not declare them.
    ensure_prelude_adt(
        &mut adts,
        &mut ctors,
        "Option",
        &[("Some", 1), ("None", 0)],
    );
    ensure_prelude_adt(
        &mut adts,
        &mut ctors,
        "Result",
        &[("Ok", 1), ("Err", 1)],
    );

    for item in &m.items {
        match item {
            lumia_syntax::Item::Trait(t) => {
                trait_requires.insert(t.name.clone(), t.requires.clone());
            }
            lumia_syntax::Item::Instance(i) => {
                let known_type = product_map.contains_key(&i.type_name)
                    || adts.iter().any(|a| a.name == i.type_name);
                if !known_type {
                    return Err(LowerError {
                        message: format!(
                            "instance {} for {}: unknown type `{}`",
                            i.trait_name, i.type_name, i.type_name
                        ),
                        span: i.span,
                    });
                }
                if !trait_requires.contains_key(&i.trait_name) {
                    return Err(LowerError {
                        message: format!(
                            "instance for unknown trait `{}` (declare `trait {} {{ }}` first)",
                            i.trait_name, i.trait_name
                        ),
                        span: i.span,
                    });
                }
                instances.insert((i.trait_name.clone(), i.type_name.clone()));
            }
            _ => {}
        }
    }
    for (tr, ty) in &instances {
        if let Some(reqs) = trait_requires.get(tr) {
            for req in reqs {
                if !instances.contains(&(req.clone(), ty.clone())) {
                    let span = m
                        .items
                        .iter()
                        .find_map(|it| match it {
                            lumia_syntax::Item::Instance(i)
                                if i.trait_name == *tr && i.type_name == *ty =>
                            {
                                Some(i.span)
                            }
                            _ => None,
                        })
                        .unwrap_or_else(lumia_syntax::Span::dummy);
                    return Err(LowerError {
                        message: format!(
                            "`instance {tr} for {ty}` requires `instance {req} for {ty}`"
                        ),
                        span,
                    });
                }
            }
        }
    }

    check_module_matches(m, &ctors, &adts, &product_map)?;

    // Pre-register top-level function names for `--parallel` FunRef maps.
    TOPLEVEL_FUNS.with(|t| {
        let mut set = t.borrow_mut();
        set.clear();
        for item in &m.items {
            match item {
                lumia_syntax::Item::Val(v) => {
                    let is_fun = v.params.is_some()
                        || matches!(v.body, lumia_syntax::Expr::Lambda { .. });
                    if is_fun {
                        set.insert(v.name.clone());
                    }
                }
                lumia_syntax::Item::Foreign(f) => {
                    set.insert(f.name.clone());
                }
                _ => {}
            }
        }
    });

    LOWER_ERR.with(|e| *e.borrow_mut() = None);
    for f in &ambiguous_product_fields {
        product_fields.remove(f);
    }
    let module = with_ctors(ctors, || {
        with_products(product_map, product_fields, ambiguous_product_fields, || {
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
                                    external: None,
                                    foreign_sig: None,
                                    foreign_pure: false,
                                }));
                            }
                            other => {
                                if v.name == "main" {
                                    items.push(Item::Fun(Fun {
                                        name: "main".into(),
                                        params: vec![],
                                        body: other,
                                        is_main: true,
                                        external: None,
                                        foreign_sig: None,
                                    foreign_pure: false,
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
                    lumia_syntax::Item::Type(_)
                    | lumia_syntax::Item::Trait(_)
                    | lumia_syntax::Item::Instance(_) => {}
                    lumia_syntax::Item::Foreign(f) => {
                        if f.abi != "C" {
                            set_lower_err(
                                format!("unsupported foreign ABI `{}` (only \"C\")", f.abi),
                                f.span,
                            );
                        }
                        let params: Vec<String> = f.params.iter().map(|(n, _)| n.clone()).collect();
                        let param_tys: Vec<String> =
                            f.params.iter().map(|(_, t)| t.clone()).collect();
                        items.push(Item::Fun(Fun {
                            name: f.name.clone(),
                            params,
                            body: Expr::Unit(f.span),
                            is_main: false,
                            external: Some(f.name.clone()),
                            foreign_sig: Some((param_tys, f.ret.clone())),
                            foreign_pure: f.is_pure,
                        }));
                    }
                }
            }
            Module {
                name: m.name.clone(),
                items,
                adts,
                products,
                instances,
            }
        })
    });
    if let Some(err) = LOWER_ERR.with(|e| e.borrow_mut().take()) {
        return Err(err);
    }
    Ok(module)
}

fn check_module_matches(
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

fn check_expr_matches(
    e: &lumia_syntax::Expr,
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
    products: &HashMap<String, Vec<String>>,
) -> Result<(), LowerError> {
    use lumia_syntax::Expr as S;
    match e {
        S::Match { arms, span, .. } => {
            check_match_exhaustiveness(arms, ctors, adts, products).map_err(|message| {
                LowerError {
                    message,
                    span: *span,
                }
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
                    message:
                        "subjectless `match { }` used as expression requires a `_` arm".into(),
                    span: *span,
                });
            }
            // `_` must be last (Kotlin else is last)
            if let Some((last, rest)) = arms.split_last() {
                if last.cond.is_some() || rest.iter().any(|a| a.cond.is_none()) {
                    return Err(LowerError {
                        message:
                            "subjectless `match { }`: `_` arm must be last and unique".into(),
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
                    | lumia_syntax::Stmt::ForCond { cond: iter, body, .. } => {
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
        S::Unary { expr, .. } | S::Field { base: expr, .. } => {
            check_expr_matches(expr, ctors, adts, products)?
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

fn check_match_exhaustiveness(
    arms: &[lumia_syntax::MatchArm],
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
    products: &HashMap<String, Vec<String>>,
) -> Result<(), String> {
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
fn coverage_catch_all(pat: &Pattern, ctors: &HashMap<String, CtorInfo>) -> bool {
    match pat {
        Pattern::Wildcard(_) => true,
        Pattern::Ident(name, _) => !ctors.get(name).is_some_and(|c| c.arity == 0),
        Pattern::Or(ps, _) => ps.iter().any(|p| coverage_catch_all(p, ctors)),
        Pattern::Struct { fields, .. } => fields
            .iter()
            .all(|(_, sub)| coverage_catch_all(sub, ctors)),
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
fn check_pats_cover(
    pats: &[&Pattern],
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
    products: &HashMap<String, Vec<String>>,
    path: &str,
) -> Result<(), String> {
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
        return Err(format!(
            "non-exhaustive match on {where_}: no covering arm (empty match or only guarded arms)"
        ));
    }
    if flat.iter().any(|p| coverage_catch_all(p, ctors)) {
        return Ok(());
    }

    let mut covered: HashMap<String, HashSet<i64>> = HashMap::new();
    let mut ctor_args: HashMap<String, Vec<Vec<&Pattern>>> = HashMap::new();
    let mut product_fields: HashMap<String, HashMap<String, Vec<&Pattern>>> = HashMap::new();
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
                        covered
                            .entry(c.adt_name.clone())
                            .or_default()
                            .insert(c.tag);
                    }
                }
            }
            Pattern::Variant { name, args, .. } => {
                if let Some(c) = ctors.get(name) {
                    saw_sum = true;
                    covered
                        .entry(c.adt_name.clone())
                        .or_default()
                        .insert(c.tag);
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
                return Err(format!(
                    "non-exhaustive match on {where_}: missing variant(s) {}",
                    missing.join(", ")
                ));
            }
            for v in &def.variants {
                if v.arity == 0 {
                    continue;
                }
                let Some(rows) = ctor_args.get(&v.name) else {
                    continue;
                };
                for slot in 0..v.arity {
                    let col: Vec<&Pattern> = rows
                        .iter()
                        .filter_map(|r| r.get(slot).copied())
                        .collect();
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
    if !saw_sum
        && !saw_product
        && saw_bool
        && !saw_int
        && !saw_float
        && !saw_list
        && !saw_open_lit
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
        return Err(format!(
            "non-exhaustive match on {where_}: missing {missing} (or `_`)"
        ));
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
                return Err(format!(
                    "non-exhaustive match on List (in {where_}): add `[]` / `[..rest]` arms or `_`"
                ));
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
            return Err(format!(
                "non-exhaustive match on Int (in {where_}): integer literals need a `_` arm"
            ));
        } else if saw_float {
            return Err(format!(
                "non-exhaustive match on Float (in {where_}): float literals need a `_` arm"
            ));
        } else if saw_open_lit {
            return Err(format!(
                "non-exhaustive match on Char/String (in {where_}): literal arms need a `_` arm"
            ));
        }
    }

    Ok(())
}

/// `[]` covers length 0; `[e0,…,ek-1, ..rest]` covers all lengths `>= k`.
/// Together they must cover `0..`.
fn list_patterns_exhaustive(pats: &[&Pattern]) -> bool {
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

#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub items: Vec<Item>,
    /// Sum types declared in this module.
    pub adts: Vec<AdtDef>,
    /// Product types declared in this module.
    pub products: Vec<ProductDef>,
    /// `(trait, type)` pairs from `instance Trait for Type { }` (MVP: empty bodies).
    pub instances: HashSet<(String, String)>,
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
    /// C ABI symbol when declared via `foreign "C" fn …`
    pub external: Option<String>,
    /// When `external` is set: (param type names, return type name), e.g. `Int`.
    pub foreign_sig: Option<(Vec<String>, String)>,
    /// `foreign "C" pure fn` → Effect::pure() only when trust is enabled.
    pub foreign_pure: bool,
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
    /// Immutable delete: `m.remove(k)` / `s.remove(x)` → new Map/Set.
    MapRemove,
    /// Immutable set add: `s.insert(x)` → new Set (no-op if already present).
    SetInsert,
    MapKeys,
    MapValues,
    MapItems,
    /// List/Set identity (heap list); Map → keys. Used by indexed `for` / `toList`.
    Elems,
    Range,
    RangeInclusive,
    /// Format any scalar / String / Char as a heap String (interpolation).
    Show,
    /// String ops.
    StrTrim,
    StrSplit,
    StrSubstring,
    StrToLower,
    StrToUpper,
    StrStartsWith,
    StrEndsWith,
    /// Read entire stdin → String (IO).
    ReadStdin,
    /// Non-exhaustive / failed match (runtime abort).
    MatchFail,
    /// `xs.take(n)` → prefix List.
    ListTake,
    /// `xs.reverse()` → new List (same element order reversed).
    ListReverse,
    /// `xs.sort()` → new List[Int] ascending.
    ListSort,
    /// `xs.sortBy(f)` → permute by Ord keys (stable); runtime takes (values, keys).
    ListSortByKeys,
    /// Auto-parallel candidate `xs.map(f)` (FunRef-safe); demoted if impure/non-scalar.
    ListParMap,
    /// `assert(cond)` — abort if false (programming error).
    Assert,
    /// `xs.join(sep)` for List[String] → String.
    ListJoin,
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
        } => {
            // DESIGN: `and` / `or` short-circuit — desugar to `if`.
            let l = lower_expr(left);
            let r = lower_expr(right);
            match op {
                BinOp::And => Expr::If {
                    cond: Box::new(l),
                    then_branch: Box::new(r),
                    else_branch: Box::new(Expr::Bool(false, *span)),
                    span: *span,
                },
                BinOp::Or => Expr::If {
                    cond: Box::new(l),
                    then_branch: Box::new(Expr::Bool(true, *span)),
                    else_branch: Box::new(r),
                    span: *span,
                },
                _ => Expr::Binary {
                    op: *op,
                    left: Box::new(l),
                    right: Box::new(r),
                    span: *span,
                },
            }
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
        lumia_syntax::Expr::Pipeline { left, right, span } => {
            // Fuse `xs >> map … >> filter … >> fold(z, g)` before expanding intermediates.
            if let lumia_syntax::Expr::Call { callee, args, .. } = right.as_ref() {
                if let lumia_syntax::Expr::Ident(name, _) = callee.as_ref() {
                    if name == "fold" && args.len() == 2 {
                        if let Some(fused) =
                            try_fuse_hof_fold(left, &args[0], &args[1], *span)
                        {
                            return fused;
                        }
                    }
                }
            }
            match right.as_ref() {
                lumia_syntax::Expr::Call { callee, args, .. } => {
                    let mut new_args = vec![lower_expr(left)];
                    new_args.extend(args.iter().map(lower_expr));
                    lower_call_from_parts(lower_expr(callee), new_args, *span)
                }
                other => {
                    lower_call_from_parts(lower_expr(other), vec![lower_expr(left)], *span)
                }
            }
        }
        lumia_syntax::Expr::Field { base, field, span } => {
            // `xs.len` → len(xs); product fields → adt_field; `p.0` → adt_field;
            // else call field(base)
            if field == "len" {
                Expr::BuiltinCall {
                    name: Builtin::ListLen,
                    args: vec![lower_expr(base)],
                    span: *span,
                }
            } else if let Ok(idx) = field.parse::<i64>() {
                // Tuple / positional projection (DESIGN: `p.0`)
                Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![lower_expr(base), Expr::Int(idx, *span)],
                    span: *span,
                }
            } else if is_ambiguous_product_field(field) {
                set_lower_err(
                    format!(
                        "cannot resolve field `{field}` (ambiguous across product types)"
                    ),
                    *span,
                );
                Expr::Unit(*span)
            } else if let Some((adt_name, idx)) = lookup_product_field(field) {
                // Carry expected product name so ty can reject wrong receivers
                // (global name→index alone is unsound across distinct products).
                Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![
                        lower_expr(base),
                        Expr::Int(idx as i64, *span),
                        Expr::String(adt_name, *span),
                    ],
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
        lumia_syntax::Expr::MatchCond { arms, span } => lower_match_cond(arms, *span),
    }
}

/// Subjectless `match { c -> a; _ -> b }` → nested if/else.
fn lower_match_cond(arms: &[lumia_syntax::MatchCondArm], span: Span) -> Expr {
    fold_match_cond_arms(arms, span)
}

fn fold_match_cond_arms(arms: &[lumia_syntax::MatchCondArm], span: Span) -> Expr {
    if arms.is_empty() {
        return Expr::Unit(span);
    }
    let (arm, rest) = arms.split_first().unwrap();
    match &arm.cond {
        None => lower_expr(&arm.body),
        Some(cond) => {
            let else_body = if rest.is_empty() {
                Expr::Unit(span)
            } else {
                fold_match_cond_arms(rest, span)
            };
            Expr::If {
                cond: Box::new(lower_expr(cond)),
                then_branch: Box::new(lower_expr(&arm.body)),
                else_branch: Box::new(else_body),
                span,
            }
        }
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
        if name == "assert" {
            return Expr::BuiltinCall {
                name: Builtin::Assert,
                args: args.iter().map(lower_expr).collect(),
                span,
            };
        }
        if name == "fold" && args.len() == 3 {
            if let Some(fused) = try_fuse_hof_fold(&args[0], &args[1], &args[2], span) {
                return fused;
            }
        }
    }
    // Method call: fuse `….map(…).filter(…).fold(z, g)` on the syntax tree.
    if let lumia_syntax::Expr::Field { base, field, .. } = callee {
        if field == "fold" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_fold(base, &args[0], &args[1], span) {
                return fused;
            }
        }
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
            "flatMap" if args.len() == 2 => {
                return lower_list_flat_map(args[0].clone(), args[1].clone(), span);
            }
            "fold" if args.len() == 3 => {
                return lower_list_fold(
                    args[0].clone(),
                    args[1].clone(),
                    args[2].clone(),
                    span,
                );
            }
            "any" if args.len() == 2 => {
                return lower_list_any(args[0].clone(), args[1].clone(), span);
            }
            "all" if args.len() == 2 => {
                return lower_list_all(args[0].clone(), args[1].clone(), span);
            }
            "find" if args.len() == 2 => {
                return lower_list_find(args[0].clone(), args[1].clone(), span);
            }
            "append" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListAppend,
                    args,
                    span,
                };
            }
            "isEmpty" if args.len() == 1 => {
                return Expr::Binary {
                    op: BinOp::Eq,
                    left: Box::new(Expr::BuiltinCall {
                        name: Builtin::ListLen,
                        args: vec![args[0].clone()],
                        span,
                    }),
                    right: Box::new(Expr::Int(0, span)),
                    span,
                };
            }
            "toSet" if args.len() == 1 => {
                return lower_to_set(args[0].clone(), span);
            }
            "toList" if args.len() == 1 => {
                return lower_to_list(args[0].clone(), span);
            }
            "toMap" if args.len() == 1 => {
                return lower_to_map(args[0].clone(), span);
            }
            "union" if args.len() == 2 => {
                return lower_set_union(args[0].clone(), args[1].clone(), span);
            }
            "intersect" if args.len() == 2 => {
                return lower_set_intersect(args[0].clone(), args[1].clone(), span);
            }
            "diff" if args.len() == 2 => {
                return lower_set_diff(args[0].clone(), args[1].clone(), span);
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
            "insert" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::SetInsert,
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
            "drop" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListSlice,
                    args,
                    span,
                };
            }
            "take" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListTake,
                    args,
                    span,
                };
            }
            "reverse" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListReverse,
                    args,
                    span,
                };
            }
            "sort" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListSort,
                    args,
                    span,
                };
            }
            "sortBy" if args.len() == 2 => {
                return lower_list_sort_by(args[0].clone(), args[1].clone(), span);
            }
            "join" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListJoin,
                    args,
                    span,
                };
            }
            "lines" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrSplit,
                    args: vec![args[0].clone(), Expr::Char('\n', span)],
                    span,
                };
            }
            "trim" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrTrim,
                    args,
                    span,
                };
            }
            "split" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrSplit,
                    args,
                    span,
                };
            }
            "substring" if args.len() == 3 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrSubstring,
                    args,
                    span,
                };
            }
            "toLower" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrToLower,
                    args,
                    span,
                };
            }
            "toUpper" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrToUpper,
                    args,
                    span,
                };
            }
            "startsWith" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrStartsWith,
                    args,
                    span,
                };
            }
            "endsWith" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrEndsWith,
                    args,
                    span,
                };
            }
            "readStdin" if args.is_empty() => {
                return Expr::BuiltinCall {
                    name: Builtin::ReadStdin,
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
        if by_name.insert(f.clone(), lower_expr(e)).is_some() {
            set_lower_err(format!("duplicate field `{f}` in `{name}` struct literal"), span);
        }
    }
    let mut args = Vec::with_capacity(order.len());
    for f in &order {
        if let Some(e) = by_name.remove(f) {
            args.push(e);
        } else {
            set_lower_err(format!("missing field `{f}` in `{name}` struct literal"), span);
            // Placeholder; `lower_module` aborts on LOWER_ERR.
            args.push(Expr::Int(0, span));
        }
    }
    if let Some((extra, _)) = by_name.iter().next() {
        set_lower_err(
            format!("unknown field `{extra}` in `{name}` struct literal"),
            span,
        );
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
    // Infer product from first updated field name. Shared field names across
    // product types are stripped from the map (ambiguous) and must error here.
    let Some((fname, _)) = fields.first() else {
        return lower_expr(base);
    };
    let Some((type_name, _)) = lookup_product_field(fname) else {
        set_lower_err(
            format!(
                "cannot resolve `with` field `{fname}` (unknown or ambiguous across product types)"
            ),
            span,
        );
        return lower_expr(base);
    };
    let Some(order) = lookup_product(&type_name) else {
        set_lower_err(
            format!("unknown product type `{type_name}` in `with`"),
            span,
        );
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
    let expanded = expand_or_arms(arms);
    let body = fold_match_arms(&expanded, &scrut, span);
    Expr::Let {
        name: scrut,
        value: Box::new(lower_expr(scrutinee)),
        body: Box::new(body),
        mutable: false,
    }
}

/// Top-level `A | B -> body` → two arms (correct bindings per alternative).
fn expand_or_arms(arms: &[lumia_syntax::MatchArm]) -> Vec<lumia_syntax::MatchArm> {
    let mut out = Vec::new();
    for arm in arms {
        match &arm.pattern {
            Pattern::Or(pats, _) if pats.len() > 1 => {
                for p in pats {
                    out.push(lumia_syntax::MatchArm {
                        pattern: p.clone(),
                        guard: arm.guard.clone(),
                        body: arm.body.clone(),
                        span: arm.span,
                    });
                }
            }
            _ => out.push(arm.clone()),
        }
    }
    out
}

fn fold_match_arms(arms: &[lumia_syntax::MatchArm], scrut: &str, span: Span) -> Expr {
    if arms.is_empty() {
        return Expr::BuiltinCall {
            name: Builtin::MatchFail,
            args: vec![],
            span,
        };
    }
    let scrut_e = Expr::Var(scrut.into(), span);
    let (arm, rest) = arms.split_first().unwrap();
    let (pat_cond, binds) = pattern_cond(&arm.pattern, &scrut_e, span);
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
        // Short-circuit: do not evaluate guard (or its field loads) if pat fails.
        short_and(pat_cond, guard_e, span)
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
    // Always test the pattern — including the last arm (unless irrefutable).
    if rest.is_empty() {
        if pattern_irrefutable(&arm.pattern) {
            return then_body;
        }
        return Expr::If {
            cond: Box::new(cond),
            then_branch: Box::new(then_body),
            else_branch: Box::new(Expr::BuiltinCall {
                name: Builtin::MatchFail,
                args: vec![],
                span,
            }),
            span,
        };
    }
    let else_body = fold_match_arms(rest, scrut, span);
    Expr::If {
        cond: Box::new(cond),
        then_branch: Box::new(then_body),
        else_branch: Box::new(else_body),
        span,
    }
}

/// Last-arm elision: only `_` / binders (and all-irrefutable `or`) may skip the
/// tag test + `MatchFail`. Nullary ctor names like `None` are refutable — same
/// rule as [`coverage_catch_all`].
fn pattern_irrefutable(pat: &Pattern) -> bool {
    match pat {
        Pattern::Wildcard(_) => true,
        Pattern::Ident(name, _) => !lookup_ctor(name).is_some_and(|c| c.arity == 0),
        Pattern::Or(ps, _) => !ps.is_empty() && ps.iter().all(pattern_irrefutable),
        _ => false,
    }
}

/// Build match condition + binder equations for `pat` against scrutinee expression `scrut`.
/// Nested patterns compose field/get paths (no temps), so binders stay valid in the arm body.
fn pattern_cond(pat: &Pattern, scrut: &Expr, span: Span) -> (Expr, Vec<(String, Expr)>) {
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
            if let Some(c) = lookup_ctor(name) {
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
            (
                Expr::Bool(true, span),
                vec![(name.clone(), scrut.clone())],
            )
        }
        Pattern::Or(pats, _) => {
            // Nested or-patterns with binders are ambiguous; top-level or is expanded.
            let mut cond = Expr::Bool(false, span);
            let mut binds = vec![];
            for p in pats {
                let (c, b) = pattern_cond(p, scrut, span);
                if !b.is_empty() {
                    set_lower_err(
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
            let Some(c) = lookup_ctor(name) else {
                set_lower_err(
                    format!("unknown variant `{name}` in pattern"),
                    span,
                );
                return (Expr::Bool(false, span), vec![]);
            };
            if args.len() != c.arity {
                set_lower_err(
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
                    Pattern::Ident(n, _) if lookup_ctor(n).is_none_or(|c| c.arity != 0) => {
                        // Binder (not a nullary ctor name).
                        binds.push((n.clone(), field));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(sub, &field, span);
                        cond = short_and(cond, sub_cond, span);
                        binds.extend(sub_binds);
                    }
                }
            }
            (cond, binds)
        }
        Pattern::Struct { name, fields, .. } => {
            let Some(order) = lookup_product(name) else {
                set_lower_err(
                    format!("unknown product type `{name}` in struct pattern"),
                    span,
                );
                return (Expr::Bool(false, span), vec![]);
            };
            let mut cond = Expr::Bool(true, span);
            let mut binds = vec![];
            for (fname, sub) in fields {
                let Some(idx) = order.iter().position(|f| f == fname) else {
                    set_lower_err(
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
                    Pattern::Ident(n, _) if lookup_ctor(n).is_none_or(|c| c.arity != 0) => {
                        binds.push((n.clone(), field));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(sub, &field, span);
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
                    Pattern::Ident(n, _) if lookup_ctor(n).is_none_or(|c| c.arity != 0) => {
                        binds.push((n.clone(), field));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(sub, &field, span);
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
                        if lookup_ctor(name).is_none_or(|c| c.arity != 0) =>
                    {
                        binds.push((name.clone(), get));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(sub, &get, span);
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

/// `xs.map(f)` → `ListParMap` when FunRef-safe; else sequential accumulate.
/// Type checking may demote `ListParMap` back to sequential (IO / non-scalar).
fn lower_list_map(list: Expr, f: Expr, span: Span) -> Expr {
    if map_callback_is_parallel_safe(&f) {
        return Expr::BuiltinCall {
            name: Builtin::ListParMap,
            args: vec![list, f],
            span,
        };
    }
    desugar_list_map_sequential(list, f, span)
}

/// Sequential `map` loop (also used when auto-parallel demotes `ListParMap`).
pub fn desugar_list_map_sequential(list: Expr, f: Expr, span: Span) -> Expr {
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

/// Parallel map: capture-free lambda, or a top-level function name (FunRef).
fn map_callback_is_parallel_safe(f: &Expr) -> bool {
    match f {
        Expr::Lambda { params, body, .. } => {
            let mut bound: Vec<String> = params.clone();
            free_vars_expr(body, &mut bound).is_empty()
        }
        Expr::Var(n, _) => TOPLEVEL_FUNS.with(|t| t.borrow().contains(n)),
        _ => false,
    }
}

fn free_vars_expr(e: &Expr, bound: &mut Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(e: &Expr, bound: &mut Vec<String>, out: &mut Vec<String>) {
        match e {
            Expr::Var(n, _) => {
                if !bound.iter().any(|b| b == n) && !out.iter().any(|x| x == n) {
                    out.push(n.clone());
                }
            }
            Expr::Let {
                name, value, body, ..
            } => {
                walk(value, bound, out);
                bound.push(name.clone());
                walk(body, bound, out);
                bound.pop();
            }
            Expr::Lambda { params, body, .. } => {
                let n = bound.len();
                for p in params {
                    bound.push(p.clone());
                }
                walk(body, bound, out);
                bound.truncate(n);
            }
            Expr::Assign { value, .. } | Expr::Unary { expr: value, .. } => walk(value, bound, out),
            Expr::Call { callee, args, .. } => {
                walk(callee, bound, out);
                for a in args {
                    walk(a, bound, out);
                }
            }
            Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
                for a in args {
                    walk(a, bound, out);
                }
            }
            Expr::Binary { left, right, .. } => {
                walk(left, bound, out);
                walk(right, bound, out);
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                walk(cond, bound, out);
                walk(then_branch, bound, out);
                walk(else_branch, bound, out);
            }
            Expr::Loop {
                cond, body, step, ..
            } => {
                walk(cond, bound, out);
                walk(body, bound, out);
                if let Some(s) = step {
                    walk(s, bound, out);
                }
            }
            Expr::Seq { stmts, .. } => {
                for s in stmts {
                    walk(s, bound, out);
                }
            }
            _ => {}
        }
    }
    walk(e, bound, &mut out);
    out
}

/// `xs.sortBy(f)` — key must be Int / String / Char; stable permute of elements.
fn lower_list_sort_by(list: Expr, f: Expr, span: Span) -> Expr {
    let xs = format!("__sby_xs_{}", span.start.0);
    let keys = format!("__sby_keys_{}", span.start.0);
    Expr::Let {
        name: xs.clone(),
        value: Box::new(list),
        body: Box::new(Expr::Let {
            name: keys.clone(),
            value: Box::new(lower_list_map(Expr::Var(xs.clone(), span), f, span)),
            body: Box::new(Expr::BuiltinCall {
                name: Builtin::ListSortByKeys,
                args: vec![Expr::Var(xs, span), Expr::Var(keys, span)],
                span,
            }),
            mutable: false,
        }),
        mutable: false,
    }
}

fn lower_list_map_inline(list: Expr, x: String, body: Expr, span: Span) -> Expr {
    let acc = format!("__map_acc_{}", span.start.0);
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
        name: acc.clone(),
        value: Box::new(empty_list(span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x, list, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    }
}

fn lower_list_map_call(list: Expr, f: Expr, f_name: String, x: String, span: Span) -> Expr {
    let acc = format!("__map_acc_{}", span.start.0);
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
            name: acc.clone(),
            value: Box::new(empty_list(span)),
            body: Box::new(Expr::Seq {
                stmts: vec![
                    for_each_elem(&x, list, step, span),
                    Expr::Var(acc, span),
                ],
                span,
            }),
            mutable: true,
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
        name: acc.clone(),
        value: Box::new(empty_list(span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x, list, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    }
}

fn lower_list_filter_call(list: Expr, f: Expr, f_name: String, x: String, span: Span) -> Expr {
    let acc = format!("__flt_acc_{}", span.start.0);
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
            name: acc.clone(),
            value: Box::new(empty_list(span)),
            body: Box::new(Expr::Seq {
                stmts: vec![
                    for_each_elem(&x, list, step, span),
                    Expr::Var(acc, span),
                ],
                span,
            }),
            mutable: true,
        }),
        mutable: false,
    }
}

fn apply_pred(f: &Expr, x: Expr, span: Span) -> Expr {
    match f {
        Expr::Lambda { params, body, .. } if params.len() == 1 => Expr::Let {
            name: params[0].clone(),
            value: Box::new(x),
            body: body.clone(),
            mutable: false,
        },
        _ => Expr::Call {
            callee: Box::new(f.clone()),
            args: vec![x],
            span,
        },
    }
}

/// `xs.flatMap(f)` where `f: T -> List[U]` → concat mapped lists.
fn lower_list_flat_map(list: Expr, f: Expr, span: Span) -> Expr {
    let acc = format!("__fmap_acc_{}", span.start.0);
    let x = format!("__fmap_x_{}", span.start.0);
    match &f {
        Expr::Lambda { params, body, .. } if params.len() == 1 => {
            let mapped = Expr::Let {
                name: params[0].clone(),
                value: Box::new(Expr::Var(x.clone(), span)),
                body: body.clone(),
                mutable: false,
            };
            let step = Expr::Assign {
                name: acc.clone(),
                value: Box::new(Expr::BuiltinCall {
                    name: Builtin::ListConcat,
                    args: vec![Expr::Var(acc.clone(), span), mapped],
                    span,
                }),
                span,
            };
            Expr::Let {
                name: acc.clone(),
                value: Box::new(empty_list(span)),
                body: Box::new(Expr::Seq {
                    stmts: vec![
                        for_each_elem(&x, list, step, span),
                        Expr::Var(acc, span),
                    ],
                    span,
                }),
                mutable: true,
            }
        }
        _ => {
            let f_name = format!("__fmap_f_{}", span.start.0);
            let mapped = Expr::Call {
                callee: Box::new(Expr::Var(f_name.clone(), span)),
                args: vec![Expr::Var(x.clone(), span)],
                span,
            };
            let step = Expr::Assign {
                name: acc.clone(),
                value: Box::new(Expr::BuiltinCall {
                    name: Builtin::ListConcat,
                    args: vec![Expr::Var(acc.clone(), span), mapped],
                    span,
                }),
                span,
            };
            Expr::Let {
                name: f_name,
                value: Box::new(f),
                body: Box::new(Expr::Let {
                    name: acc.clone(),
                    value: Box::new(empty_list(span)),
                    body: Box::new(Expr::Seq {
                        stmts: vec![
                            for_each_elem(&x, list, step, span),
                            Expr::Var(acc, span),
                        ],
                        span,
                    }),
                    mutable: true,
                }),
                mutable: false,
            }
        }
    }
}

fn option_some(x: Expr, span: Span) -> Expr {
    match lookup_ctor("Some") {
        Some(c) => Expr::AdtNew {
            adt_name: c.adt_name,
            variant: "Some".into(),
            tag: c.tag,
            args: vec![x],
            span,
        },
        None => Expr::Call {
            callee: Box::new(Expr::Var("Some".into(), span)),
            args: vec![x],
            span,
        },
    }
}

fn option_none(span: Span) -> Expr {
    match lookup_ctor("None") {
        Some(c) => Expr::AdtNew {
            adt_name: c.adt_name,
            variant: "None".into(),
            tag: c.tag,
            args: vec![],
            span,
        },
        None => Expr::Call {
            callee: Box::new(Expr::Var("None".into(), span)),
            args: vec![],
            span,
        },
    }
}

/// Bind non-lambda `f` to a temp; lambdas stay inline.
fn bind_fun(f: Expr, span: Span) -> (Option<(String, Expr)>, Expr) {
    match &f {
        Expr::Lambda { .. } => (None, f),
        _ => {
            let name = format!("__pred_f_{}", span.start.0);
            (Some((name.clone(), f)), Expr::Var(name, span))
        }
    }
}

fn lower_list_any(list: Expr, f: Expr, span: Span) -> Expr {
    let acc = format!("__any_acc_{}", span.start.0);
    let x = format!("__any_x_{}", span.start.0);
    let (f_bind, pred_f) = bind_fun(f, span);
    let pred = apply_pred(&pred_f, Expr::Var(x.clone(), span), span);
    let hit = Expr::Seq {
        stmts: vec![
            Expr::Assign {
                name: acc.clone(),
                value: Box::new(Expr::Bool(true, span)),
                span,
            },
            Expr::Break(span),
        ],
        span,
    };
    let step = Expr::If {
        cond: Box::new(pred),
        then_branch: Box::new(hit),
        else_branch: Box::new(Expr::Unit(span)),
        span,
    };
    let core = Expr::Let {
        name: acc.clone(),
        value: Box::new(Expr::Bool(false, span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x, list, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    };
    match f_bind {
        Some((name, val)) => Expr::Let {
            name,
            value: Box::new(val),
            body: Box::new(core),
            mutable: false,
        },
        None => core,
    }
}

fn lower_list_all(list: Expr, f: Expr, span: Span) -> Expr {
    let acc = format!("__all_acc_{}", span.start.0);
    let x = format!("__all_x_{}", span.start.0);
    let (f_bind, pred_f) = bind_fun(f, span);
    let pred = apply_pred(&pred_f, Expr::Var(x.clone(), span), span);
    let miss = Expr::Seq {
        stmts: vec![
            Expr::Assign {
                name: acc.clone(),
                value: Box::new(Expr::Bool(false, span)),
                span,
            },
            Expr::Break(span),
        ],
        span,
    };
    let step = Expr::If {
        cond: Box::new(pred),
        then_branch: Box::new(Expr::Unit(span)),
        else_branch: Box::new(miss),
        span,
    };
    let core = Expr::Let {
        name: acc.clone(),
        value: Box::new(Expr::Bool(true, span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x, list, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    };
    match f_bind {
        Some((name, val)) => Expr::Let {
            name,
            value: Box::new(val),
            body: Box::new(core),
            mutable: false,
        },
        None => core,
    }
}

fn lower_list_find(list: Expr, f: Expr, span: Span) -> Expr {
    let acc = format!("__find_acc_{}", span.start.0);
    let x = format!("__find_x_{}", span.start.0);
    let (f_bind, pred_f) = bind_fun(f, span);
    let pred = apply_pred(&pred_f, Expr::Var(x.clone(), span), span);
    let hit = Expr::Seq {
        stmts: vec![
            Expr::Assign {
                name: acc.clone(),
                value: Box::new(option_some(Expr::Var(x.clone(), span), span)),
                span,
            },
            Expr::Break(span),
        ],
        span,
    };
    let step = Expr::If {
        cond: Box::new(pred),
        then_branch: Box::new(hit),
        else_branch: Box::new(Expr::Unit(span)),
        span,
    };
    let core = Expr::Let {
        name: acc.clone(),
        value: Box::new(option_none(span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x, list, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    };
    match f_bind {
        Some((name, val)) => Expr::Let {
            name,
            value: Box::new(val),
            body: Box::new(core),
            mutable: false,
        },
        None => core,
    }
}

fn lower_list_fold(list: Expr, init: Expr, f: Expr, span: Span) -> Expr {
    // `range(a,b).fold(...)` → counter loop (no HeapList materialization).
    if let Expr::BuiltinCall { name, args, .. } = &list {
        if matches!(name, Builtin::Range | Builtin::RangeInclusive) && args.len() == 2 {
            let inclusive = matches!(name, Builtin::RangeInclusive);
            let start = args[0].clone();
            let end = args[1].clone();
            return match &f {
                Expr::Lambda { params, body, .. } if params.len() == 2 => range_fold_inline(
                    start,
                    end,
                    inclusive,
                    init,
                    params[0].clone(),
                    params[1].clone(),
                    *body.clone(),
                    span,
                ),
                _ => {
                    let f_name = format!("__fold_f_{}", span.start.0);
                    let x = format!("__fold_x_{}", span.start.0);
                    let acc = format!("__fold_acc_{}", span.start.0);
                    range_fold_call(start, end, inclusive, init, f, f_name, acc, x, span)
                }
            };
        }
    }
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

fn range_fold_inline(
    start: Expr,
    end: Expr,
    inclusive: bool,
    init: Expr,
    acc: String,
    x: String,
    body: Expr,
    span: Span,
) -> Expr {
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(body),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(init),
        body: Box::new(Expr::Seq {
            stmts: vec![
                counter_for_in(&x, start, end, inclusive, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    }
}

fn range_fold_call(
    start: Expr,
    end: Expr,
    inclusive: bool,
    init: Expr,
    f: Expr,
    f_name: String,
    acc: String,
    x: String,
    span: Span,
) -> Expr {
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
            name: acc.clone(),
            value: Box::new(init),
            body: Box::new(Expr::Seq {
                stmts: vec![
                    counter_for_in(&x, start, end, inclusive, step, span),
                    Expr::Var(acc, span),
                ],
                span,
            }),
            mutable: true,
        }),
        mutable: false,
    }
}

fn lower_list_fold_inline(
    list: Expr,
    init: Expr,
    acc: String,
    x: String,
    body: Expr,
    span: Span,
) -> Expr {
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(body),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(init),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x, list, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
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
            name: acc.clone(),
            value: Box::new(init),
            body: Box::new(Expr::Seq {
                stmts: vec![
                    for_each_elem(&x, list, step, span),
                    Expr::Var(acc, span),
                ],
                span,
            }),
            mutable: true,
        }),
        mutable: false,
    }
}

/// `for x in range(a,b)` → counter loop; `for (k,v) in m` → items + destructure;
/// `for (k,v) in pairs` (already a List of 2-tuples) → destructure only;
/// otherwise indexed len/get loop (List / Set).
fn lower_for_in(
    binding: &lumia_syntax::ForBinding,
    iter: &lumia_syntax::Expr,
    body: &lumia_syntax::Expr,
    span: Span,
) -> Expr {
    let body_e = lower_expr(body);
    match binding {
        lumia_syntax::ForBinding::Pair(k, v) => {
            let lowered = lower_expr(iter);
            // Map → MapItems; already a List[(K,V)] (listOf / items / sortBy) → as-is.
            // Runtime `lumia_map_items` is also identity on List as a safety net.
            let items = if expr_already_pair_list(&lowered) {
                lowered
            } else {
                Expr::BuiltinCall {
                    name: Builtin::MapItems,
                    args: vec![lowered],
                    span,
                }
            };
            let pair = format!("__kv_{}", span.start.0);
            let bind_k = Expr::Let {
                name: k.clone(),
                value: Box::new(Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![Expr::Var(pair.clone(), span), Expr::Int(0, span)],
                    span,
                }),
                body: Box::new(Expr::Let {
                    name: v.clone(),
                    value: Box::new(Expr::BuiltinCall {
                        name: Builtin::AdtField,
                        args: vec![Expr::Var(pair.clone(), span), Expr::Int(1, span)],
                        span,
                    }),
                    body: Box::new(body_e),
                    mutable: false,
                }),
                mutable: false,
            };
            list_for_in(&pair, items, bind_k, span)
        }
        lumia_syntax::ForBinding::Name(name) => {
            let lowered_iter = lower_expr(iter);
            if let Expr::BuiltinCall { name: b, args, .. } = &lowered_iter {
                if matches!(b, Builtin::Range | Builtin::RangeInclusive) && args.len() == 2 {
                    let inclusive = matches!(b, Builtin::RangeInclusive);
                    return counter_for_in(
                        name,
                        args[0].clone(),
                        args[1].clone(),
                        inclusive,
                        body_e,
                        span,
                    );
                }
            }
            list_for_in(name, lowered_iter, body_e, span)
        }
    }
}

/// True when `e` is already a List of pairs (not a Map needing `items()`).
fn expr_already_pair_list(e: &Expr) -> bool {
    match e {
        Expr::BuiltinCall { name, .. } => matches!(
            name,
            Builtin::MapItems | Builtin::ListSortByKeys
        ),
        Expr::Call { callee, .. } => {
            matches!(callee.as_ref(), Expr::Var(n, _) if n == "listOf")
        }
        Expr::Let { body, .. } => expr_already_pair_list(body),
        Expr::Seq { stmts, .. } => stmts
            .last()
            .map(expr_already_pair_list)
            .unwrap_or(false),
        _ => false,
    }
}

fn counter_for_in(
    binding: &str,
    start: Expr,
    end: Expr,
    inclusive: bool,
    body: Expr,
    span: Span,
) -> Expr {
    let i = format!("__i_{}", span.start.0);
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
    // Map is key-addressed; normalize to an indexable List (keys) first.
    let list = Expr::BuiltinCall {
        name: Builtin::Elems,
        args: vec![list],
        span,
    };
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

/// Iterate with a custom per-element step, using either a counter (range) or indexed get.
fn for_each_elem(x: &str, list: Expr, step: Expr, span: Span) -> Expr {
    if let Expr::BuiltinCall { name, args, .. } = &list {
        if matches!(name, Builtin::Range | Builtin::RangeInclusive) && args.len() == 2 {
            let inclusive = matches!(name, Builtin::RangeInclusive);
            return counter_for_in(x, args[0].clone(), args[1].clone(), inclusive, step, span);
        }
    }
    list_for_in(x, list, step, span)
}

fn empty_list(span: Span) -> Expr {
    Expr::Call {
        callee: Box::new(Expr::Var("listOf".into(), span)),
        args: vec![],
        span,
    }
}

fn empty_set(span: Span) -> Expr {
    Expr::Call {
        callee: Box::new(Expr::Var("setOf".into(), span)),
        args: vec![],
        span,
    }
}

fn empty_map(span: Span) -> Expr {
    Expr::Call {
        callee: Box::new(Expr::Var("mapOf".into(), span)),
        args: vec![],
        span,
    }
}

fn lower_to_set(list: Expr, span: Span) -> Expr {
    let acc = format!("__toset_acc_{}", span.start.0);
    let x = format!("__toset_x_{}", span.start.0);
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::SetInsert,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(empty_set(span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x, list, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    }
}

fn lower_to_list(col: Expr, span: Span) -> Expr {
    let acc = format!("__tolist_acc_{}", span.start.0);
    let x = format!("__tolist_x_{}", span.start.0);
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListAppend,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(empty_list(span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x, col, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    }
}

/// `pairs.toMap()` — each element is a 2-tuple `(k, v)`.
fn lower_to_map(pairs: Expr, span: Span) -> Expr {
    let acc = format!("__tomap_acc_{}", span.start.0);
    let p = format!("__tomap_p_{}", span.start.0);
    let k = Expr::BuiltinCall {
        name: Builtin::AdtField,
        args: vec![Expr::Var(p.clone(), span), Expr::Int(0, span)],
        span,
    };
    let v = Expr::BuiltinCall {
        name: Builtin::AdtField,
        args: vec![Expr::Var(p.clone(), span), Expr::Int(1, span)],
        span,
    };
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::MapSet,
            args: vec![Expr::Var(acc.clone(), span), k, v],
            span,
        }),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(empty_map(span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                list_for_in(&p, pairs, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    }
}

fn lower_set_union(a: Expr, b: Expr, span: Span) -> Expr {
    let acc = format!("__union_acc_{}", span.start.0);
    let x = format!("__union_x_{}", span.start.0);
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::SetInsert,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(a),
        body: Box::new(Expr::Seq {
            stmts: vec![
                list_for_in(&x, b, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    }
}

fn lower_set_intersect(a: Expr, b: Expr, span: Span) -> Expr {
    let acc = format!("__isect_acc_{}", span.start.0);
    let other = format!("__isect_b_{}", span.start.0);
    let x = format!("__isect_x_{}", span.start.0);
    let insert = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::SetInsert,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    let step = Expr::If {
        cond: Box::new(Expr::BuiltinCall {
            name: Builtin::Contains,
            args: vec![Expr::Var(other.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        then_branch: Box::new(insert),
        else_branch: Box::new(Expr::Unit(span)),
        span,
    };
    Expr::Let {
        name: other.clone(),
        value: Box::new(b),
        body: Box::new(Expr::Let {
            name: acc.clone(),
            value: Box::new(empty_set(span)),
            body: Box::new(Expr::Seq {
                stmts: vec![
                    list_for_in(&x, a, step, span),
                    Expr::Var(acc, span),
                ],
                span,
            }),
            mutable: true,
        }),
        mutable: false,
    }
}

fn lower_set_diff(a: Expr, b: Expr, span: Span) -> Expr {
    let acc = format!("__diff_acc_{}", span.start.0);
    let x = format!("__diff_x_{}", span.start.0);
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::MapRemove,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(a),
        body: Box::new(Expr::Seq {
            stmts: vec![
                list_for_in(&x, b, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    }
}

/// Peel trailing `.map(f)` / `.filter(p)` / `map(…)` / `filter(…)` / pipeline hops.
fn peel_hof_maps_filters<'a>(
    mut e: &'a lumia_syntax::Expr,
) -> (
    &'a lumia_syntax::Expr,
    Vec<&'a lumia_syntax::Expr>,
    Vec<&'a lumia_syntax::Expr>,
) {
    let mut maps: Vec<&lumia_syntax::Expr> = Vec::new();
    let mut filters: Vec<&lumia_syntax::Expr> = Vec::new();
    loop {
        match e {
            lumia_syntax::Expr::Pipeline { left, right, .. } => match right.as_ref() {
                lumia_syntax::Expr::Call { callee, args, .. } => match callee.as_ref() {
                    lumia_syntax::Expr::Ident(n, _) if n == "map" && args.len() == 1 => {
                        maps.push(&args[0]);
                        e = left;
                        continue;
                    }
                    lumia_syntax::Expr::Ident(n, _) if n == "filter" && args.len() == 1 => {
                        filters.push(&args[0]);
                        e = left;
                        continue;
                    }
                    _ => break,
                },
                _ => break,
            },
            lumia_syntax::Expr::Call { callee, args, .. } => {
                if let lumia_syntax::Expr::Field { base, field, .. } = callee.as_ref() {
                    if field == "map" && args.len() == 1 {
                        maps.push(&args[0]);
                        e = base;
                        continue;
                    }
                    if field == "filter" && args.len() == 1 {
                        filters.push(&args[0]);
                        e = base;
                        continue;
                    }
                }
                if let lumia_syntax::Expr::Ident(n, _) = callee.as_ref() {
                    if n == "map" && args.len() == 2 {
                        maps.push(&args[1]);
                        e = &args[0];
                        continue;
                    }
                    if n == "filter" && args.len() == 2 {
                        filters.push(&args[1]);
                        e = &args[0];
                        continue;
                    }
                }
                break;
            }
            _ => break,
        }
    }
    maps.reverse();
    filters.reverse();
    (e, maps, filters)
}

fn apply_hof_fn(f: &lumia_syntax::Expr, arg: Expr, span: Span) -> Expr {
    match f {
        lumia_syntax::Expr::Lambda { params, body, .. } if params.len() == 1 => Expr::Let {
            name: params[0].clone(),
            value: Box::new(arg),
            body: Box::new(lower_expr(body)),
            mutable: false,
        },
        _ => Expr::Call {
            callee: Box::new(lower_expr(f)),
            args: vec![arg],
            span,
        },
    }
}

fn apply_fold_fn(f: &lumia_syntax::Expr, acc: Expr, x: Expr, span: Span) -> Expr {
    match f {
        lumia_syntax::Expr::Lambda { params, body, .. } if params.len() == 2 => Expr::Let {
            name: params[0].clone(),
            value: Box::new(acc),
            body: Box::new(Expr::Let {
                name: params[1].clone(),
                value: Box::new(x),
                body: Box::new(lower_expr(body)),
                mutable: false,
            }),
            mutable: false,
        },
        _ => Expr::Call {
            callee: Box::new(lower_expr(f)),
            args: vec![acc, x],
            span,
        },
    }
}

/// Single-pass `source.map*.filter*.fold` — no intermediate lists.
fn try_fuse_hof_fold(
    coll: &lumia_syntax::Expr,
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    let (source, maps, filters) = peel_hof_maps_filters(coll);
    if maps.is_empty() && filters.is_empty() {
        return None;
    }
    let acc = format!("__fuse_acc_{}", span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let mut cur = Expr::Var(x0.clone(), span);
    for m in &maps {
        cur = apply_hof_fn(m, cur, span);
    }
    let x_mapped = format!("__fuse_xm_{}", span.start.0);
    let mut body = Expr::Assign {
        name: acc.clone(),
        value: Box::new(apply_fold_fn(
            f,
            Expr::Var(acc.clone(), span),
            Expr::Var(x_mapped.clone(), span),
            span,
        )),
        span,
    };
    for p in filters.iter().rev() {
        body = Expr::If {
            cond: Box::new(apply_hof_fn(p, Expr::Var(x_mapped.clone(), span), span)),
            then_branch: Box::new(body),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        };
    }
    let step = Expr::Let {
        name: x_mapped,
        value: Box::new(cur),
        body: Box::new(body),
        mutable: false,
    };
    let source_e = lower_expr(source);
    Some(Expr::Let {
        name: acc.clone(),
        value: Box::new(lower_expr(init)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    })
}

#[cfg(test)]
mod tests {
    use super::{lower_module, Builtin, Expr, Item};
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
        let err = lower_module(&ast).unwrap_err().to_string();
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

    #[test]
    fn exhaustiveness_rejects_nested_missing() {
        let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
    o match {
        None -> { 0 }
        Some(None) -> { 1 }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("non-exhaustive"), "{err}");
        assert!(err.contains("Some"), "{err}");
        assert!(err.contains("in Some"), "{err}");
    }

    #[test]
    fn exhaustiveness_accepts_nested_option() {
        let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
    o match {
        None -> { 0 }
        Some(None) -> { 1 }
        Some(Some(n)) -> { n }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        assert!(lower_module(&ast).is_ok());
    }

    #[test]
    fn exhaustiveness_rejects_nested_result_missing_err() {
        let src = r#"
module M
type Option { Some(value) None }
type Result { Ok(value) Err(msg) }
val f = { o ->
    o match {
        None -> { 0 }
        Some(Ok(n)) -> { n }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("non-exhaustive"), "{err}");
        assert!(err.contains("Err"), "{err}");
    }

    #[test]
    fn exhaustiveness_rejects_product_field_gap() {
        let src = r#"
module M
type Option { Some(value) None }
type Box { val inner }
val f = { b ->
    b match {
        Box { inner = Some(n) } -> { n }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("non-exhaustive"), "{err}");
        assert!(err.contains("None"), "{err}");
    }

    #[test]
    fn exhaustiveness_accepts_nested_catch_all_payload() {
        let src = r#"
module M
type Option { Some(value) None }
type Result { Ok(value) Err(msg) }
val f = { o ->
    o match {
        None -> { 0 }
        Some(_) -> { 1 }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        assert!(lower_module(&ast).is_ok());
    }

    #[test]
    fn exhaustiveness_rejects_int_literals_without_wildcard() {
        let src = r#"
module M
val f = { n ->
    n match {
        0 -> { 1 }
        1 -> { 2 }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("non-exhaustive"), "{err}");
        assert!(err.contains("Int"), "{err}");
    }

    #[test]
    fn exhaustiveness_rejects_empty_match() {
        let src = r#"
module M
val f = { n ->
    n match { }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("non-exhaustive"), "{err}");
    }

    #[test]
    fn exhaustiveness_rejects_guard_only_arms() {
        let src = r#"
module M
val f = { n ->
    n match {
        x if x > 0 -> { 1 }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("non-exhaustive"), "{err}");
    }

    #[test]
    fn exhaustiveness_accepts_int_with_wildcard() {
        let src = r#"
module M
val f = { n ->
    n match {
        0 -> { 1 }
        _ -> { 2 }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        assert!(lower_module(&ast).is_ok());
    }

    #[test]
    fn exhaustiveness_accepts_bool_both_arms() {
        let src = r#"
module M
val f = { b ->
    b match {
        true -> { 1 }
        false -> { 0 }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        assert!(lower_module(&ast).is_ok(), "{:?}", lower_module(&ast));
    }

    #[test]
    fn exhaustiveness_rejects_bool_missing_false() {
        let src = r#"
module M
val f = { b ->
    b match {
        true -> { 1 }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("non-exhaustive"), "{err}");
        assert!(err.contains("false") || err.contains("Bool"), "{err}");
    }

    #[test]
    fn exhaustiveness_rejects_char_without_wildcard() {
        let src = r#"
module M
val f = { c ->
    c match {
        'a' -> { 1 }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("non-exhaustive"), "{err}");
    }

    #[test]
    fn exhaustiveness_rejects_partial_list() {
        let src = r#"
module M
val f = { xs ->
    xs match {
        [] -> { 0 }
        [x] -> { x }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("non-exhaustive"), "{err}");
        assert!(err.contains("List"), "{err}");
    }

    #[test]
    fn exhaustiveness_accepts_list_empty_and_rest() {
        let src = r#"
module M
val f = { xs ->
    xs match {
        [] -> { 0 }
        [h, ..rest] -> { h }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        assert!(lower_module(&ast).is_ok());
    }

    #[test]
    fn exhaustiveness_rejects_nested_int_literal_gap() {
        let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
    o match {
        None -> { 0 }
        Some(3) -> { 1 }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("non-exhaustive"), "{err}");
        assert!(err.contains("Int"), "{err}");
    }

    #[test]
    fn exhaustiveness_rejects_nested_partial_list() {
        let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
    o match {
        None -> { 0 }
        Some([a]) -> { a }
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("non-exhaustive"), "{err}");
        assert!(err.contains("List"), "{err}");
    }

    #[test]
    fn with_rejects_ambiguous_product_field() {
        let src = r#"
module M
type Point { val x val y }
type Rect { val x val w }
val main = {
    val p = Point { x = 1, y = 2 }
    p with { x = 9 }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(
            err.contains("ambiguous") || err.contains("cannot resolve"),
            "{err}"
        );
    }

    #[test]
    fn struct_pattern_rejects_unknown_field() {
        let src = r#"
module M
type Point { val x val y }
val f = { p ->
    p match {
        Point { z } -> z
        _ -> 0
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("unknown field"), "{err}");
        assert!(err.contains('z'), "{err}");
    }

    #[test]
    fn struct_pattern_rejects_unknown_product() {
        let src = r#"
module M
type Point { val x val y }
val f = { p ->
    p match {
        Piont { x } -> x
        _ -> 0
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let err = lower_module(&ast).unwrap_err().to_string();
        assert!(err.contains("unknown product"), "{err}");
        assert!(err.contains("Piont"), "{err}");
    }

    /// Last-arm nullary ctor must still test the tag (and `MatchFail` on miss).
    #[test]
    fn last_arm_nullary_ctor_keeps_match_fail() {
        let src = r#"
module M
val f = { o ->
    o match {
        Some(x) -> x
        None -> 0
    }
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let fun = hir
            .items
            .iter()
            .find_map(|it| match it {
                Item::Fun(f) if f.name == "f" => Some(f),
                _ => None,
            })
            .expect("fun f");
        fn has_match_fail(e: &Expr) -> bool {
            match e {
                Expr::BuiltinCall {
                    name: Builtin::MatchFail,
                    ..
                } => true,
                Expr::If {
                    then_branch,
                    else_branch,
                    cond,
                    ..
                } => {
                    has_match_fail(cond)
                        || has_match_fail(then_branch)
                        || has_match_fail(else_branch)
                }
                Expr::Let { value, body, .. } => has_match_fail(value) || has_match_fail(body),
                Expr::Call { callee, args, .. } => {
                    has_match_fail(callee) || args.iter().any(has_match_fail)
                }
                Expr::Seq { stmts, .. } => stmts.iter().any(has_match_fail),
                Expr::Lambda { body, .. } => has_match_fail(body),
                Expr::Binary { left, right, .. } => has_match_fail(left) || has_match_fail(right),
                Expr::Unary { expr, .. } => has_match_fail(expr),
                Expr::BuiltinCall { args, .. } => args.iter().any(has_match_fail),
                Expr::Assign { value, .. } => has_match_fail(value),
                Expr::Loop {
                    cond,
                    body,
                    step,
                    ..
                } => {
                    has_match_fail(cond)
                        || has_match_fail(body)
                        || step.as_ref().is_some_and(|s| has_match_fail(s))
                }
                Expr::AdtNew { args, .. } => args.iter().any(has_match_fail),
                _ => false,
            }
        }
        assert!(
            has_match_fail(&fun.body),
            "last-arm `None` must remain refutable with MatchFail"
        );
    }
}
