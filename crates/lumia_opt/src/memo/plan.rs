use lumia_abi::{
    MEMO_IDX_TABLE_BYTES, MEMO_PROCESS_BYTE_CAP, MEMO_SLOTS_TABLE_BYTES, MEMO_TF_MAX_ARGS,
};
use lumia_core::CoreBinOp as BinOp;
use lumia_core::{
    block_calls, for_each_nested_block, Block, CoreFun, CoreModule, Local, MemoTf, Op, Value,
};
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use super::{MEMO_IDX_MAX_FUNS, MEMO_TF_MAX_FUNS_U32};

pub fn plan_memo_tf(module: &CoreModule) -> HashMap<String, MemoTf> {
    // One module walk for const-arg reuse (Slots hit-rate proxy); avoid
    // rescanning all bodies per candidate (§7.5.2 / Todo memo plan O(n²)).
    let const_arg_reuse = max_const_arg_reuse_by_fun(module);
    let mut next_slots = 0u32;
    let mut next_dense = 0u32;
    let mut bytes_used: usize = 0;
    let mut plan = HashMap::default();
    for f in &module.functions {
        if eligible_dense(f)
            && next_dense < MEMO_IDX_MAX_FUNS
            && bytes_used + MEMO_IDX_TABLE_BYTES <= MEMO_PROCESS_BYTE_CAP
        {
            plan.insert(f.name.clone(), MemoTf::DenseInt { id: next_dense });
            next_dense += 1;
            bytes_used += MEMO_IDX_TABLE_BYTES;
            continue;
        }
        if slots_cost_ok(f, &const_arg_reuse)
            && next_slots < MEMO_TF_MAX_FUNS_U32
            && bytes_used + MEMO_SLOTS_TABLE_BYTES <= MEMO_PROCESS_BYTE_CAP
        {
            plan.insert(f.name.clone(), MemoTf::Slots { id: next_slots });
            next_slots += 1;
            bytes_used += MEMO_SLOTS_TABLE_BYTES;
        }
    }
    plan
}

pub fn apply_memo_plan(module: &mut CoreModule, plan: &HashMap<String, MemoTf>) {
    for f in &mut module.functions {
        if f.memo.is_none() {
            if let Some(m) = plan.get(&f.name) {
                f.memo = Some(*m);
            }
        }
    }
}

fn eligible_dense(f: &CoreFun) -> bool {
    if f.is_main || !f.effect.is_pure() || f.external.is_some() {
        return false;
    }
    // Closures carry an env / FunRef payload — not user Int recursion. Use
    // FunKind, not the synthetic first param name `"env"` (that false-excluded
    // ordinary `{ env -> … }` bindings).
    if f.is_lifted_lambda() {
        return false;
    }
    if f.params.len() != 1 {
        return false;
    }
    let pty = f.param_tys.first().cloned().unwrap_or(Type::Int);
    if !matches!(pty, Type::Int) || !matches!(f.ret_ty, Type::Int | Type::Bool) {
        return false;
    }
    // Dense index table: need structural recursion on smaller `n` (§7.5.3), not mere self-calls.
    body_weight(&f.body) >= 2 && structural_int_self_recursion(f)
}

/// True when every self-call argument is proven `param - k` for some `k > 0`
/// (via `Sub` with a positive constant, and SSA aliases). Increasing / same-arg
/// recursion must not take the dense path.
fn structural_int_self_recursion(f: &CoreFun) -> bool {
    let param = match f.params.first() {
        Some(Local(p)) => *p,
        None => return false,
    };
    let mut is_param = HashSet::default();
    is_param.insert(param);
    let mut st = StructRec {
        fun: &f.name,
        is_param,
        smaller: HashSet::default(),
        self_funrefs: HashSet::default(),
        known_int: crate::ir_util::KnownScalars::new(),
        self_calls: 0,
        bad_self: false,
    };
    walk_struct_rec(&f.body, &mut st);
    st.self_calls > 0 && !st.bad_self
}

struct StructRec<'a> {
    fun: &'a str,
    is_param: HashSet<u32>,
    smaller: HashSet<u32>,
    /// Locals proven equal to `FunRef(self)` (SSA aliases included).
    self_funrefs: HashSet<u32>,
    known_int: crate::ir_util::KnownScalars,
    self_calls: usize,
    bad_self: bool,
}

fn note_self_call(st: &mut StructRec<'_>, args: &[Local]) {
    st.self_calls += 1;
    let ok = args.len() == 1 && st.smaller.contains(&args[0].0);
    if !ok {
        st.bad_self = true;
    }
}

fn walk_struct_rec(block: &Block, st: &mut StructRec<'_>) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => match value {
                Value::Int(_) | Value::Bool(_) | Value::Char(_) => {
                    st.known_int.track(local.0, value);
                }
                Value::Local(Local(src)) => {
                    if st.is_param.contains(src) {
                        st.is_param.insert(local.0);
                    }
                    if st.smaller.contains(src) {
                        st.smaller.insert(local.0);
                    }
                    if st.self_funrefs.contains(src) {
                        st.self_funrefs.insert(local.0);
                    }
                    st.known_int.track(local.0, value);
                }
                Value::FunRef(name) if name == st.fun => {
                    st.self_funrefs.insert(local.0);
                }
                Value::Binary {
                    op: BinOp::Sub,
                    left,
                    right,
                } => {
                    let left_ok = st.is_param.contains(&left.0) || st.smaller.contains(&left.0);
                    let k = st.known_int.get(right.0);
                    if left_ok && matches!(k, Some(n) if n > 0) {
                        st.smaller.insert(local.0);
                    }
                }
                Value::Call { fun, args } if fun == st.fun => {
                    note_self_call(st, args);
                }
                Value::IndirectCall { callee, args } if st.self_funrefs.contains(&callee.0) => {
                    // FunRef(self) → IndirectCall is structural self-recursion
                    // (directize may leave this shape; same param-k proof as Call).
                    note_self_call(st, args);
                }
                Value::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    let mut t = clone_struct_env(st);
                    let mut e = clone_struct_env(st);
                    walk_struct_rec(then_block, &mut t);
                    walk_struct_rec(else_block, &mut e);
                    merge_struct_rec(st, t, e);
                }
                Value::Loop { .. } => {
                    // Sequential env through header/body/latch (not fork-join).
                    // New Loop layout → `for_each_nested_block` in visit.rs.
                    let mut h = clone_struct_env(st);
                    for_each_nested_block(value, &mut |b| walk_struct_rec(b, &mut h));
                    st.self_calls += h.self_calls;
                    st.bad_self |= h.bad_self;
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn clone_struct_env<'a>(st: &StructRec<'a>) -> StructRec<'a> {
    StructRec {
        fun: st.fun,
        is_param: st.is_param.clone(),
        smaller: st.smaller.clone(),
        self_funrefs: st.self_funrefs.clone(),
        known_int: st.known_int.clone(),
        self_calls: 0,
        bad_self: false,
    }
}

fn merge_struct_rec(dst: &mut StructRec<'_>, a: StructRec<'_>, b: StructRec<'_>) {
    dst.self_calls += a.self_calls + b.self_calls;
    dst.bad_self |= a.bad_self || b.bad_self;
}

fn slots_cost_ok(f: &CoreFun, const_arg_reuse: &HashMap<String, usize>) -> bool {
    if f.is_main || !f.effect.is_pure() || f.external.is_some() {
        return false;
    }
    if f.is_lifted_lambda() {
        return false;
    }
    let n = f.params.len();
    if n == 0 || n > MEMO_TF_MAX_ARGS {
        return false;
    }
    // Heap values as raw i64 are identity-unsafe under GC; only scalar Int/Bool/Float.
    let scalar = |t: &Type| matches!(t, Type::Int | Type::Bool | Type::Float | Type::Unit);
    if !f.param_tys.iter().all(scalar) || !scalar(&f.ret_ty) {
        return false;
    }
    let c_body = body_weight(&f.body);
    if c_body < 2 {
        return false;
    }
    let recursive = block_calls(&f.body, &f.name);
    // Hit-rate proxy (§7.5.2): require *static* evidence of repeated keys.
    // Bare recursion is NOT enough — streaming unique outer keys with a recursive
    // body (e.g. collatzSteps(1..N)) thrash a 4-slot table. Structural Int
    // recursion takes the DenseInt path instead.
    let _ = recursive;
    let h_proxy = if const_arg_reuse.get(&f.name).copied().unwrap_or(0) >= 2 {
        2
    } else {
        0
    };
    h_proxy > 0 && c_body * h_proxy >= 4
}

/// Per callee: how often the most-common fully-constant argument tuple appears
/// at direct call sites (module-wide). Built in one pass over all bodies.
fn max_const_arg_reuse_by_fun(module: &CoreModule) -> HashMap<String, usize> {
    let mut freq: HashMap<String, HashMap<Vec<i64>, usize>> = HashMap::default();
    for f in &module.functions {
        collect_const_calls(&f.body, &mut crate::ir_util::KnownScalars::new(), &mut freq);
    }
    let mut out = HashMap::default();
    out.reserve(freq.len());
    for (fun, counts) in freq {
        out.insert(fun, counts.values().copied().max().unwrap_or(0));
    }
    out
}

fn collect_const_calls(
    block: &Block,
    known: &mut crate::ir_util::KnownScalars,
    freq: &mut HashMap<String, HashMap<Vec<i64>, usize>>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => match value {
                Value::Int(_) | Value::Bool(_) | Value::Char(_) | Value::Local(_) => {
                    known.track(local.0, value);
                }
                Value::Call { fun: callee, args } => {
                    if let Some(key) = known.resolve_all(args) {
                        *freq
                            .entry(callee.name.clone())
                            .or_default()
                            .entry(key)
                            .or_default() += 1;
                    }
                    known.remove(local.0);
                }
                Value::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    collect_const_calls(then_block, &mut known.clone(), freq);
                    collect_const_calls(else_block, &mut known.clone(), freq);
                    known.remove(local.0);
                }
                Value::Loop {
                    header,
                    body,
                    latch,
                } => {
                    collect_const_calls(header, &mut known.clone(), freq);
                    collect_const_calls(body, &mut known.clone(), freq);
                    collect_const_calls(latch, &mut known.clone(), freq);
                    known.remove(local.0);
                }
                _ => {
                    known.remove(local.0);
                }
            },
            _ => {}
        }
    }
}

fn body_weight(block: &Block) -> usize {
    let mut n = block.ops.len();
    for op in &block.ops {
        if let Op::Let { value, .. } = op {
            n += value_weight(value);
        }
    }
    n
}

fn value_weight(v: &Value) -> usize {
    match v {
        Value::If {
            then_block,
            else_block,
            ..
        } => 1 + body_weight(then_block) + body_weight(else_block),
        Value::Loop {
            header,
            body,
            latch,
        } => 1 + body_weight(header) + body_weight(body) + body_weight(latch),
        Value::Call { .. } | Value::IndirectCall { .. } | Value::Builtin { .. } => 1,
        _ => 0,
    }
}
