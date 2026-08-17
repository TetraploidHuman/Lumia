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
    block_calls, count_ops, has_early_return, rewrite_block_locals, Block, CoreFun, CoreModule,
    Local, Op, Value,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

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
        if *n >= SPECIALIZE_CONST_MAX_CLONES_PER_FUN || ordered.len() >= SPECIALIZE_CONST_MAX_TOTAL_CLONES {
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
        new_funs.push(build_const_clone(orig, &key.args, mangled.clone()));
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

fn build_const_clone(orig: &CoreFun, args: &[i64], name: String) -> CoreFun {
    let mut body = orig.body.clone();
    // Remap original params to fresh locals, then bind them to scalar constants
    // at the top of the body so SSA uses stay valid.
    let base = lumia_core::max_local_in_fun(orig).saturating_add(1);
    let mut remap: HashMap<u32, u32> = HashMap::default();
    let mut preamble = Vec::with_capacity(args.len());
    for (i, p) in orig.params.iter().enumerate() {
        let fresh = Local(base + i as u32);
        remap.insert(p.0, fresh.0);
        let ty = orig
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
        ret_ty: orig.ret_ty.clone(),
        effect: orig.effect,
        is_main: false,
        memo: None,
        external: None,
        foreign_abi: lumia_core::ForeignAbi::C,
        escaping: HashSet::default(),
        scheme_poly: false,
        mono_of: Some(orig.name.clone()),
        kind: orig.kind,
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
                    if let Some(&arity) = candidate_arity.get(fun) {
                        if arity == args.len() {
                            if let Some(consts) = known.resolve_all(args) {
                                needed.insert(ConstKey {
                                    fun: fun.clone(),
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
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            collect_const_calls(then_block, candidate_arity, needed);
            collect_const_calls(else_block, candidate_arity, needed);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            collect_const_calls(header, candidate_arity, needed);
            collect_const_calls(body, candidate_arity, needed);
            collect_const_calls(latch, candidate_arity, needed);
        }
        Value::Lambda { body, .. } => collect_const_calls(body, candidate_arity, needed),
        _ => {}
    }
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
                    if candidate_arity.contains_key(fun) {
                        if let Some(consts) = known.resolve_all(args) {
                            if let Some(mangled) = renames.get(&(fun.clone(), consts)) {
                                *value = Value::Call {
                                    fun: mangled.clone(),
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
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            rewrite_const_calls(then_block, renames, candidate_arity);
            rewrite_const_calls(else_block, renames, candidate_arity);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            rewrite_const_calls(header, renames, candidate_arity);
            rewrite_const_calls(body, renames, candidate_arity);
            rewrite_const_calls(latch, renames, candidate_arity);
        }
        Value::Lambda { body, .. } => rewrite_const_calls(body, renames, candidate_arity),
        _ => {}
    }
}

#[cfg(test)]
#[path = "specialize_const_tests.rs"]
mod tests;
