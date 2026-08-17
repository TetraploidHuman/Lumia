//! Function and module-level inference driver.

use super::Infer;
use crate::alt::apply_alt_desugars;
use crate::product_resolve::apply_product_field_rewrites;
use crate::traits::{apply_join_rewrites, apply_ufcs_rewrites};
use crate::types::{at, expr_span, Effect, NameVisibility, Type, TypeError, TypedModule};
use lumia_hir::{Fun, Item, Module};
use rustc_hash::FxHashMap as HashMap;

impl Infer {
    /// Resolve a surface ascription, expanding bare product/sum names to fresh field slots.
    pub(crate) fn resolve_type_ann(
        &mut self,
        ann: &str,
        span: lumia_syntax::Span,
        ctx: &str,
    ) -> Result<Type, TypeError> {
        let ty = parse_type_name(ann).map_err(|e| at(span, format!("{ctx}: {}", e.message())))?;
        self.expand_nominal_ascription(ty)
            .map_err(|e| at(span, format!("{ctx}: {e}")))
    }

    fn expand_nominal_ascription(&mut self, ty: Type) -> Result<Type, String> {
        match ty {
            Type::List(e) => Ok(Type::List(Box::new(self.expand_nominal_ascription(*e)?))),
            Type::Set(e) => Ok(Type::Set(Box::new(self.expand_nominal_ascription(*e)?))),
            Type::Task(e) => Ok(Type::Task(Box::new(self.expand_nominal_ascription(*e)?))),
            Type::Channel(e) => Ok(Type::Channel(Box::new(self.expand_nominal_ascription(*e)?))),
            Type::Map(k, v) => Ok(Type::Map(
                Box::new(self.expand_nominal_ascription(*k)?),
                Box::new(self.expand_nominal_ascription(*v)?),
            )),
            Type::Tuple(ts) => Ok(Type::Tuple(
                ts.into_iter()
                    .map(|t| self.expand_nominal_ascription(t))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Type::Adt { name, params } if params.is_empty() => {
                if let Some(fields) = self.products.products.get(&name) {
                    let n = fields.len();
                    return Ok(Type::Adt {
                        name,
                        params: (0..n).map(|_| self.fresh()).collect(),
                    });
                }
                if let Some(&max) = self.products.sum_max_arity.get(&name) {
                    return Ok(Type::Adt {
                        name,
                        params: (0..max).map(|_| self.fresh()).collect(),
                    });
                }
                if lumia_hir::is_option_or_result(&name) {
                    // Bare `Option` / `Result` without args — open payload slots.
                    let n = if lumia_hir::is_result(&name) { 2 } else { 1 };
                    return Ok(Type::Adt {
                        name,
                        params: (0..n).map(|_| self.fresh()).collect(),
                    });
                }
                Err(format!("unknown type `{name}`"))
            }
            Type::Adt { name, params } => Ok(Type::Adt {
                name,
                params: params
                    .into_iter()
                    .map(|t| self.expand_nominal_ascription(t))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            other => Ok(other),
        }
    }

    pub(crate) fn infer_fun(&mut self, fun: &Fun) -> Result<(Type, Effect), TypeError> {
        self.push();
        let mut pts = vec![];
        for (i, p) in fun.params.iter().enumerate() {
            let tv = if let Some(Some(ann)) = fun.param_ann.get(i) {
                self.resolve_type_ann(
                    ann,
                    expr_span(&fun.body),
                    &format!("in type ascription for `{p}`"),
                )?
            } else {
                self.fresh()
            };
            pts.push(tv.clone());
            self.bind(p.clone(), tv);
        }
        let ret_tv = if let Some(ann) = &fun.ret_ann {
            self.resolve_type_ann(
                ann,
                expr_span(&fun.body),
                "in return type ascription",
            )?
        } else {
            self.fresh()
        };
        self.ctrl.return_stack.push(ret_tv.clone());
        let saved_loop = self.ctrl.loop_depth;
        self.ctrl.loop_depth = 0;
        let body_result = self.infer_expr(&fun.body);
        self.ctrl.loop_depth = saved_loop;
        let (rt, re) = body_result?;
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
///
/// Accepts scalars, flat FFI aliases (`ListFloat`), and bracket forms
/// (`List[Float]`, `Map[Int, String]`, `Option[Int]`, `Point`, …).
pub fn parse_type_name(name: &str) -> Result<Type, TypeError> {
    parse_type_name_trimmed(name.trim())
}

fn parse_type_name_trimmed(name: &str) -> Result<Type, TypeError> {
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
        other => parse_type_name_compound(other),
    }
}

fn parse_type_name_compound(name: &str) -> Result<Type, TypeError> {
    if let Some((head, args_src)) = split_bracket_args(name)? {
        let mut args = Vec::with_capacity(args_src.len());
        for a in &args_src {
            args.push(parse_type_name_trimmed(a)?);
        }
        return match head {
            "List" if args.len() == 1 => Ok(Type::List(Box::new(args.pop().unwrap()))),
            "Set" if args.len() == 1 => Ok(Type::Set(Box::new(args.pop().unwrap()))),
            "Task" if args.len() == 1 => Ok(Type::Task(Box::new(args.pop().unwrap()))),
            "Channel" if args.len() == 1 => Ok(Type::Channel(Box::new(args.pop().unwrap()))),
            "Map" if args.len() == 2 => {
                let v = args.pop().unwrap();
                let k = args.pop().unwrap();
                Ok(Type::Map(Box::new(k), Box::new(v)))
            }
            "Tuple" if !args.is_empty() => Ok(Type::Tuple(args)),
            "Option" if args.len() == 1 => Ok(Type::Adt {
                name: lumia_hir::OPTION.name.into(),
                params: args,
            }),
            "Result" if args.len() == 1 || args.len() == 2 => {
                if args.len() == 1 {
                    args.push(Type::Int);
                }
                Ok(Type::Adt {
                    name: lumia_hir::RESULT.name.into(),
                    params: args,
                })
            }
            adt if is_type_ident(adt) => Ok(Type::Adt {
                name: adt.into(),
                params: args,
            }),
            _ => Err(TypeError::Message(format!(
                "unsupported type constructor `{head}` (use List/Map/Set/Task/Channel/Option/Result/Tuple or a declared type)"
            ))),
        };
    }
    if is_type_ident(name) {
        match name {
            "List" | "Set" | "Task" | "Channel" | "Map" | "Tuple" | "Option" | "Result" => {
                return Err(TypeError::Message(format!(
                    "`{name}` requires type arguments (e.g. `{name}[…]`)"
                )));
            }
            _ => {}
        }
        // Nominal product/sum (params filled by Infer when known).
        return Ok(Type::Adt {
            name: name.into(),
            params: vec![],
        });
    }
    Err(TypeError::Message(format!(
        "unsupported type name `{name}` (supported: Int, Bool, Float, Unit, String, Char, List[…], Map[…], Set[…], Task[…], Channel[…], Option[…], Result[…], Tuple[…], or a declared type)"
    )))
}

fn is_type_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

/// Split `Foo[A, B[C]]` → (`Foo`, [`A`, `B[C]`]); bare idents → `None`.
fn split_bracket_args(name: &str) -> Result<Option<(&str, Vec<String>)>, TypeError> {
    let Some(open) = name.find('[') else {
        return Ok(None);
    };
    if !name.ends_with(']') {
        return Err(TypeError::Message(format!(
            "malformed type `{name}`: missing `]`"
        )));
    }
    let head = name[..open].trim();
    if head.is_empty() || !is_type_ident(head) {
        return Err(TypeError::Message(format!(
            "malformed type `{name}`: bad constructor"
        )));
    }
    let inner = &name[open + 1..name.len() - 1];
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in inner.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                if depth == 0 {
                    return Err(TypeError::Message(format!(
                        "malformed type `{name}`: unmatched `]`"
                    )));
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                let piece = inner[start..i].trim();
                if piece.is_empty() {
                    return Err(TypeError::Message(format!(
                        "malformed type `{name}`: empty type argument"
                    )));
                }
                args.push(piece.to_string());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(TypeError::Message(format!(
            "malformed type `{name}`: unmatched `[`"
        )));
    }
    let last = inner[start..].trim();
    if !last.is_empty() {
        args.push(last.to_string());
    } else if !inner.trim().is_empty() {
        return Err(TypeError::Message(format!(
            "malformed type `{name}`: trailing comma"
        )));
    }
    Ok(Some((head, args)))
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

    // Type top-level bindings in dependency-SCC order so a callee is generalized
    // before callers (declaration order must not pin polymorphism).
    let sccs = super::binding_order::binding_sccs(module);
    for scc in sccs {
        // Mono placeholders only for Funs in this SCC (mutual recursion).
        for &idx in &scc {
            if let Item::Fun(f) = &module.items[idx] {
                let tv = inf.fresh();
                inf.bind(f.name.clone(), tv);
            }
        }

        let mut pending_funs: Vec<(String, Type, Effect, lumia_syntax::Span, bool)> = Vec::new();

        for &idx in &scc {
            match &module.items[idx] {
                Item::Fun(f) => {
                    inf.current_file = f.span.file;
                    let inferred = (|| -> Result<(Type, Effect), TypeError> {
                        if let Some((ptys, ret)) = &f.foreign_sig {
                            let ps: Result<Vec<_>, _> =
                                ptys.iter().map(|t| parse_foreign_type(t)).collect();
                            let ps = ps?;
                            let r = parse_foreign_type(ret)?;
                            let eff = if f.foreign_pure {
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
                    pending_funs.push((f.name.clone(), ty, eff, f.span, f.is_main));
                }
                Item::Val {
                    name,
                    body,
                    ty: ann,
                    span: val_span,
                    ..
                } => {
                    inf.current_file = val_span.file;
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
                        match inf.resolve_type_ann(
                            ann,
                            expr_span(body),
                            &format!("in type ascription for `{name}`"),
                        ) {
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
                                errors.push(e);
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
                            format!(
                                "module-level `{name}` initializer must be pure (got IO effect)"
                            ),
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
                    inf.decls.insert(name.clone(), *val_span);
                    fun_types.insert(
                        format!("__val_{name}"),
                        Type::Fun(vec![], Box::new(ty), Effect::pure()),
                    );
                }
            }
        }

        // Generalize Fun schemes only after every body in the SCC is typed.
        for (name, ty, eff, span, is_main) in pending_funs {
            for scope in inf.scopes.env.iter_mut().rev() {
                if scope.remove(&name).is_some() {
                    break;
                }
            }
            let scheme = inf.generalize(ty.clone());
            fun_schemes.insert(name.clone(), scheme.clone());
            inf.bind_scheme(name.clone(), scheme, false);
            fun_types.insert(name.clone(), ty);
            inf.decls.insert(name, span);
            if is_main {
                main_effect = eff;
                if !main_effect.has_io() {
                    main_effect = Effect::io();
                }
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
    let join_rewrites: HashMap<_, _> = std::mem::take(&mut inf.traits.join_rewrites)
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
    if !join_rewrites.is_empty() {
        apply_join_rewrites(&mut module, &join_rewrites);
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

/// Mark which sum-variant fields are recursive spines vs parametric payloads.
/// See [`lumia_hir::classify_sum_field_recursive`].
fn classify_sum_field_recursive(adt: &lumia_hir::AdtDef) -> HashMap<String, Vec<bool>> {
    lumia_hir::classify_sum_field_recursive(adt)
}
