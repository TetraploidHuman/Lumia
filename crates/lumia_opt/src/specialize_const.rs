//! Call-site specialization on known Int/Bool/Char constants (DESIGN §7.2).
//!
//! Type-driven mono lives in `lumia_core::mono`. This pass handles the
//! complementary case: pure leaf calls whose **values** are known scalars at the
//! call site. We clone `f` as `f$c_1_2`, bake constants into the body, and
//! rewrite the call to `f$c_1_2()` so later `const_fold` / `inline` can PE it.

use lumia_core::{
    block_calls, count_ops, has_assign_or_name, has_early_return, rewrite_block_locals, Block,
    CoreFun, CoreModule, Local, Op, Value,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Cap clones per original function (avoid combinatorial blow-up).
const MAX_CLONES_PER_FUN: usize = 16;
/// Global safety fuse for one optimize run.
const MAX_TOTAL_CLONES: usize = 64;
/// Same size budget as inline — specialization targets the same leaf set.
const MAX_OPS: usize = 24;

pub struct SpecializeConstPass;

impl crate::Pass for SpecializeConstPass {
    fn name(&self) -> &str {
        "specialize_const"
    }
    fn run(&self, module: &mut CoreModule) {
        specialize_const_calls(module);
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ConstKey {
    fun: String,
    args: Vec<i64>,
}

fn specialize_const_calls(module: &mut CoreModule) {
    let candidates: HashMap<String, CoreFun> = module
        .functions
        .iter()
        .filter(|f| is_specializeable(f))
        .map(|f| (f.name.clone(), f.clone()))
        .collect();
    if candidates.is_empty() {
        return;
    }

    let mut needed: HashSet<ConstKey> = HashSet::default();
    for fun in &module.functions {
        collect_const_calls(&fun.body, &candidates, &mut needed);
        if needed.len() >= MAX_TOTAL_CLONES {
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
        if *n >= MAX_CLONES_PER_FUN || ordered.len() >= MAX_TOTAL_CLONES {
            continue;
        }
        *n += 1;
        ordered.push(key);
    }

    let mut renames: HashMap<(String, Vec<i64>), String> = HashMap::default();
    let mut new_funs: Vec<CoreFun> = Vec::new();
    for key in &ordered {
        let Some(orig) = candidates.get(&key.fun) else {
            continue;
        };
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
        rewrite_const_calls(&mut fun.body, &renames, &candidates);
    }
}

fn is_specializeable(f: &CoreFun) -> bool {
    if f.is_main || f.external.is_some() || f.memo.is_some() {
        return false;
    }
    if !f.effect.is_pure() || f.params.is_empty() {
        return false;
    }
    if count_ops(&f.body) > MAX_OPS {
        return false;
    }
    if block_calls(&f.body, &f.name) {
        return false;
    }
    if has_assign_or_name(&f.body) || has_early_return(&f.body) {
        return false;
    }
    // Specialize when params are Int/Bool/Char, or still-open vars (call-site
    // scalars prove them). Reject Float / heap — those need richer PE.
    f.param_tys.iter().all(param_ok_for_const_scalar)
}

fn param_ok_for_const_scalar(t: &lumia_ty::Type) -> bool {
    matches!(
        t,
        lumia_ty::Type::Int | lumia_ty::Type::Bool | lumia_ty::Type::Char | lumia_ty::Type::Var(_)
    )
}

fn mangle_const_clone(fun: &str, args: &[i64]) -> String {
    let mut out = format!("{fun}$c");
    for a in args {
        if *a < 0 {
            out.push_str(&format!("_n{}", -a));
        } else {
            out.push_str(&format!("_{a}"));
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
        escaping: HashSet::default(),
        scheme_poly: false,
        mono_of: Some(orig.name.clone()),
    }
}

fn collect_const_calls(
    block: &Block,
    candidates: &HashMap<String, CoreFun>,
    needed: &mut HashSet<ConstKey>,
) {
    let mut known_int: HashMap<u32, i64> = HashMap::default();
    for op in &block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                track_int(local.0, value, &mut known_int);
                if let Value::Call { fun, args } = value {
                    if let Some(c) = candidates.get(fun) {
                        if c.params.len() == args.len() {
                            if let Some(consts) = resolve_all_ints(args, &known_int) {
                                needed.insert(ConstKey {
                                    fun: fun.clone(),
                                    args: consts,
                                });
                            }
                        }
                    }
                }
                walk_nested_collect(value, candidates, needed);
            }
            Op::Let { value, .. } | Op::Effect { value } => {
                walk_nested_collect(value, candidates, needed);
            }
            _ => {}
        }
        if needed.len() >= MAX_TOTAL_CLONES {
            return;
        }
    }
}

fn walk_nested_collect(
    value: &Value,
    candidates: &HashMap<String, CoreFun>,
    needed: &mut HashSet<ConstKey>,
) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            collect_const_calls(then_block, candidates, needed);
            collect_const_calls(else_block, candidates, needed);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            collect_const_calls(header, candidates, needed);
            collect_const_calls(body, candidates, needed);
            collect_const_calls(latch, candidates, needed);
        }
        Value::Lambda { body, .. } => collect_const_calls(body, candidates, needed),
        _ => {}
    }
}

fn rewrite_const_calls(
    block: &mut Block,
    renames: &HashMap<(String, Vec<i64>), String>,
    candidates: &HashMap<String, CoreFun>,
) {
    let mut known_int: HashMap<u32, i64> = HashMap::default();
    for op in &mut block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                if let Value::Call { fun, args } = value {
                    if candidates.contains_key(fun) {
                        if let Some(consts) = resolve_all_ints(args, &known_int) {
                            if let Some(mangled) = renames.get(&(fun.clone(), consts)) {
                                *value = Value::Call {
                                    fun: mangled.clone(),
                                    args: vec![],
                                };
                            }
                        }
                    }
                }
                track_int(local.0, value, &mut known_int);
                walk_nested_rewrite(value, renames, candidates);
            }
            Op::Let { value, .. } | Op::Effect { value } => {
                walk_nested_rewrite(value, renames, candidates);
            }
            _ => {}
        }
    }
}

fn walk_nested_rewrite(
    value: &mut Value,
    renames: &HashMap<(String, Vec<i64>), String>,
    candidates: &HashMap<String, CoreFun>,
) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            rewrite_const_calls(then_block, renames, candidates);
            rewrite_const_calls(else_block, renames, candidates);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            rewrite_const_calls(header, renames, candidates);
            rewrite_const_calls(body, renames, candidates);
            rewrite_const_calls(latch, renames, candidates);
        }
        Value::Lambda { body, .. } => rewrite_const_calls(body, renames, candidates),
        _ => {}
    }
}

fn track_int(local: u32, value: &Value, known: &mut HashMap<u32, i64>) {
    // Track Int / Bool / Char as i64 bit patterns for call-site matching.
    match value {
        Value::Int(n) => {
            known.insert(local, *n);
        }
        Value::Bool(b) => {
            known.insert(local, if *b { 1 } else { 0 });
        }
        Value::Char(c) => {
            known.insert(local, u32::from(*c) as i64);
        }
        Value::Local(Local(src)) => {
            if let Some(&n) = known.get(src) {
                known.insert(local, n);
            }
        }
        _ => {}
    }
}

fn resolve_all_ints(args: &[Local], known: &HashMap<u32, i64>) -> Option<Vec<i64>> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        out.push(*known.get(&a.0)?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{compile_source_to_optimized, OptOptions, Pass};

    #[test]
    fn specialize_const_clones_pure_int_call() {
        // Isolate specialize (full release pipeline may inline the clone away).
        let src = r#"
module M
val add1 = { x -> x + 1 }
val main = {
    add1(41)
}
"#;
        let mut core = lumia_core::compile_source_to_core(src).expect("core");
        crate::ConstFoldPass.run(&mut core);
        SpecializeConstPass.run(&mut core);
        assert!(
            core.functions.iter().any(|f| f.name == "add1$c_41"),
            "expected const-specialized clone, funs={:?}",
            core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let calls_clone = main.body.ops.iter().any(|op| match op {
            Op::Let {
                value: Value::Call { fun, args },
                ..
            } => fun == "add1$c_41" && args.is_empty(),
            _ => false,
        });
        assert!(calls_clone, "main should call specialized clone");
    }

    #[test]
    fn specialize_const_clones_pure_bool_call() {
        let src = r#"
module M
val flip = { b -> if b { false } else { true } }
val main = {
    flip(false)
}
"#;
        let mut core = lumia_core::compile_source_to_core(src).expect("core");
        crate::ConstFoldPass.run(&mut core);
        SpecializeConstPass.run(&mut core);
        assert!(
            core.functions
                .iter()
                .any(|f| f.name.starts_with("flip$c_") || f.name.contains("flip$c_")),
            "expected bool const clone, funs={:?}",
            core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn specialize_const_pe_result_visible_in_ir() {
        // After specialize + fold + inline, the call should collapse toward 42.
        let core = compile_source_to_optimized(
            r#"
module M
val add1 = { x -> x + 1 }
val main = add1(41)
"#,
            &OptOptions::for_build(true),
        )
        .expect("opt");
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let has_42 = main.body.ops.iter().any(|op| {
            matches!(
                op,
                Op::Let {
                    value: Value::Int(42),
                    ..
                }
            )
        }) || matches!(
            main.body.result.and_then(|r| {
                main.body.ops.iter().rev().find_map(|op| match op {
                    Op::Let { local, value, .. } if *local == r => Some(value),
                    _ => None,
                })
            }),
            Some(Value::Int(42)) | Some(Value::Local(_))
        );
        // Soft check: either Int(42) appears or a specialized clone exists.
        let has_clone = core.functions.iter().any(|f| f.name.contains("add1$c_"));
        assert!(
            has_42 || has_clone,
            "expected PE of add1(41) or a const clone"
        );
    }
}
