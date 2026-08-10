//! Module item scanning and lowering driver.

use super::ctx::{LowerCtx, LowerError};
use super::expr::push_lowered_val;
use crate::ast::{AdtDef, AdtVariant, CtorInfo, Expr, Fun, Item, Module, ProductDef};
use crate::match_check::check_module_matches;
use lumia_syntax::VariantFields;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

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

/// Builtin trait prerequisites (DESIGN §3.6); user `trait` decls may extend.
fn builtin_trait_requires() -> HashMap<String, Vec<String>> {
    [
        ("Ord".into(), vec!["Eq".into()]),
        ("Eq".into(), vec![]),
        ("Hash".into(), vec![]),
        ("Show".into(), vec![]),
        ("Num".into(), vec![]),
    ]
    .into_iter()
    .collect()
}

struct TypeScan {
    adts: Vec<AdtDef>,
    products: Vec<ProductDef>,
    ctors: HashMap<String, CtorInfo>,
    product_map: HashMap<String, Vec<String>>,
    product_fields: HashMap<String, (String, usize)>,
    ambiguous_product_fields: HashSet<String>,
}

fn scan_type_decls(m: &lumia_syntax::Module) -> TypeScan {
    let mut adts = Vec::new();
    let mut products = Vec::new();
    let mut ctors = HashMap::default();
    let mut product_map = HashMap::default();
    let mut product_fields = HashMap::default();
    let mut ambiguous_product_fields: HashSet<String> = HashSet::default();
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
                    let fields = fields.clone();
                    product_map.insert(t.name.clone(), fields.clone());
                    products.push(ProductDef {
                        name: t.name.clone(),
                        fields,
                    });
                }
            }
        }
    }
    // Prelude ADTs: inject Option / Result when the module does not declare them.
    ensure_prelude_adt(&mut adts, &mut ctors, "Option", &[("Some", 1), ("None", 0)]);
    ensure_prelude_adt(&mut adts, &mut ctors, "Result", &[("Ok", 1), ("Err", 1)]);
    TypeScan {
        adts,
        products,
        ctors,
        product_map,
        product_fields,
        ambiguous_product_fields,
    }
}

fn collect_instances(
    m: &lumia_syntax::Module,
    adts: &[AdtDef],
    product_map: &HashMap<String, Vec<String>>,
    trait_requires: &mut HashMap<String, Vec<String>>,
) -> Result<HashSet<(String, String)>, LowerError> {
    let mut instances: HashSet<(String, String)> = HashSet::default();
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
    // Auto-derive Eq / Show for products and sums (DESIGN §3.6).
    // Hash / Ord / Num stay opt-in: Hash gates Map/Set hash tables; Ord/Num are stronger claims.
    for name in product_map
        .keys()
        .cloned()
        .chain(adts.iter().map(|a| a.name.clone()))
    {
        for tr in ["Eq", "Show"] {
            instances.insert((tr.into(), name.clone()));
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
    Ok(instances)
}

/// Pre-register top-level function names for `--parallel` FunRef maps.
fn collect_toplevel_funs(m: &lumia_syntax::Module) -> HashSet<String> {
    let mut toplevel_funs = HashSet::default();
    for item in &m.items {
        match item {
            lumia_syntax::Item::Val(v) => {
                // Keep in sync with `push_lowered_val`: bare `{ ... }` (no `->`)
                // is a zero-arg Fun (DESIGN §4.4), not only `main`.
                let is_fun = v.params.is_some()
                    || matches!(
                        v.body,
                        lumia_syntax::Expr::Lambda { .. } | lumia_syntax::Expr::Block { .. }
                    );
                if is_fun {
                    toplevel_funs.insert(v.name.clone());
                }
            }
            lumia_syntax::Item::Foreign(f) => {
                toplevel_funs.insert(f.name.clone());
            }
            _ => {}
        }
    }
    toplevel_funs
}

fn collect_fold_assoc(m: &lumia_syntax::Module) -> HashSet<String> {
    let mut toplevel_fold_assoc = HashSet::default();
    for item in &m.items {
        if let lumia_syntax::Item::Val(v) = item {
            if let Some(params) = &v.params {
                if params.len() == 2
                    && crate::list_hof::syntax_fold_body_is_associative(
                        &v.body, &params[0], &params[1],
                    )
                {
                    toplevel_fold_assoc.insert(v.name.clone());
                }
            } else if let lumia_syntax::Expr::Lambda { params, body, .. } = &v.body {
                if params.len() == 2
                    && crate::list_hof::syntax_fold_body_is_associative(
                        body, &params[0], &params[1],
                    )
                {
                    toplevel_fold_assoc.insert(v.name.clone());
                }
            }
        }
    }
    toplevel_fold_assoc
}

pub fn lower_module(m: &lumia_syntax::Module) -> Result<Module, LowerError> {
    let TypeScan {
        adts,
        products,
        ctors,
        product_map,
        mut product_fields,
        ambiguous_product_fields,
    } = scan_type_decls(m);

    let mut trait_requires = builtin_trait_requires();
    let instances = collect_instances(m, &adts, &product_map, &mut trait_requires)?;

    check_module_matches(m, &ctors, &adts, &product_map)?;

    let toplevel_funs = collect_toplevel_funs(m);
    let toplevel_fold_assoc = collect_fold_assoc(m);

    for f in &ambiguous_product_fields {
        product_fields.remove(f);
    }
    // trait name → (method name → default body)
    let mut trait_defaults: HashMap<String, HashMap<String, lumia_syntax::ValItem>> =
        HashMap::default();
    // method → trait (reject duplicate short names across traits at lower time).
    let mut method_traits: HashMap<String, String> = HashMap::default();
    for item in &m.items {
        if let lumia_syntax::Item::Trait(t) = item {
            let mut ms = HashMap::default();
            for method in &t.methods {
                ms.insert(method.name.clone(), method.clone());
                match method_traits.get(&method.name) {
                    None => {
                        method_traits.insert(method.name.clone(), t.name.clone());
                    }
                    Some(existing) if existing == &t.name => {}
                    Some(existing) => {
                        return Err(LowerError {
                            message: format!(
                                "ambiguous trait method `{}` \
                                 (defined on both `{existing}` and `{}`)",
                                method.name, t.name
                            ),
                            span: t.span,
                        });
                    }
                }
            }
            if !ms.is_empty() {
                trait_defaults.insert(t.name.clone(), ms);
            }
        }
    }

    let ctx = LowerCtx::new(
        ctors,
        product_map,
        product_fields,
        ambiguous_product_fields,
        toplevel_funs,
        toplevel_fold_assoc,
    );

    let mut items = Vec::new();
    let mut show_methods = HashMap::default();
    // (type, method) → mangled `__Trait_Type_method` (may be multi-trait).
    let mut trait_methods: HashMap<(String, String), Vec<String>> = HashMap::default();
    let mut lowered_methods: HashSet<String> = HashSet::default();
    let note_method =
        |tr: &str,
         ty: &str,
         method: &str,
         mangled: String,
         show_methods: &mut HashMap<String, String>,
         trait_methods: &mut HashMap<(String, String), Vec<String>>| {
            trait_methods
                .entry((ty.to_string(), method.to_string()))
                .or_default()
                .push(mangled.clone());
            if tr == "Show" && method == "show" {
                show_methods.insert(ty.to_string(), mangled);
            }
        };
    for item in &m.items {
        match item {
            lumia_syntax::Item::Val(v) => {
                push_lowered_val(&ctx, &mut items, v, &v.name);
            }
            lumia_syntax::Item::Type(_) | lumia_syntax::Item::Trait(_) => {}
            lumia_syntax::Item::Instance(i) => {
                for method in &i.methods {
                    let mangled = format!("__{}_{}_{}", i.trait_name, i.type_name, method.name);
                    push_lowered_val(&ctx, &mut items, method, &mangled);
                    lowered_methods.insert(mangled.clone());
                    note_method(
                        &i.trait_name,
                        &i.type_name,
                        &method.name,
                        mangled,
                        &mut show_methods,
                        &mut trait_methods,
                    );
                }
                if let Some(defaults) = trait_defaults.get(&i.trait_name) {
                    for (method_name, default) in defaults {
                        let mangled = format!("__{}_{}_{}", i.trait_name, i.type_name, method_name);
                        if lowered_methods.contains(&mangled) {
                            continue;
                        }
                        push_lowered_val(&ctx, &mut items, default, &mangled);
                        lowered_methods.insert(mangled.clone());
                        note_method(
                            &i.trait_name,
                            &i.type_name,
                            method_name,
                            mangled,
                            &mut show_methods,
                            &mut trait_methods,
                        );
                    }
                }
            }
            lumia_syntax::Item::Foreign(f) => {
                if f.abi != "C" {
                    ctx.set_err(
                        format!("unsupported foreign ABI `{}` (only \"C\")", f.abi),
                        f.span,
                    );
                }
                let params: Vec<String> = f.params.iter().map(|(n, _)| n.clone()).collect();
                let param_tys: Vec<String> = f.params.iter().map(|(_, t)| t.clone()).collect();
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
    let module = Module {
        name: m.name.clone(),
        items,
        adts,
        products,
        instances,
        show_methods,
        trait_methods,
        method_traits,
    };
    if let Some(err) = ctx.take_err() {
        return Err(err);
    }
    Ok(module)
}
