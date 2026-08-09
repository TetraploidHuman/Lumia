//! Transparent result reuse (DESIGN §7.5).
//!
//! - **§7.5.1-A local**: CSE + const-fold / copy-prop + LICM (no `T_f`)
//! - **§7.5.1-B `T_f`**: bounded cross-call table; representation = Slots | DenseInt


use lumia_core::{Block, CoreFun, CoreModule, Local, MemoTf, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::{BinOp, UnOp};
use lumia_ty::Type;
use std::collections::{HashMap, HashSet};

pub struct MemoL0Pass;
impl crate::Pass for MemoL0Pass {
    fn name(&self) -> &str {
        "memo_l0"
    }
    fn run(&self, module: &mut CoreModule) {
        for f in &mut module.functions {
            const_fold_block(&mut f.body);
            copy_prop_block(&mut f.body);
        }
    }
}

pub struct MemoL1Pass;
impl crate::Pass for MemoL1Pass {
    fn name(&self) -> &str {
        "memo_l1"
    }
    fn run(&self, module: &mut CoreModule) {
        for f in &mut module.functions {
            licm_block(&mut f.body);
        }
    }
}

pub struct MemoTfPass;
impl crate::Pass for MemoTfPass {
    fn name(&self) -> &str {
        "memo_tf"
    }
    fn run(&self, _module: &mut CoreModule) {
        // Intentionally a no-op. `T_f` must be planned on the *pre-CSE* module
        // (`plan_memo_tf` in `optimize`); applying a fresh plan here would drop
        // const-reuse evidence that CSE folds away (§7.5.2).
    }
}

/// Decide `T_f` **before** CSE: duplicate const calls are reuse evidence, but CSE
/// collapses them to one site and would otherwise hide Hits (§7.5.2).
pub fn plan_memo_tf(module: &CoreModule) -> HashMap<String, MemoTf> {
    let mut next_slots = 0u32;
    let mut next_dense = 0u32;
    let mut bytes_used: usize = 0;
    let mut plan = HashMap::new();
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

/// Must stay in sync with `lumia_rt` caps (§7.5.0 dual hard tops).
pub const MEMO_L2_MAX_FUNS: u32 = 64;
pub const MEMO_L2_MAX_ARGS: usize = 4;
pub const MEMO_L2_SLOTS: usize = 4;
pub const MEMO_IDX_MAX_FUNS: u32 = 16;
/// Keys outside `0..MEMO_IDX_CAP` are never cached (DESIGN §7.5 hard bound).
pub const MEMO_IDX_CAP: u32 = 4096;
pub const MEMO_IDX_TABLE_BYTES: usize = (MEMO_IDX_CAP as usize) * (1 + 8);
pub const MEMO_SLOTS_TABLE_BYTES: usize =
    MEMO_L2_SLOTS * (1 + MEMO_L2_MAX_ARGS * 8 + 8);
/// Process-level transparent Memo byte budget (versioned; sync with `lumia_rt`).
pub const MEMO_PROCESS_BYTE_CAP: usize = 2 * 1024 * 1024;

// ─── L0: CSE ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ExprKey {
    Int(i64),
    Bool(bool),
    Float(u64),
    Char(char),
    String(String),
    Unary(UnOp, u32),
    Binary(BinOp, u32, u32),
    Builtin(String, Vec<u32>),
    Call(String, Vec<u32>),
}

pub fn cse_module(module: &mut CoreModule) {
    // Foreign (`external`) must never be CSE'd: even trusted
    // `foreign "C" pure` is an honor-system claim; libc calls like `getpid` /
    // `getenv` are not referentially transparent. Inline already skips `external`.
    let pure_funs: HashSet<String> = module
        .functions
        .iter()
        .filter(|f| f.effect.is_pure() && f.external.is_none())
        .map(|f| f.name.clone())
        .collect();
    for f in &mut module.functions {
        cse_block(&mut f.body, &pure_funs);
    }
}

fn cse_block(block: &mut Block, pure_funs: &HashSet<String>) {
    let mut seen: HashMap<ExprKey, u32> = HashMap::new();
    let mut rewrite: HashMap<u32, u32> = HashMap::new();

    for op in &mut block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                rewrite_value(value, &rewrite);
                if let Some(key) = expr_key(value, pure_funs) {
                    if let Some(&prev) = seen.get(&key) {
                        rewrite.insert(local.0, prev);
                        *value = Value::Local(Local(prev));
                    } else {
                        seen.insert(key, local.0);
                    }
                }
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    cse_block(then_block, pure_funs);
                    cse_block(else_block, pure_funs);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    cse_block(header, pure_funs);
                    cse_block(body, pure_funs);
                    cse_block(latch, pure_funs);
                }
            }
            Op::Let { value, .. } => {
                rewrite_value(value, &rewrite);
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    cse_block(then_block, pure_funs);
                    cse_block(else_block, pure_funs);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    cse_block(header, pure_funs);
                    cse_block(body, pure_funs);
                    cse_block(latch, pure_funs);
                }
            }
            Op::Effect { value } => rewrite_value(value, &rewrite),
            Op::Assign { value, .. } => {
                if let Some(&r) = rewrite.get(&value.0) {
                    *value = Local(r);
                }
            }
            Op::Break | Op::Continue => {}
        }
    }
    if let Some(r) = block.result {
        if let Some(&nr) = rewrite.get(&r.0) {
            block.result = Some(Local(nr));
        }
    }
}

fn expr_key(value: &Value, pure_funs: &HashSet<String>) -> Option<ExprKey> {
    match value {
        Value::Int(n) => Some(ExprKey::Int(*n)),
        Value::Bool(b) => Some(ExprKey::Bool(*b)),
        Value::Float(f) => Some(ExprKey::Float(f.to_bits())),
        Value::Char(c) => Some(ExprKey::Char(*c)),
        Value::String(s) => Some(ExprKey::String(s.clone())),
        // Trapping arithmetic must not CSE across divergent paths (§2.4).
        Value::Unary {
            op: UnOp::Neg,
            ..
        } => None,
        Value::Unary { op, operand } => Some(ExprKey::Unary(*op, operand.0)),
        Value::Binary {
            op: BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem,
            ..
        } => None,
        Value::Binary { op, left, right } => Some(ExprKey::Binary(*op, left.0, right.0)),
        Value::Builtin { name, args } if builtin_is_pure(name) => Some(ExprKey::Builtin(
            format!("{name:?}"),
            args.iter().map(|a| a.0).collect(),
        )),
        Value::Call { fun, args } if pure_funs.contains(fun) => {
            Some(ExprKey::Call(fun.clone(), args.iter().map(|a| a.0).collect()))
        }
        _ => None,
    }
}

fn builtin_is_pure(b: &Builtin) -> bool {
    // Align with LICM: do not CSE traps / effects / parallel map (same key in
    // divergent control flow must not erase a failing path).
    !builtin_may_trap_or_effect(b)
}

pub(crate) fn rewrite_value(v: &mut Value, rewrite: &HashMap<u32, u32>) {
    let map_l = |l: &mut Local| {
        if let Some(&r) = rewrite.get(&l.0) {
            *l = Local(r);
        }
    };
    match v {
        Value::Local(l) => map_l(l),
        Value::Binary { left, right, .. } => {
            map_l(left);
            map_l(right);
        }
        Value::Unary { operand, .. } => map_l(operand),
        Value::Call { args, .. }
        | Value::Builtin { args, .. }
        | Value::AllocList { elems: args, .. }
        | Value::AllocSet { elems: args, .. }
        | Value::AllocMap {
            flat_pairs: args, ..
        }
        | Value::AllocAdt { fields: args, .. }
        | Value::AllocClosure {
            captures: args, ..
        } => {
            for a in args {
                map_l(a);
            }
        }
        Value::ClosureCap { env, .. } => map_l(env),
        Value::IndirectCall { callee, args } => {
            map_l(callee);
            for a in args {
                map_l(a);
            }
        }
        Value::If { cond, .. } => map_l(cond),
        Value::Loop { .. }
        | Value::Lambda { .. }
        | Value::FunRef(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::Name(_) => {}
    }
}

// ─── L0: const-fold + copy-prop ────────────────────────────────────────────

fn const_fold_block(block: &mut Block) {
    let mut known_int: HashMap<u32, i64> = HashMap::new();
    // Local → element locals of a literal `AllocList` (for ListLen / ListGet fold).
    let mut known_list: HashMap<u32, Vec<Local>> = HashMap::new();
    // Local → field locals of a literal `AllocAdt` (for AdtField fold).
    let mut known_adt: HashMap<u32, Vec<Local>> = HashMap::new();
    for op in &mut block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                match value {
                    Value::Int(n) => {
                        known_int.insert(local.0, *n);
                    }
                    Value::Bool(b) => {
                        known_int.insert(local.0, if *b { 1 } else { 0 });
                    }
                    Value::Local(Local(src)) => {
                        // Track constants through aliases; keep Local for CSE sharing.
                        if let Some(&n) = known_int.get(src) {
                            known_int.insert(local.0, n);
                        }
                        if let Some(elems) = known_list.get(src).cloned() {
                            known_list.insert(local.0, elems);
                        }
                        if let Some(fields) = known_adt.get(src).cloned() {
                            known_adt.insert(local.0, fields);
                        }
                    }
                    Value::AllocList { elems, .. } => {
                        known_list.insert(local.0, elems.clone());
                    }
                    Value::AllocAdt { fields, .. } => {
                        known_adt.insert(local.0, fields.clone());
                    }
                    Value::Unary {
                        op: UnOp::Neg,
                        operand,
                    } => {
                        if let Some(&n) = known_int.get(&operand.0) {
                            if let Some(r) = n.checked_neg() {
                                *value = Value::Int(r);
                                known_int.insert(local.0, r);
                            }
                            // Overflow (i64::MIN): leave Neg for runtime trap.
                        }
                    }
                    Value::Unary {
                        op: UnOp::Not,
                        operand,
                    } => {
                        if let Some(&n) = known_int.get(&operand.0) {
                            let r = n == 0;
                            *value = Value::Bool(r);
                            known_int.insert(local.0, if r { 1 } else { 0 });
                        }
                    }
                    Value::Binary { op, left, right } => {
                        if let (Some(&a), Some(&b)) =
                            (known_int.get(&left.0), known_int.get(&right.0))
                        {
                            if let Some(r) = fold_bin(*op, a, b) {
                                // Keep Bool for cmp/logic so println / ABI typing stay correct.
                                *value = if matches!(
                                    op,
                                    BinOp::Eq
                                        | BinOp::Ne
                                        | BinOp::Lt
                                        | BinOp::Le
                                        | BinOp::Gt
                                        | BinOp::Ge
                                        | BinOp::And
                                        | BinOp::Or
                                ) {
                                    Value::Bool(r != 0)
                                } else {
                                    Value::Int(r)
                                };
                                known_int.insert(local.0, r);
                            }
                        }
                    }
                    Value::Builtin { name, args } => match (*name, args.as_slice()) {
                        (Builtin::ListLen, [xs]) => {
                            if let Some(elems) = known_list.get(&xs.0) {
                                let n = elems.len() as i64;
                                *value = Value::Int(n);
                                known_int.insert(local.0, n);
                            }
                        }
                        (Builtin::ListGet, [xs, idx]) => {
                            if let (Some(elems), Some(&i)) =
                                (known_list.get(&xs.0), known_int.get(&idx.0))
                            {
                                if i >= 0 && (i as usize) < elems.len() {
                                    let el = elems[i as usize];
                                    *value = Value::Local(el);
                                    if let Some(&n) = known_int.get(&el.0) {
                                        known_int.insert(local.0, n);
                                    }
                                    if let Some(inner) = known_list.get(&el.0).cloned() {
                                        known_list.insert(local.0, inner);
                                    }
                                }
                            }
                        }
                        (Builtin::AdtField, [adt, idx, ..]) => {
                            if let (Some(fields), Some(&i)) =
                                (known_adt.get(&adt.0), known_int.get(&idx.0))
                            {
                                if i >= 0 && (i as usize) < fields.len() {
                                    let el = fields[i as usize];
                                    *value = Value::Local(el);
                                    if let Some(&n) = known_int.get(&el.0) {
                                        known_int.insert(local.0, n);
                                    }
                                    if let Some(inner) = known_list.get(&el.0).cloned() {
                                        known_list.insert(local.0, inner);
                                    }
                                    if let Some(inner) = known_adt.get(&el.0).cloned() {
                                        known_adt.insert(local.0, inner);
                                    }
                                }
                            }
                        }
                        _ => {}
                    },
                    Value::If {
                        then_block,
                        else_block,
                        ..
                    } => {
                        const_fold_block(then_block);
                        const_fold_block(else_block);
                    }
                    Value::Loop {
                        header,
                        body,
                        latch,
                    } => {
                        const_fold_block(header);
                        const_fold_block(body);
                        const_fold_block(latch);
                    }
                    _ => {}
                }
            }
            Op::Let { value, .. } => {
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    const_fold_block(then_block);
                    const_fold_block(else_block);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    const_fold_block(header);
                    const_fold_block(body);
                    const_fold_block(latch);
                }
            }
            _ => {}
        }
    }
}

fn fold_bin(op: BinOp, a: i64, b: i64) -> Option<i64> {
    Some(match op {
        BinOp::Add => a.checked_add(b)?,
        BinOp::Sub => a.checked_sub(b)?,
        BinOp::Mul => a.checked_mul(b)?,
        BinOp::Div if b != 0 && !(a == i64::MIN && b == -1) => a / b,
        BinOp::Rem if b != 0 && !(a == i64::MIN && b == -1) => a % b,
        BinOp::Eq => (a == b) as i64,
        BinOp::Ne => (a != b) as i64,
        BinOp::Lt => (a < b) as i64,
        BinOp::Le => (a <= b) as i64,
        BinOp::Gt => (a > b) as i64,
        BinOp::Ge => (a >= b) as i64,
        BinOp::And => ((a != 0) && (b != 0)) as i64,
        BinOp::Or => ((a != 0) || (b != 0)) as i64,
        _ => return None,
    })
}

fn copy_prop_block(block: &mut Block) {
    let mut rewrite: HashMap<u32, u32> = HashMap::new();
    for op in &mut block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                rewrite_value(value, &rewrite);
                if let Value::Local(Local(src)) = value {
                    let root = rewrite.get(src).copied().unwrap_or(*src);
                    rewrite.insert(local.0, root);
                    *value = Value::Local(Local(root));
                }
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    copy_prop_block(then_block);
                    copy_prop_block(else_block);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    copy_prop_block(header);
                    copy_prop_block(body);
                    copy_prop_block(latch);
                }
            }
            Op::Let { value, .. } => {
                rewrite_value(value, &rewrite);
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    copy_prop_block(then_block);
                    copy_prop_block(else_block);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    copy_prop_block(header);
                    copy_prop_block(body);
                    copy_prop_block(latch);
                }
            }
            Op::Effect { value } => rewrite_value(value, &rewrite),
            Op::Assign { value, .. } => {
                if let Some(&r) = rewrite.get(&value.0) {
                    *value = Local(r);
                }
            }
            Op::Break | Op::Continue => {}
        }
    }
    if let Some(r) = block.result {
        if let Some(&nr) = rewrite.get(&r.0) {
            block.result = Some(Local(nr));
        }
    }
}

// ─── L1: LICM (loop-invariant pure lets → shared cells outside the loop) ───

fn licm_block(block: &mut Block) {
    let mut out = Vec::with_capacity(block.ops.len());
    for mut op in std::mem::take(&mut block.ops) {
        match &mut op {
            Op::Let {
                value:
                    Value::If {
                        then_block,
                        else_block,
                        ..
                    },
                ..
            } => {
                licm_block(then_block);
                licm_block(else_block);
                out.push(op);
            }
            Op::Let {
                value:
                    Value::Loop {
                        header,
                        body,
                        latch,
                    },
                ..
            } => {
                licm_block(header);
                licm_block(body);
                licm_block(latch);

                let mut loop_defs = HashSet::new();
                collect_defs(header, &mut loop_defs);
                collect_defs(body, &mut loop_defs);
                collect_defs(latch, &mut loop_defs);

                // Iterate: hoist pure lets whose operands are all outside the loop.
                loop {
                    let mut changed = false;
                    let mut kept = Vec::new();
                    for hop in std::mem::take(&mut body.ops) {
                        if let Op::Let {
                            local,
                            value,
                            pure_region: true,
                        } = &hop
                        {
                            if is_hoistable(value, &loop_defs) {
                                loop_defs.remove(&local.0);
                                out.push(hop);
                                changed = true;
                                continue;
                            }
                        }
                        kept.push(hop);
                    }
                    body.ops = kept;
                    if !changed {
                        break;
                    }
                }
                out.push(op);
            }
            _ => out.push(op),
        }
    }
    block.ops = out;
}

fn collect_defs(block: &Block, defs: &mut HashSet<u32>) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                defs.insert(local.0);
                match value {
                    Value::If {
                        then_block,
                        else_block,
                        ..
                    } => {
                        collect_defs(then_block, defs);
                        collect_defs(else_block, defs);
                    }
                    Value::Loop {
                        header,
                        body,
                        latch,
                    } => {
                        collect_defs(header, defs);
                        collect_defs(body, defs);
                        collect_defs(latch, defs);
                    }
                    _ => {}
                }
            }
            Op::Assign { .. } | Op::Effect { .. } | Op::Break | Op::Continue => {}
        }
    }
}

fn is_hoistable(value: &Value, loop_defs: &HashSet<u32>) -> bool {
    match value {
        // Don't hoist control / alloc / names (may observe mutation).
        Value::If { .. }
        | Value::Loop { .. }
        | Value::Lambda { .. }
        | Value::Name(_)
        | Value::AllocList { .. }
        | Value::AllocSet { .. }
        | Value::AllocMap { .. }
        | Value::AllocAdt { .. }
        | Value::AllocClosure { .. }
        | Value::IndirectCall { .. } => false,
        // Checked Int arithmetic / Neg may trap — must not hoist past break (§2.4).
        Value::Binary {
            op: BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem,
            ..
        } => false,
        Value::Unary {
            op: UnOp::Neg, ..
        } => false,
        Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::FunRef(_) => true,
        Value::Local(l) => !loop_defs.contains(&l.0),
        Value::Unary { operand, .. } => !loop_defs.contains(&operand.0),
        Value::Binary { left, right, .. } => {
            !loop_defs.contains(&left.0) && !loop_defs.contains(&right.0)
        }
        // User calls / builtins may trap or allocate; only hoist trivial locals above.
        Value::Call { .. } => false,
        Value::Builtin { name, args } => {
            if builtin_may_trap_or_effect(name) {
                return false;
            }
            args.iter().all(|a| !loop_defs.contains(&a.0))
        }
        Value::ClosureCap { env, .. } => !loop_defs.contains(&env.0),
    }
}

fn builtin_may_trap_or_effect(b: &Builtin) -> bool {
    matches!(
        b,
        Builtin::ListGet
            | Builtin::MapRemove
            | Builtin::Println
            | Builtin::PrintlnInt
            | Builtin::PrintlnStr
            | Builtin::ReadStdin
            | Builtin::MatchFail
            | Builtin::Assert
            | Builtin::ListParMap
            | Builtin::ListParFold
            | Builtin::Range
            | Builtin::RangeInclusive
            | Builtin::AdtField
            | Builtin::AdtTag
    )
}

// ─── T_f eligibility ───────────────────────────────────────────────────────

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
    let mut is_param = HashSet::new();
    is_param.insert(param);
    let mut st = StructRec {
        fun: &f.name,
        is_param,
        smaller: HashSet::new(),
        known_int: HashMap::new(),
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
                    let left_ok =
                        st.is_param.contains(&left.0) || st.smaller.contains(&left.0);
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
    let mut freq: HashMap<Vec<i64>, usize> = HashMap::new();
    for f in &module.functions {
        collect_const_calls(&f.body, fun, &mut HashMap::new(), &mut freq);
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
            Op::Let { local, value, .. } => {
                match value {
                    Value::Int(n) => {
                        known.insert(local.0, *n);
                    }
                    Value::Call {
                        fun: callee,
                        args,
                    } if callee == fun => {
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
                }
            }
            Op::Effect { value } => {
                if let Value::Call {
                    fun: callee,
                    args,
                } = value
                {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pass;
    use lumia_syntax::BinOp;
    use lumia_ty::{Effect, Type};

    fn bare_fun(name: &str, params: Vec<Local>, body: Block) -> CoreFun {
        let n = params.len();
        CoreFun {
            name: name.into(),
            params,
            param_names: (0..n).map(|i| format!("p{i}")).collect(),
            param_tys: vec![Type::Int; n],
            body,
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
        escaping: std::collections::HashSet::new(),
        }
    }

    #[test]
    fn cse_dedups_int_and_nontrapping_binary() {
        // Add/Sub/Mul/Div/Rem are not CSE'd (may trap). Eq is pure and may share.
        let mut module = CoreModule {
            name: "C".into(),
            functions: vec![bare_fun(
                "main",
                vec![],
                Block {
                    params: vec![],
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Int(1),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::Int(1),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(2),
                            value: Value::Binary {
                                op: BinOp::Eq,
                                left: Local(0),
                                right: Local(1),
                            },
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(3),
                            value: Value::Binary {
                                op: BinOp::Eq,
                                left: Local(0),
                                right: Local(1),
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(3)),
                },
            )],
            hash_adts: std::collections::HashSet::new(),
        };
        module.functions[0].is_main = true;
        module.functions[0].effect = Effect::io();
        cse_module(&mut module);
        let ops = &module.functions[0].body.ops;
        assert!(matches!(
            &ops[1],
            Op::Let {
                value: Value::Local(Local(0)),
                ..
            }
        ));
        assert!(matches!(
            &ops[3],
            Op::Let {
                value: Value::Local(_),
                ..
            }
        ));
    }

    #[test]
    fn cse_preserves_distinct_external_calls() {
        let mut getpid = bare_fun(
            "getpid",
            vec![],
            Block {
                params: vec![],
                ops: vec![],
                result: None,
            },
        );
        getpid.external = Some("getpid".into());
        getpid.effect = Effect::pure();
        let mut module = CoreModule {
            name: "C".into(),
            functions: vec![
                getpid,
                bare_fun(
                    "main",
                    vec![],
                    Block {
                        params: vec![],
                        ops: vec![
                            Op::Let {
                                local: Local(0),
                                value: Value::Call {
                                    fun: "getpid".into(),
                                    args: vec![],
                                },
                                pure_region: true,
                            },
                            Op::Let {
                                local: Local(1),
                                value: Value::Call {
                                    fun: "getpid".into(),
                                    args: vec![],
                                },
                                pure_region: true,
                            },
                        ],
                        result: Some(Local(1)),
                    },
                ),
            ],
            hash_adts: std::collections::HashSet::new(),
        };
        module.functions[1].is_main = true;
        module.functions[1].effect = Effect::io();
        cse_module(&mut module);
        let ops = &module.functions[1].body.ops;
        assert!(
            matches!(
                &ops[0],
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } if fun == "getpid"
            ),
            "first foreign call must remain"
        );
        assert!(
            matches!(
                &ops[1],
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } if fun == "getpid"
            ),
            "second foreign call must not be CSE'd into the first"
        );
    }

    #[test]
    fn memo_l0_folds_list_len_get() {
        use lumia_core::ListRepr;
        let mut module = CoreModule {
            name: "C".into(),
            functions: vec![bare_fun(
                "f",
                vec![],
                Block {
                    params: vec![],
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Int(10),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::Int(20),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(2),
                            value: Value::AllocList {
                                elems: vec![Local(0), Local(1)],
                                repr: ListRepr::LitList,
                            },
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(3),
                            value: Value::Builtin {
                                name: Builtin::ListLen,
                                args: vec![Local(2)],
                            },
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(4),
                            value: Value::Int(1),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(5),
                            value: Value::Builtin {
                                name: Builtin::ListGet,
                                args: vec![Local(2), Local(4)],
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(5)),
                },
            )],
            hash_adts: std::collections::HashSet::new(),
        };
        MemoL0Pass.run(&mut module);
        assert!(matches!(
            &module.functions[0].body.ops[3],
            Op::Let {
                value: Value::Int(2),
                ..
            }
        ));
        assert!(matches!(
            &module.functions[0].body.ops[5],
            Op::Let {
                value: Value::Local(Local(1)),
                ..
            }
        ));
    }

    #[test]
    fn memo_l0_const_folds() {
        let mut module = CoreModule {
            name: "C".into(),
            functions: vec![bare_fun(
                "f",
                vec![],
                Block {
                    params: vec![],
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Int(2),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::Int(3),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(2),
                            value: Value::Binary {
                                op: BinOp::Mul,
                                left: Local(0),
                                right: Local(1),
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(2)),
                },
            )],
            hash_adts: std::collections::HashSet::new(),
        };
        MemoL0Pass.run(&mut module);
        assert!(matches!(
            &module.functions[0].body.ops[2],
            Op::Let {
                value: Value::Int(6),
                ..
            }
        ));
    }

    #[test]
    fn memo_l0_folds_cmp_to_bool() {
        let mut module = CoreModule {
            name: "C".into(),
            functions: vec![bare_fun(
                "f",
                vec![],
                Block {
                    params: vec![],
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Int(1),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::Int(2),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(2),
                            value: Value::Binary {
                                op: BinOp::Lt,
                                left: Local(0),
                                right: Local(1),
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(2)),
                },
            )],
            hash_adts: std::collections::HashSet::new(),
        };
        MemoL0Pass.run(&mut module);
        assert!(matches!(
            &module.functions[0].body.ops[2],
            Op::Let {
                value: Value::Bool(true),
                ..
            }
        ));
    }

    #[test]
    fn memo_l1_licm_hoists_not_but_not_trapping_add() {
        // Checked Add may trap — must stay in-loop (§2.4). Pure Bool `not` is safe to hoist.
        let mut module = CoreModule {
            name: "L".into(),
            functions: vec![bare_fun(
                "f",
                vec![Local(0)],
                Block {
                    params: vec![],
                    ops: vec![Op::Let {
                        local: Local(10),
                        value: Value::Loop {
                            header: Box::new(Block {
                                params: vec![],
                                ops: vec![Op::Let {
                                    local: Local(2),
                                    value: Value::Bool(true),
                                    pure_region: true,
                                }],
                                result: Some(Local(2)),
                            }),
                            body: Box::new(Block {
                                params: vec![],
                                ops: vec![
                                    Op::Let {
                                        local: Local(3),
                                        value: Value::Binary {
                                            op: BinOp::Add,
                                            left: Local(0),
                                            right: Local(0),
                                        },
                                        pure_region: true,
                                    },
                                    Op::Let {
                                        local: Local(4),
                                        value: Value::Unary {
                                            op: UnOp::Not,
                                            operand: Local(0),
                                        },
                                        pure_region: true,
                                    },
                                ],
                                result: Some(Local(4)),
                            }),
                            latch: Box::new(Block {
                                params: vec![],
                                ops: vec![],
                                result: None,
                            }),
                        },
                        pure_region: true,
                    }],
                    result: Some(Local(10)),
                },
            )],
            hash_adts: std::collections::HashSet::new(),
        };
        MemoL1Pass.run(&mut module);
        let ops = &module.functions[0].body.ops;
        assert!(
            matches!(
                &ops[0],
                Op::Let {
                    value: Value::Unary {
                        op: UnOp::Not,
                        ..
                    },
                    ..
                }
            ),
            "invariant `not` should hoist before loop, got {:?}",
            ops[0]
        );
        let body_ops = match &ops[1] {
            Op::Let {
                value: Value::Loop { body, .. },
                ..
            } => &body.ops,
            other => panic!("expected loop as second op, got {other:?}"),
        };
        assert!(
            body_ops.iter().any(|op| matches!(
                op,
                Op::Let {
                    value: Value::Binary {
                        op: BinOp::Add,
                        ..
                    },
                    ..
                }
            )),
            "trapping Add must remain inside the loop"
        );
    }

    #[test]
    fn memo_tf_marks_dense_int() {
        // fib-like: f(n) = f(n-1) with enough body weight.
        let mut fib = bare_fun(
            "fib",
            vec![Local(0)],
            Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Binary {
                            op: BinOp::Sub,
                            left: Local(0),
                            right: Local(1),
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Call {
                            fun: "fib".into(),
                            args: vec![Local(2)],
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(4),
                        value: Value::Binary {
                            op: BinOp::Add,
                            left: Local(3),
                            right: Local(1),
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(4)),
            },
        );
        fib.param_names = vec!["n".into()];
        let module = CoreModule {
            name: "M".into(),
            functions: vec![fib],
            hash_adts: std::collections::HashSet::new(),
        };
        let plan = plan_memo_tf(&module);
        assert!(
            matches!(plan.get("fib"), Some(MemoTf::DenseInt { .. })),
            "expected DenseInt, got {:?}",
            plan.get("fib")
        );
    }

    #[test]
    fn memo_tf_marks_slots() {
        // Pure multi-arg with static same-arg reuse from caller → Slots.
        let mut sq = bare_fun(
            "sq",
            vec![Local(0)],
            Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(1),
                        value: Value::Binary {
                            op: BinOp::Mul,
                            left: Local(0),
                            right: Local(0),
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Binary {
                            op: BinOp::Add,
                            left: Local(1),
                            right: Local(0),
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Binary {
                            op: BinOp::Mul,
                            left: Local(2),
                            right: Local(2),
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(3)),
            },
        );
        sq.param_names = vec!["n".into()];
        let main = CoreFun {
            name: "main".into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            body: Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Int(99),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Call {
                            fun: "sq".into(),
                            args: vec![Local(0)],
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Call {
                            fun: "sq".into(),
                            args: vec![Local(0)],
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(2)),
            },
            ret_ty: Type::Int,
            effect: Effect::io(),
            is_main: true,
            memo: None,
            external: None,
        escaping: std::collections::HashSet::new(),
        };
        let module = CoreModule {
            name: "M".into(),
            functions: vec![sq, main],
            hash_adts: std::collections::HashSet::new(),
        };
        let plan = plan_memo_tf(&module);
        assert!(
            matches!(plan.get("sq"), Some(MemoTf::Slots { .. })),
            "expected Slots, got {:?}",
            plan.get("sq")
        );
    }

    #[test]
    fn memo_tf_increasing_recursion_not_dense() {
        // f(n) = f(n+1) must not get DenseInt.
        let mut f = bare_fun(
            "inc",
            vec![Local(0)],
            Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Binary {
                            op: BinOp::Add,
                            left: Local(0),
                            right: Local(1),
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Call {
                            fun: "inc".into(),
                            args: vec![Local(2)],
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(3)),
            },
        );
        f.param_names = vec!["n".into()];
        let module = CoreModule {
            name: "M".into(),
            functions: vec![f],
            hash_adts: std::collections::HashSet::new(),
        };
        let plan = plan_memo_tf(&module);
        assert!(
            !matches!(plan.get("inc"), Some(MemoTf::DenseInt { .. })),
            "increasing self-recursion must not use dense index T_f, got {:?}",
            plan.get("inc")
        );
    }
}
