//! Call-site specialization on known Int/Bool/Char/Float constants (DESIGN §7.2).
//!
//! Type-driven mono lives in `lumia_core::mono`. This pass handles the
//! complementary case: pure leaf calls whose **values** are known scalars at the
//! call site. We clone `f` as `f$c_1_2`, bake constants into the body, and
//! rewrite the call to `f$c_1_2()` so later `const_fold` / `inline` can PE it.

use lumia_abi::{
    SPECIALIZE_CONST_MAX_CLONES_PER_FUN, SPECIALIZE_CONST_MAX_OPS,
    SPECIALIZE_CONST_MAX_TOTAL_CLONES,
};
use lumia_core::{
    block_calls, count_ops, for_each_nested_block, for_each_nested_block_mut, has_early_return,
    max_local_in_fun, rewrite_block_locals, Block, CoreFun, CoreModule, Local, Op, Value,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::sync::Arc;

/// Shared template for `$c_` clones of one original (body cloned once into Arc).
struct ConstTemplate {
    params: Vec<Local>,
    param_tys: Vec<lumia_ty::Type>,
    ret_ty: lumia_ty::Type,
    effect: lumia_ty::Effect,
    foreign_abi: lumia_core::ForeignAbi,
    kind: lumia_core::FunKind,
    mono_of: String,
    body: Arc<Block>,
    /// `max_local_in_fun` of the original — bake remaps from here.
    max_local: u32,
}

pub struct SpecializeConstPass;

impl SpecializeConstPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        specialize_const_calls(module);
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ConstKey {
    fun: String,
    args: Vec<i64>,
}

fn specialize_const_calls(module: &mut CoreModule) {
    // Arity index only — clone bodies when emitting `$c_` clones.
    let candidate_arity: HashMap<String, usize> = module
        .functions
        .iter()
        .filter(|f| is_specializeable(f))
        .map(|f| (f.name.clone(), f.params.len()))
        .collect();
    if candidate_arity.is_empty() {
        return;
    }

    let mut needed: HashSet<ConstKey> = HashSet::default();
    for fun in &module.functions {
        collect_const_calls(&fun.body, &candidate_arity, &mut needed);
        if needed.len() >= SPECIALIZE_CONST_MAX_TOTAL_CLONES {
            break;
        }
    }
    if needed.is_empty() {
        return;
    }

    // Bound per-function clone count.
    let mut per_fun: HashMap<String, usize> = HashMap::default();
    let mut ordered: Vec<ConstKey> = Vec::new();
    for key in needed {
        let n = per_fun.entry(key.fun.clone()).or_insert(0);
        if *n >= SPECIALIZE_CONST_MAX_CLONES_PER_FUN
            || ordered.len() >= SPECIALIZE_CONST_MAX_TOTAL_CLONES
        {
            continue;
        }
        *n += 1;
        ordered.push(key);
    }

    let fun_index: HashMap<String, usize> = module
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();

    let mut renames: HashMap<(String, Vec<i64>), String> = HashMap::default();
    let mut new_funs: Vec<CoreFun> = Vec::new();
    let mut templates: HashMap<String, ConstTemplate> = HashMap::default();
    for key in &ordered {
        let Some(&idx) = fun_index.get(&key.fun) else {
            continue;
        };
        let orig = &module.functions[idx];
        if key.args.len() != orig.params.len() {
            continue;
        }
        let mangled = mangle_const_clone(&key.fun, &key.args);
        if module.functions.iter().any(|f| f.name == mangled)
            || new_funs.iter().any(|f| f.name == mangled)
        {
            renames.insert((key.fun.clone(), key.args.clone()), mangled);
            continue;
        }
        let tmpl = templates
            .entry(key.fun.clone())
            .or_insert_with(|| ConstTemplate {
                params: orig.params.clone(),
                param_tys: orig.param_tys.clone(),
                ret_ty: orig.ret_ty.clone(),
                effect: orig.effect,
                foreign_abi: orig.foreign_abi,
                kind: orig.kind,
                mono_of: orig.name.clone(),
                max_local: max_local_in_fun(orig),
                body: Arc::new(orig.body.clone()),
            });
        new_funs.push(build_const_clone(tmpl, &key.args, mangled.clone()));
        renames.insert((key.fun.clone(), key.args.clone()), mangled);
    }
    module.functions.extend(new_funs);

    for fun in &mut module.functions {
        rewrite_const_calls(&mut fun.body, &renames, &candidate_arity);
    }
}

fn is_specializeable(f: &CoreFun) -> bool {
    // Memo'd originals are OK: clones get `memo: None` and bake call-site scalars.
    if f.is_main || f.external.is_some() {
        return false;
    }
    if !f.effect.is_pure() || f.params.is_empty() {
        return false;
    }
    if count_ops(&f.body) > SPECIALIZE_CONST_MAX_OPS {
        return false;
    }
    if block_calls(&f.body, &f.name) {
        return false;
    }
    // Assign / Name are fine: we only bake immutable param scalars into the clone.
    // Early return still blocks (clone shape / PE assumptions).
    if has_early_return(&f.body) {
        return false;
    }
    // Specialize when params are Int/Bool/Char/Float, or still-open vars
    // (call-site scalars prove them). Reject heap — those need richer PE.
    f.param_tys.iter().all(param_ok_for_const_scalar)
}

fn param_ok_for_const_scalar(t: &lumia_ty::Type) -> bool {
    matches!(
        t,
        lumia_ty::Type::Int
            | lumia_ty::Type::Bool
            | lumia_ty::Type::Char
            | lumia_ty::Type::Float
            | lumia_ty::Type::Var(_)
    )
}

fn mangle_const_clone(fun: &str, args: &[i64]) -> String {
    let mut out = format!("{fun}$c");
    for a in args {
        // Small non-neg ints keep decimal; Float bits / large / neg use hex.
        if (0..=0xffff).contains(a) {
            out.push_str(&format!("_{a}"));
        } else {
            out.push_str(&format!("_x{:x}", *a as u64));
        }
    }
    out
}

fn bake_const_value(ty: &lumia_ty::Type, n: i64) -> Value {
    match ty {
        lumia_ty::Type::Bool => Value::Bool(n != 0),
        lumia_ty::Type::Char => {
            let c = char::from_u32(n as u32).unwrap_or('\0');
            Value::Char(c)
        }
        lumia_ty::Type::Float => Value::Float(f64::from_bits(n as u64)),
        _ => Value::Int(n),
    }
}

fn build_const_clone(tmpl: &ConstTemplate, args: &[i64], name: String) -> CoreFun {
    // Deep-clone from shared Arc only when emitting this `$c_` variant.
    let mut body = (*tmpl.body).clone();
    // Remap original params to fresh locals, then bind them to scalar constants
    // at the top of the body so SSA uses stay valid.
    let base = tmpl.max_local.saturating_add(1);
    let mut remap: HashMap<u32, u32> = HashMap::default();
    let mut preamble = Vec::with_capacity(args.len());
    for (i, p) in tmpl.params.iter().enumerate() {
        let fresh = Local(base + i as u32);
        remap.insert(p.0, fresh.0);
        let ty = tmpl
            .param_tys
            .get(i)
            .cloned()
            .unwrap_or(lumia_ty::Type::Int);
        preamble.push(Op::Let {
            local: fresh,
            value: bake_const_value(&ty, args[i]),
            pure_region: true,
        });
    }
    rewrite_block_locals(&mut body, &remap);
    preamble.append(&mut body.ops);
    body.ops = preamble;
    CoreFun {
        name,
        params: vec![],
        param_names: vec![],
        param_tys: vec![],
        body,
        ret_ty: tmpl.ret_ty.clone(),
        effect: tmpl.effect,
        is_main: false,
        memo: None,
        external: None,
        foreign_abi: tmpl.foreign_abi,
        escaping: HashSet::default(),
        nsw_binop_locals: Default::default(),
        safe_divisor_locals: Default::default(),
        nonneg_iv_load_locals: Default::default(),
        scheme_poly: false,
        mono_of: Some(tmpl.mono_of.clone()),
        kind: tmpl.kind,
    }
}

fn collect_const_calls(
    block: &Block,
    candidate_arity: &HashMap<String, usize>,
    needed: &mut HashSet<ConstKey>,
) {
    let mut known = crate::ir_util::KnownScalars::new();
    for op in &block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                known.track(local.0, value);
                if let Value::Call { fun, args } = value {
                    if let Some(&arity) = candidate_arity.get(fun.as_str()) {
                        if arity == args.len() {
                            if let Some(consts) = known.resolve_all(args) {
                                needed.insert(ConstKey {
                                    fun: fun.name.clone(),
                                    args: consts,
                                });
                            }
                        }
                    }
                }
                walk_nested_collect(value, candidate_arity, needed);
            }
            Op::Let { value, .. } => {
                walk_nested_collect(value, candidate_arity, needed);
            }
            _ => {}
        }
        if needed.len() >= SPECIALIZE_CONST_MAX_TOTAL_CLONES {
            return;
        }
    }
}

fn walk_nested_collect(
    value: &Value,
    candidate_arity: &HashMap<String, usize>,
    needed: &mut HashSet<ConstKey>,
) {
    // New Value region arms → extend `for_each_nested_block` in visit.rs.
    for_each_nested_block(value, &mut |b| {
        collect_const_calls(b, candidate_arity, needed);
    });
}

fn rewrite_const_calls(
    block: &mut Block,
    renames: &HashMap<(String, Vec<i64>), String>,
    candidate_arity: &HashMap<String, usize>,
) {
    let mut known = crate::ir_util::KnownScalars::new();
    for op in &mut block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                if let Value::Call { fun, args } = value {
                    if candidate_arity.contains_key(fun.as_str()) {
                        if let Some(consts) = known.resolve_all(args) {
                            if let Some(mangled) = renames.get(&(fun.name.clone(), consts)) {
                                *value = Value::Call {
                                    fun: mangled.clone().into(),
                                    args: vec![],
                                };
                            }
                        }
                    }
                }
                known.track(local.0, value);
                walk_nested_rewrite(value, renames, candidate_arity);
            }
            Op::Let { value, .. } => {
                walk_nested_rewrite(value, renames, candidate_arity);
            }
            _ => {}
        }
    }
}

fn walk_nested_rewrite(
    value: &mut Value,
    renames: &HashMap<(String, Vec<i64>), String>,
    candidate_arity: &HashMap<String, usize>,
) {
    // New Value region arms → extend `for_each_nested_block_mut` in visit.rs.
    for_each_nested_block_mut(value, &mut |b| {
        rewrite_const_calls(b, renames, candidate_arity);
    });
}

#[cfg(test)]
#[path = "specialize_const_tests.rs"]
mod tests;
