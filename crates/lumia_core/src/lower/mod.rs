//! HIR → Core lowering (pure translation).
//!
//! Mid-end ABI refinement lives in [`crate::run_core_abi_pipeline`] — call it
//! from compile entries after lower, not here.

mod ctx;
mod expr;

use crate::ir::{CoreFun, CoreModule, ForeignAbi, FunKind, ListRepr, MapRepr, Op, SetRepr, Value};
use ctx::CoreLowerCtx;
use expr::lower_expr_block;
use lumia_hir::{Item, Module as HirModule};
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Lower HIR using inferred types and HM schemes (scheme-driven monomorphization).
///
/// `type_at` is the zonked expression-type table from typecheck; used to stamp
/// ground builtin results (e.g. `Channel[T]`) onto Core values.
///
/// `assert_files[span.file]` is `(path_label, source)` for injecting default
/// messages on bare `assert(cond)` — HIR stays as typed (1 arg).
pub fn lower_hir_with_schemes(
    module: &HirModule,
    fun_types: &HashMap<String, Type>,
    fun_schemes: &HashMap<String, lumia_ty::Scheme>,
    type_at: &[(lumia_syntax::Span, Type)],
    assert_files: &[(&str, &str)],
) -> Result<CoreModule, String> {
    let type_at: std::rc::Rc<[(lumia_syntax::Span, Type)]> = std::rc::Rc::from(type_at);
    let assert_files: std::rc::Rc<[(String, String)]> = assert_files
        .iter()
        .map(|(p, s)| ((*p).to_string(), (*s).to_string()))
        .collect::<Vec<_>>()
        .into();
    let toplevel_funs: HashSet<String> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fun(f) => Some(f.name.clone()),
            _ => None,
        })
        .collect();
    let toplevel_vals: HashSet<String> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Val { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    let trait_method_names: HashSet<String> = module
        .trait_methods
        .keys()
        .map(|(_, m)| m.clone())
        .collect();
    let io_funs: HashSet<String> = fun_types
        .iter()
        .filter_map(|(n, ty)| match ty {
            Type::Fun(_, _, e) if e.has_io() => Some(n.clone()),
            _ => None,
        })
        .collect();
    let mut functions = vec![];
    for item in &module.items {
        match item {
            Item::Fun(f) => {
                let mut ctx = CoreLowerCtx::new(
                    toplevel_funs.clone(),
                    toplevel_vals.clone(),
                    trait_method_names.clone(),
                    io_funs.clone(),
                    type_at.clone(),
                    assert_files.clone(),
                );
                let mut params = vec![];
                for p in &f.params {
                    let l = ctx.fresh();
                    ctx.bind_name(p.clone(), l);
                    params.push(l);
                }
                let (body, _) = lower_expr_block(&mut ctx, &f.body);
                if let Some(msg) = ctx.ice {
                    return Err(msg);
                }
                let (ret_ty, effect, param_tys) = match fun_types.get(&f.name) {
                    Some(Type::Fun(ps, r, e)) => ((**r).clone(), *e, ps.clone()),
                    _ => (
                        Type::Unit,
                        if f.is_main {
                            Effect::io()
                        } else {
                            Effect::pure()
                        },
                        vec![Type::Int; f.params.len()],
                    ),
                };
                let scheme_poly = fun_schemes
                    .get(&f.name)
                    .map(|s| s.needs_mono())
                    .unwrap_or_else(|| {
                        type_is_open(&Type::Fun(
                            param_tys.clone(),
                            Box::new(ret_ty.clone()),
                            effect,
                        ))
                    });
                functions.push(CoreFun {
                    name: f.name.clone(),
                    params,
                    param_names: f.params.clone(),
                    param_tys,
                    body,
                    ret_ty,
                    effect,
                    is_main: f.is_main,
                    memo: None,
                    external: f.external.clone(),
                    // Surface `foreign "C"` is always the platform C ABI, even if
                    // the symbol happens to look like `lumia_*`.
                    foreign_abi: if f.external.is_some() {
                        ForeignAbi::C
                    } else {
                        ForeignAbi::default()
                    },
                    escaping: HashSet::default(),
                    nsw_binop_locals: Default::default(),
                    safe_divisor_locals: Default::default(),
                    nonneg_iv_load_locals: Default::default(),
                    scheme_poly,
                    mono_of: None,
                    kind: FunKind::Normal,
                });
            }
            Item::Val {
                name, body, ty: _, ..
            } => {
                // Module-level `val` → zero-arg getter `__val_<name>` (pure).
                // Ret type must match inference so codegen roots heap returns.
                let getter = format!("__val_{name}");
                let ret_ty = match fun_types.get(&getter).or_else(|| fun_types.get(name)) {
                    Some(Type::Fun(_, r, _)) => (**r).clone(),
                    Some(t) => t.clone(),
                    None => Type::Int,
                };
                let mut ctx = CoreLowerCtx::new(
                    toplevel_funs.clone(),
                    toplevel_vals.clone(),
                    trait_method_names.clone(),
                    io_funs.clone(),
                    type_at.clone(),
                    assert_files.clone(),
                );
                let (body, _) = lower_expr_block(&mut ctx, body);
                if let Some(msg) = ctx.ice {
                    return Err(msg);
                }
                // Getters are nullary; poly lives on the value's Fun scheme / lifted body.
                let scheme_poly = fun_schemes
                    .get(name)
                    .map(|s| s.needs_mono())
                    .unwrap_or(false);
                functions.push(CoreFun {
                    name: getter,
                    params: vec![],
                    param_names: vec![],
                    param_tys: vec![],
                    body,
                    ret_ty,
                    effect: Effect::pure(),
                    is_main: false,
                    memo: None,
                    external: None,
                    foreign_abi: ForeignAbi::C,
                    escaping: HashSet::default(),
                    nsw_binop_locals: Default::default(),
                    safe_divisor_locals: Default::default(),
                    nonneg_iv_load_locals: Default::default(),
                    scheme_poly,
                    mono_of: None,
                    kind: FunKind::ValGetter,
                });
            }
        }
    }
    let hash_adts: HashSet<String> = module
        .instances
        .iter()
        .filter(|(tr, _)| tr == "Hash")
        .map(|(_, ty)| ty.clone())
        .collect();
    let mut adt_variant_names: HashMap<String, Vec<String>> = HashMap::default();
    for adt in &module.adts {
        let mut names = vec![String::new(); adt.variants.len()];
        for v in &adt.variants {
            let idx = v.tag as usize;
            if idx >= names.len() {
                names.resize(idx + 1, String::new());
            }
            names[idx] = v.name.clone();
        }
        adt_variant_names.insert(adt.name.clone(), names);
    }
    for prod in &module.products {
        // Products are tag-0 payloads; print the type name.
        adt_variant_names.insert(prod.name.clone(), vec![prod.name.clone()]);
    }
    let sum_max_arity: HashMap<String, usize> = module
        .adts
        .iter()
        .map(|a| {
            // Match ty: parametric slots only (recursive spines are `Self`).
            let total = sum_parametric_arity(a);
            (a.name.clone(), total)
        })
        .collect();
    let mut core = CoreModule {
        name: module.name.clone(),
        functions,
        hash_adts,
        trait_methods: module.trait_methods.clone(),
        adt_variant_names,
        sum_max_arity,
        channel_elem_hint: None,
        channel_elem_by_local: Default::default(),
        channel_elem_conflicts: Vec::new(),
    };
    ensure_prelude_ctor_stubs(&mut core);
    Ok(core)
}

fn type_is_open(t: &Type) -> bool {
    match t {
        Type::Var(_) => true,
        Type::Fun(ps, r, _) => ps.iter().any(type_is_open) || type_is_open(r),
        Type::List(e) | Type::Set(e) | Type::Task(e) | Type::Channel(e) => type_is_open(e),
        Type::Map(k, v) => type_is_open(k) || type_is_open(v),
        Type::Tuple(ts) | Type::TuplePrefix(ts) | Type::Adt { params: ts, .. } => {
            ts.iter().any(type_is_open)
        }
        _ => false,
    }
}

/// Count type parameters for a sum ADT — see [`lumia_hir::sum_parametric_arity`].
fn sum_parametric_arity(adt: &lumia_hir::AdtDef) -> usize {
    lumia_hir::sum_parametric_arity(adt)
}

/// Nullary empty-container stubs for first-class `listOf` / `mapOf` / `setOf`.
fn ensure_prelude_ctor_stubs(core: &mut CoreModule) {
    let mut needed: HashSet<&'static str> = HashSet::default();
    for f in &core.functions {
        crate::visit::for_each_block_dfs(&f.body, &mut |b| {
            for op in &b.ops {
                if let Op::Let {
                    value: Value::FunRef(n),
                    ..
                } = op
                {
                    match n.as_str() {
                        "__prelude_listOf" => {
                            needed.insert("__prelude_listOf");
                        }
                        "__prelude_mapOf" => {
                            needed.insert("__prelude_mapOf");
                        }
                        "__prelude_setOf" => {
                            needed.insert("__prelude_setOf");
                        }
                        _ => {}
                    }
                }
            }
        });
    }
    let existing: HashSet<String> = core.functions.iter().map(|f| f.name.clone()).collect();
    for name in needed {
        if existing.contains(name) {
            continue;
        }
        let (alloc, ret_ty) = match name {
            "__prelude_listOf" => (
                Value::AllocList {
                    elems: vec![],
                    repr: ListRepr::HeapList,
                },
                Type::List(Box::new(Type::Int)),
            ),
            "__prelude_mapOf" => (
                Value::AllocMap {
                    flat_pairs: vec![],
                    repr: MapRepr::HashOrdered,
                },
                Type::Map(Box::new(Type::Int), Box::new(Type::Int)),
            ),
            "__prelude_setOf" => (
                Value::AllocSet {
                    elems: vec![],
                    repr: SetRepr::HeapSet,
                },
                Type::Set(Box::new(Type::Int)),
            ),
            _ => continue,
        };
        let local = crate::ir::Local(0);
        core.functions.push(CoreFun {
            name: name.into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            body: crate::ir::Block {
                ops: vec![Op::Let {
                    local,
                    value: alloc,
                    pure_region: true,
                }],
                result: Some(local),
            },
            ret_ty,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: ForeignAbi::C,
            escaping: HashSet::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        });
    }
}
