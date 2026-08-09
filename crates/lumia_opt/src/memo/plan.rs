use lumia_core::{Block, CoreFun, CoreModule, Local, MemoTf, Op, Value};
use lumia_syntax::BinOp;
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use super::{
    MEMO_IDX_MAX_FUNS, MEMO_IDX_TABLE_BYTES, MEMO_L2_MAX_ARGS, MEMO_L2_MAX_FUNS,
    MEMO_PROCESS_BYTE_CAP, MEMO_SLOTS_TABLE_BYTES,
};

pub fn plan_memo_tf(module: &CoreModule) -> HashMap<String, MemoTf> {
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
        if slots_cost_ok(f, module)
            && next_slots < MEMO_L2_MAX_FUNS
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
    if f.param_names.first().map(|s| s.as_str()) == Some("env") {
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
        known_int: HashMap::default(),
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
    known_int: HashMap<u32, i64>,
    self_calls: usize,
    bad_self: bool,
}

fn walk_struct_rec(block: &Block, st: &mut StructRec<'_>) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => match value {
                Value::Int(n) => {
                    st.known_int.insert(local.0, *n);
                }
                Value::Local(Local(src)) => {
                    if st.is_param.contains(src) {
                        st.is_param.insert(local.0);
                    }
                    if st.smaller.contains(src) {
                        st.smaller.insert(local.0);
                    }
                    if let Some(&n) = st.known_int.get(src) {
                        st.known_int.insert(local.0, n);
                    }
                }
                Value::Binary {
                    op: BinOp::Sub,
                    left,
                    right,
                } => {
                    let left_ok = st.is_param.contains(&left.0) || st.smaller.contains(&left.0);
                    let k = st.known_int.get(&right.0).copied();
                    if left_ok && matches!(k, Some(n) if n > 0) {
                        st.smaller.insert(local.0);
                    }
                }
                Value::Call { fun, args } if fun == st.fun => {
                    st.self_calls += 1;
                    let ok = args.len() == 1 && st.smaller.contains(&args[0].0);
                    if !ok {
                        st.bad_self = true;
                    }
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
                Value::Loop {
                    header,
                    body,
                    latch,
                } => {
                    let mut h = clone_struct_env(st);
                    walk_struct_rec(header, &mut h);
                    walk_struct_rec(body, &mut h);
                    walk_struct_rec(latch, &mut h);
                    st.self_calls += h.self_calls;
                    st.bad_self |= h.bad_self;
                }
                _ => {}
            },
            Op::Effect { value } => {
                if let Value::Call { fun, args } = value {
                    if fun == st.fun {
                        st.self_calls += 1;
                        let ok = args.len() == 1 && st.smaller.contains(&args[0].0);
                        if !ok {
                            st.bad_self = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn clone_struct_env<'a>(st: &StructRec<'a>) -> StructRec<'a> {
    StructRec {
        fun: st.fun,
        is_param: st.is_param.clone(),
        smaller: st.smaller.clone(),
        known_int: st.known_int.clone(),
        self_calls: 0,
        bad_self: false,
    }
}

fn merge_struct_rec(dst: &mut StructRec<'_>, a: StructRec<'_>, b: StructRec<'_>) {
    dst.self_calls += a.self_calls + b.self_calls;
    dst.bad_self |= a.bad_self || b.bad_self;
}

fn slots_cost_ok(f: &CoreFun, module: &CoreModule) -> bool {
    if f.is_main || !f.effect.is_pure() || f.external.is_some() {
        return false;
    }
    if f.param_names.first().map(|s| s.as_str()) == Some("env") {
        return false;
    }
    let n = f.params.len();
    if n == 0 || n > MEMO_L2_MAX_ARGS {
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
    let recursive = calls_self(&f.body, &f.name);
    // Hit-rate proxy (§7.5.2): require *static* evidence of repeated keys.
    // Bare recursion is NOT enough — streaming unique outer keys with a recursive
    // body (e.g. collatzSteps(1..N)) thrash a 4-slot table. Structural Int
    // recursion takes the DenseInt path instead.
    let _ = recursive;
    let h_proxy = if const_arg_reuse_count(module, &f.name) >= 2 {
        2
    } else {
        0
    };
    h_proxy > 0 && c_body * h_proxy >= 4
}

/// How many times the most-common fully-constant argument tuple is used at
/// direct call sites of `fun` (module-wide).
fn const_arg_reuse_count(module: &CoreModule, fun: &str) -> usize {
    let mut freq: HashMap<Vec<i64>, usize> = HashMap::default();
    for f in &module.functions {
        collect_const_calls(&f.body, fun, &mut HashMap::default(), &mut freq);
    }
    freq.values().copied().max().unwrap_or(0)
}

fn collect_const_calls(
    block: &Block,
    fun: &str,
    known: &mut HashMap<u32, i64>,
    freq: &mut HashMap<Vec<i64>, usize>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => match value {
                Value::Int(n) => {
                    known.insert(local.0, *n);
                }
                Value::Call { fun: callee, args } if callee == fun => {
                    let mut key = Vec::with_capacity(args.len());
                    let mut ok = true;
                    for a in args {
                        if let Some(&n) = known.get(&a.0) {
                            key.push(n);
                        } else {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        *freq.entry(key).or_default() += 1;
                    }
                    known.remove(&local.0);
                }
                Value::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    collect_const_calls(then_block, fun, &mut known.clone(), freq);
                    collect_const_calls(else_block, fun, &mut known.clone(), freq);
                    known.remove(&local.0);
                }
                Value::Loop {
                    header,
                    body,
                    latch,
                } => {
                    collect_const_calls(header, fun, &mut known.clone(), freq);
                    collect_const_calls(body, fun, &mut known.clone(), freq);
                    collect_const_calls(latch, fun, &mut known.clone(), freq);
                    known.remove(&local.0);
                }
                _ => {
                    known.remove(&local.0);
                }
            },
            Op::Effect { value } => {
                if let Value::Call { fun: callee, args } = value {
                    if callee == fun {
                        let mut key = Vec::with_capacity(args.len());
                        let mut ok = true;
                        for a in args {
                            if let Some(&n) = known.get(&a.0) {
                                key.push(n);
                            } else {
                                ok = false;
                                break;
                            }
                        }
                        if ok {
                            *freq.entry(key).or_default() += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn calls_self(block: &Block, name: &str) -> bool {
    for op in &block.ops {
        if let Op::Let { value, .. } | Op::Effect { value } = op {
            if value_calls(value, name) {
                return true;
            }
        }
    }
    false
}

fn value_calls(v: &Value, name: &str) -> bool {
    match v {
        Value::Call { fun, .. } if fun == name => true,
        Value::If {
            then_block,
            else_block,
            ..
        } => calls_self(then_block, name) || calls_self(else_block, name),
        Value::Loop {
            header,
            body,
            latch,
        } => calls_self(header, name) || calls_self(body, name) || calls_self(latch, name),
        _ => false,
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
