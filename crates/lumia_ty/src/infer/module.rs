//! Function and module-level inference driver.

use super::Infer;
use crate::alt::apply_alt_desugars;
use crate::product_resolve::apply_product_field_rewrites;
use crate::traits::apply_ufcs_rewrites;
use crate::types::{at, expr_span, Effect, NameVisibility, Type, TypeError, TypedModule};
use lumia_hir::{Fun, Item, Module};
use rustc_hash::FxHashMap as HashMap;

impl Infer {
    pub(crate) fn infer_fun(&mut self, fun: &Fun) -> Result<(Type, Effect), TypeError> {
        self.push();
        let mut pts = vec![];
        for (i, p) in fun.params.iter().enumerate() {
            let tv = if let Some(Some(ann)) = fun.param_ann.get(i) {
                parse_type_name(ann).map_err(|e| {
                    at(
                        expr_span(&fun.body),
                        format!("in type ascription for `{p}`: {}", e.message()),
                    )
                })?
            } else {
                self.fresh()
            };
            pts.push(tv.clone());
            self.bind(p.clone(), tv);
        }
        let ret_tv = if let Some(ann) = &fun.ret_ann {
            parse_type_name(ann).map_err(|e| {
                at(
                    expr_span(&fun.body),
                    format!("in return type ascription: {}", e.message()),
                )
            })?
        } else {
            self.fresh()
        };
        self.ctrl.return_stack.push(ret_tv.clone());
        let (rt, re) = self.infer_expr(&fun.body)?;
        self.unify_at(expr_span(&fun.body), rt, ret_tv.clone())?;
        self.ctrl.return_stack.pop();
        // main is always an effect root
        let re = if fun.is_main {
            self.union_eff(re, Effect::io())
        } else {
            re
        };
        self.pop();
        let ty = Type::Fun(pts, Box::new(ret_tv), re);
        Ok((ty, re))
    }
}

/// Resolve a surface type name used in ascriptions / foreign signatures.
pub fn parse_type_name(name: &str) -> Result<Type, TypeError> {
    match name {
        "Int" => Ok(Type::Int),
        "Bool" => Ok(Type::Bool),
        "Float" => Ok(Type::Float),
        "Unit" => Ok(Type::Unit),
        "String" => Ok(Type::String),
        "Char" => Ok(Type::Char),
        // Flat aliases (foreign param syntax is a single ident).
        "ListString" => Ok(Type::List(Box::new(Type::String))),
        "ListFloat" => Ok(Type::List(Box::new(Type::Float))),
        other => Err(TypeError::Message(format!(
            "unsupported type name `{other}` (supported: Int, Bool, Float, Unit, String, Char, ListString, ListFloat)"
        ))),
    }
}

/// Alias for FFI signatures (same surface names).
pub(crate) fn parse_foreign_type(name: &str) -> Result<Type, TypeError> {
    parse_type_name(name)
}

/// Options for module inference (FFI trust, etc.).
#[derive(Debug, Clone, Default)]
pub struct InferOptions {
    /// Honor `foreign "C" pure` as [`Effect::Pure`]. Without this, `pure` is rejected
    /// (FFI purity is not verified; default foreign effect is IO).
    pub trust_foreign_pure: bool,
    /// When set, collect per-item type errors and keep typing the rest (IDE).
    pub recovering: bool,
}

pub fn infer_module(module: &Module) -> Result<TypedModule, TypeError> {
    infer_module_with_visibility(module, NameVisibility::default())
}

pub fn infer_module_with_visibility(
    module: &Module,
    vis: NameVisibility,
) -> Result<TypedModule, TypeError> {
    infer_module_with_options(module, vis, InferOptions::default())
}

pub fn infer_module_with_options(
    module: &Module,
    vis: NameVisibility,
    opts: InferOptions,
) -> Result<TypedModule, TypeError> {
    let (typed, errors) = infer_module_inner(module, vis, opts);
    match errors.into_iter().next() {
        Some(e) => Err(e),
        None => Ok(typed.expect("typed module without errors")),
    }
}

/// Infer with per-item recovery: returns whatever typed successfully plus all errors.
pub fn infer_module_recovering(
    module: &Module,
    vis: NameVisibility,
    opts: InferOptions,
) -> (Option<TypedModule>, Vec<TypeError>) {
    let mut opts = opts;
    opts.recovering = true;
    let (typed, errors) = infer_module_inner(module, vis, opts);
    (typed, errors)
}

fn infer_module_inner(
    module: &Module,
    vis: NameVisibility,
    opts: InferOptions,
) -> (Option<TypedModule>, Vec<TypeError>) {
    let mut errors = Vec::new();
    let mut inf = Infer::new(vis);
    inf.traits.ord_instances = module
        .instances
        .iter()
        .filter(|(tr, _)| tr == "Ord")
        .map(|(_, ty)| ty.clone())
        .collect();
    inf.traits.num_instances = module
        .instances
        .iter()
        .filter(|(tr, _)| tr == "Num")
        .map(|(_, ty)| ty.clone())
        .collect();
    inf.traits.trait_methods = module
        .trait_methods
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    inf.traits.instances = module.instances.iter().cloned().collect();
    inf.traits.method_trait = module
        .method_traits
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    inf.products.products = module
        .products
        .iter()
        .map(|p| (p.name.clone(), p.fields.clone()))
        .collect();
    inf.products.sum_field_recursive = HashMap::default();
    inf.products.sum_max_arity = HashMap::default();
    inf.products.sum_ctors = HashMap::default();
    for a in &module.adts {
        let kinds = classify_sum_field_recursive(a);
        let mut param_offset = 0usize;
        let mut total_params = 0usize;
        for v in &a.variants {
            let rec = kinds.get(v.name.as_str()).cloned().unwrap_or_default();
            let parametric = rec.iter().filter(|r| !**r).count();
            inf.products
                .sum_ctors
                .insert(v.name.clone(), (a.name.clone(), v.arity, param_offset));
            inf.products
                .sum_field_recursive
                .insert(v.name.clone(), rec);
            param_offset += parametric;
            total_params += parametric;
        }
        inf.products
            .sum_max_arity
            .insert(a.name.clone(), total_params);
    }
    let mut fun_types = HashMap::default();
    let mut fun_schemes = HashMap::default();
    let mut main_effect = Effect::pure();

    // First pass: bind function names with fresh types for recursion
    for item in &module.items {
        if let Item::Fun(f) = item {
            let tv = inf.fresh();
            inf.bind(f.name.clone(), tv);
        }
    }

    for item in &module.items {
        match item {
            Item::Fun(f) => {
                inf.current_file = expr_span(&f.body).file;
                let inferred = (|| -> Result<(Type, Effect), TypeError> {
                    if let Some((ptys, ret)) = &f.foreign_sig {
                        let ps: Result<Vec<_>, _> =
                            ptys.iter().map(|t| parse_foreign_type(t)).collect();
                        let ps = ps?;
                        let r = parse_foreign_type(ret)?;
                        let eff = if f.foreign_pure {
                            // `lumia_*` runtime symbols are part of the distribution;
                            // other `pure` FFI still needs an explicit trust flag.
                            let runtime_sym = f
                                .external
                                .as_deref()
                                .is_some_and(|s| s.starts_with("lumia_"));
                            if !opts.trust_foreign_pure && !runtime_sym {
                                return Err(at(
                                    expr_span(&f.body),
                                    "`foreign \"C\" pure` requires `--trust-foreign-pure` \
                                     (or `package.trust_foreign_pure = true`); FFI purity is \
                                     not verified — omit `pure` to type the import as IO",
                                ));
                            }
                            Effect::pure()
                        } else {
                            Effect::io()
                        };
                        Ok((Type::Fun(ps, Box::new(r), eff), eff))
                    } else {
                        inf.infer_fun(f)
                    }
                })();
                let (ty, eff) = match inferred {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(e);
                        if !opts.recovering {
                            return (None, errors);
                        }
                        continue;
                    }
                };
                if let Some(existing) = inf.lookup(&f.name) {
                    if let Err(e) = inf.unify(existing, ty.clone()) {
                        errors.push(e);
                        if !opts.recovering {
                            return (None, errors);
                        }
                        continue;
                    }
                }
                let ty = inf.prune(ty);
                // Remove the recursive placeholder before generalize; otherwise its
                // free vars (via unify into the mono binding) look env-bound and
                // top-level `val dbl = { x -> x + x }` never gets a polymorphic scheme.
                for scope in inf.scopes.env.iter_mut().rev() {
                    if scope.remove(&f.name).is_some() {
                        break;
                    }
                }
                let scheme = inf.generalize(ty.clone());
                fun_schemes.insert(f.name.clone(), scheme.clone());
                inf.bind_scheme(f.name.clone(), scheme, false);
                fun_types.insert(f.name.clone(), ty);
                // Decl span: use body span as stand-in for foreign/unit; funs lack item span in HIR.
                inf.decls.insert(f.name.clone(), expr_span(&f.body));
                if f.is_main {
                    main_effect = eff;
                    if !eff.has_io() {
                        main_effect = Effect::io();
                    }
                }
            }
            Item::Val {
                name,
                body,
                ty: ann,
            } => {
                inf.current_file = expr_span(body).file;
                let (mut ty, eff) = match inf.infer_expr(body) {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(e);
                        if !opts.recovering {
                            return (None, errors);
                        }
                        continue;
                    }
                };
                if let Some(ann) = ann {
                    match parse_type_name(ann) {
                        Ok(expect) => {
                            if let Err(e) =
                                inf.unify_at(expr_span(body), ty.clone(), expect.clone())
                            {
                                errors.push(e);
                                if !opts.recovering {
                                    return (None, errors);
                                }
                                continue;
                            }
                            ty = expect;
                        }
                        Err(e) => {
                            errors.push(at(
                                expr_span(body),
                                format!("in type ascription for `{name}`: {}", e.message()),
                            ));
                            if !opts.recovering {
                                return (None, errors);
                            }
                            continue;
                        }
                    }
                }
                if inf.prune_eff(eff).has_io() {
                    let e = at(
                        expr_span(body),
                        format!("module-level `{name}` initializer must be pure (got IO effect)"),
                    );
                    errors.push(e);
                    if !opts.recovering {
                        return (None, errors);
                    }
                    continue;
                }
                let ty = inf.prune(ty);
                let scheme = inf.generalize(ty.clone());
                fun_schemes.insert(name.clone(), scheme.clone());
                inf.bind_scheme(name.clone(), scheme, false);
                inf.decls.insert(name.clone(), expr_span(body));
                // Zero-arg getter used by Core lowering / codegen GC rooting.
                fun_types.insert(
                    format!("__val_{name}"),
                    Type::Fun(vec![], Box::new(ty), Effect::pure()),
                );
            }
        }
    }

    // Resolve open effect vars (unconstrained → Pure; Io bound via later call sites).
    for ty in fun_types.values_mut() {
        *ty = inf.zonk_type(ty.clone());
    }
    for sch in fun_schemes.values_mut() {
        sch.ty = inf.zonk_type(sch.ty.clone());
    }
    main_effect = inf.zonk_eff(main_effect);

    let type_at_raw = std::mem::take(&mut inf.type_at);
    let type_at: Vec<_> = type_at_raw
        .into_iter()
        .map(|(sp, t)| (sp, inf.zonk_type(t)))
        .collect();
    let decls: HashMap<_, _> = std::mem::take(&mut inf.decls).into_iter().collect();
    let ufcs_rewrites: HashMap<_, _> = std::mem::take(&mut inf.traits.ufcs_rewrites)
        .into_iter()
        .collect();
    let alt_kinds: HashMap<_, _> = std::mem::take(&mut inf.ctrl.alt_kinds)
        .into_iter()
        .collect();
    let field_rewrites: HashMap<_, _> = std::mem::take(&mut inf.ctrl.product_field_rewrites)
        .into_iter()
        .collect();
    let with_rewrites: HashMap<_, _> = std::mem::take(&mut inf.ctrl.with_rewrites)
        .into_iter()
        .collect();
    let mut module = module.clone();
    if !ufcs_rewrites.is_empty() {
        apply_ufcs_rewrites(&mut module, &ufcs_rewrites);
    }
    apply_alt_desugars(&mut module, &alt_kinds);
    apply_product_field_rewrites(&mut module, &field_rewrites, &with_rewrites);
    (
        Some(TypedModule {
            module,
            fun_types: fun_types.into_iter().collect(),
            fun_schemes: fun_schemes.into_iter().collect(),
            main_effect,
            type_at,
            decls,
        }),
        errors,
    )
}

/// Mark which sum-variant fields are recursive spines (`Nat.S`, `UList.Cons` tail)
/// vs parametric payloads (`UList` head, `Expr.Lit`/`Add`, `Either`, `Shape`).
///
/// Without a nullary base, arity alone cannot tell `Expr.Add` from `Shape.Rect`,
/// so non-nullary sums keep every field parametric; recursive values still type
/// as `Adt[…]` when nested as payloads.
fn classify_sum_field_recursive(adt: &lumia_hir::AdtDef) -> HashMap<String, Vec<bool>> {
    // Prelude Option/Result keep parametric payloads (Result is also special-cased
    // in `infer_adt_new`). Treating `Some` like `Nat.S` would require `Some(3): Option`.
    if adt.name == "Option" || adt.name == "Result" {
        return adt
            .variants
            .iter()
            .map(|v| (v.name.clone(), vec![false; v.arity]))
            .collect();
    }
    let arities: Vec<usize> = adt.variants.iter().map(|v| v.arity).collect();
    let has_nullary = arities.iter().any(|&a| a == 0);
    let only_nullary_unary = arities.iter().all(|&a| a <= 1);
    let mut out = HashMap::default();
    for v in &adt.variants {
        let rec = if v.arity == 0 {
            vec![]
        } else if only_nullary_unary && has_nullary {
            // `Nat { Z S(n) }`: the unary payload is `Nat` itself.
            vec![true; v.arity]
        } else if has_nullary && v.arity >= 2 {
            // `UList { Nil Cons(h, t) }`: last field recursive, earlier parametric.
            let mut k = vec![false; v.arity];
            if let Some(last) = k.last_mut() {
                *last = true;
            }
            k
        } else {
            // `Either` / `Shape` / `Expr`: all parametric (concatenated slots).
            vec![false; v.arity]
        };
        out.insert(v.name.clone(), rec);
    }
    out
}
