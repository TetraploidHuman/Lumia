//! Function and module-level inference driver.

use super::Infer;
use crate::traits::apply_ufcs_rewrites;
use crate::types::{at, expr_span, Effect, NameVisibility, Type, TypeError, TypedModule};
use lumia_hir::{Fun, Item, Module};
use std::collections::HashMap;

impl Infer {
    pub(crate) fn infer_fun(&mut self, fun: &Fun) -> Result<(Type, Effect), TypeError> {
        self.push();
        let mut pts = vec![];
        for p in &fun.params {
            let tv = self.fresh();
            pts.push(tv.clone());
            self.bind(p.clone(), tv);
        }
        let (rt, re) = self.infer_expr(&fun.body)?;
        // main is always an effect root
        let re = if fun.is_main {
            self.union_eff(re, Effect::io())
        } else {
            re
        };
        self.pop();
        let ty = Type::Fun(pts, Box::new(rt), re);
        Ok((ty, re))
    }
}

pub(crate) fn parse_foreign_type(name: &str) -> Result<Type, TypeError> {
    match name {
        "Int" => Ok(Type::Int),
        "Bool" => Ok(Type::Bool),
        "Float" => Ok(Type::Float),
        "Unit" => Ok(Type::Unit),
        "String" => Ok(Type::String),
        other => Err(TypeError::Message(format!(
            "unsupported foreign type `{other}` (supported: Int, Bool, Float, Unit, String)"
        ))),
    }
}

/// Options for module inference (FFI trust, etc.).
#[derive(Debug, Clone, Default)]
pub struct InferOptions {
    /// Honor `foreign "C" pure` as [`Effect::Pure`]. Without this, `pure` is rejected
    /// (FFI purity is not verified; default foreign effect is IO).
    pub trust_foreign_pure: bool,
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
    let mut inf = Infer::new(vis);
    inf.ord_instances = module
        .instances
        .iter()
        .filter(|(tr, _)| tr == "Ord")
        .map(|(_, ty)| ty.clone())
        .collect();
    inf.num_instances = module
        .instances
        .iter()
        .filter(|(tr, _)| tr == "Num")
        .map(|(_, ty)| ty.clone())
        .collect();
    inf.trait_methods = module.trait_methods.clone();
    inf.instances = module.instances.clone();
    inf.method_trait = module.method_traits.clone();
    inf.products = module
        .products
        .iter()
        .map(|p| (p.name.clone(), p.fields.clone()))
        .collect();
    let mut fun_types = HashMap::new();
    let mut fun_schemes = HashMap::new();
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
                let (ty, eff) = if let Some((ptys, ret)) = &f.foreign_sig {
                    let ps: Result<Vec<_>, _> =
                        ptys.iter().map(|t| parse_foreign_type(t)).collect();
                    let ps = ps?;
                    let r = parse_foreign_type(ret)?;
                    // Default: foreign is IO. `pure` is an honor-system claim and
                    // requires `--trust-foreign-pure` / `package.trust_foreign_pure`.
                    // Opts still never CSE/memo/inline externals (`lumia_opt`).
                    let eff = if f.foreign_pure {
                        if !opts.trust_foreign_pure {
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
                    (Type::Fun(ps, Box::new(r), eff), eff)
                } else {
                    inf.infer_fun(f)?
                };
                if let Some(existing) = inf.lookup(&f.name) {
                    inf.unify(existing, ty.clone())?;
                }
                let ty = inf.prune(ty);
                // Remove the recursive placeholder before generalize; otherwise its
                // free vars (via unify into the mono binding) look env-bound and
                // top-level `val dbl = { x -> x + x }` never gets a polymorphic scheme.
                for scope in inf.env.iter_mut().rev() {
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
            Item::Val { name, body } => {
                inf.current_file = expr_span(body).file;
                let (ty, eff) = inf.infer_expr(body)?;
                if inf.prune_eff(eff).has_io() {
                    return Err(at(
                        expr_span(body),
                        format!("module-level `{name}` initializer must be pure (got IO effect)"),
                    ));
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
    let decls = std::mem::take(&mut inf.decls);
    let ufcs_rewrites = std::mem::take(&mut inf.ufcs_rewrites);
    let mut module = module.clone();
    if !ufcs_rewrites.is_empty() {
        apply_ufcs_rewrites(&mut module, &ufcs_rewrites);
    }
    Ok(TypedModule {
        module,
        fun_types,
        fun_schemes,
        main_effect,
        type_at,
        decls,
    })
}
