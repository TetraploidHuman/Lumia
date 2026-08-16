use lumia_core::{Block, CoreModule, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_core::{CoreBinOp as BinOp, CoreUnOp as UnOp};
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ExprKey {
    Int(i64),
    Bool(bool),
    Float(u64),
    Char(char),
    String(String),
    Unary(UnOp, u32),
    Binary(BinOp, u32, u32),
    Builtin(Builtin, Vec<u32>),
    Call(String, Vec<u32>),
}

fn ret_is_cse_safe(ty: &Type) -> bool {
    // Heap identity matters: CSE of `zeros(16)` must not alias two mutable lists.
    matches!(
        ty,
        Type::Int | Type::Bool | Type::Float | Type::Char | Type::Unit
    )
}

pub fn cse_module(module: &mut CoreModule) {
    // Foreign (`external`) must never be CSE'd: even trusted
    // `foreign "C" pure` is an honor-system claim; libc calls like `getpid` /
    // `getenv` are not referentially transparent. Inline already skips `external`.
    // Also skip pure wrappers that return heap objects (`zeros` → shared buffer).
    let pure_funs: HashSet<String> = module
        .functions
        .iter()
        .filter(|f| f.effect.is_pure() && f.external.is_none() && ret_is_cse_safe(&f.ret_ty))
        .map(|f| f.name.clone())
        .collect();
    let float_rets: HashSet<String> = module
        .functions
        .iter()
        .filter(|f| matches!(f.ret_ty, Type::Float))
        .map(|f| f.name.clone())
        .collect();
    for f in &mut module.functions {
        let mut float_locals = HashSet::default();
        for (i, ty) in f.param_tys.iter().enumerate() {
            if matches!(ty, Type::Float) {
                if let Some(p) = f.params.get(i) {
                    float_locals.insert(p.0);
                }
            }
        }
        cse_block(&mut f.body, &pure_funs, &float_rets, &mut float_locals);
    }
}

fn cse_block(
    block: &mut Block,
    pure_funs: &HashSet<String>,
    float_rets: &HashSet<String>,
    float_locals: &mut HashSet<u32>,
) {
    let mut seen: HashMap<ExprKey, u32> = HashMap::default();
    let mut rewrite: HashMap<u32, u32> = HashMap::default();

    for op in &mut block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                rewrite_value(value, &rewrite);
                note_float_local(local.0, value, float_locals, float_rets);
                if let Some(key) = expr_key(value, pure_funs, float_locals) {
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
                    cse_block(then_block, pure_funs, float_rets, float_locals);
                    cse_block(else_block, pure_funs, float_rets, float_locals);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    cse_block(header, pure_funs, float_rets, float_locals);
                    cse_block(body, pure_funs, float_rets, float_locals);
                    cse_block(latch, pure_funs, float_rets, float_locals);
                }
            }
            Op::Let { local, value, .. } => {
                rewrite_value(value, &rewrite);
                note_float_local(local.0, value, float_locals, float_rets);
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    cse_block(then_block, pure_funs, float_rets, float_locals);
                    cse_block(else_block, pure_funs, float_rets, float_locals);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    cse_block(header, pure_funs, float_rets, float_locals);
                    cse_block(body, pure_funs, float_rets, float_locals);
                    cse_block(latch, pure_funs, float_rets, float_locals);
                }
            }
            Op::Assign { value, .. } | Op::Return { value } => {
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

fn note_float_local(
    local: u32,
    value: &Value,
    float_locals: &mut HashSet<u32>,
    float_rets: &HashSet<String>,
) {
    let is_float = match value {
        Value::Float(_) => true,
        Value::Local(Local(src)) => float_locals.contains(src),
        Value::Unary {
            op: UnOp::Neg,
            operand,
        } => float_locals.contains(&operand.0),
        Value::Binary {
            op: BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem,
            left,
            right,
        } => float_locals.contains(&left.0) && float_locals.contains(&right.0),
        Value::Call { fun, .. } => float_rets.contains(fun),
        _ => false,
    };
    if is_float {
        float_locals.insert(local);
    }
}

fn expr_key(
    value: &Value,
    pure_funs: &HashSet<String>,
    float_locals: &HashSet<u32>,
) -> Option<ExprKey> {
    match value {
        Value::Int(n) => Some(ExprKey::Int(*n)),
        Value::Bool(b) => Some(ExprKey::Bool(*b)),
        Value::Float(f) => Some(ExprKey::Float(f.to_bits())),
        Value::Char(c) => Some(ExprKey::Char(*c)),
        Value::String(s) => Some(ExprKey::String(s.clone())),
        // Int Neg may trap (i64::MIN); Float Neg is fine to share.
        Value::Unary {
            op: UnOp::Neg,
            operand,
        } => {
            if float_locals.contains(&operand.0) {
                Some(ExprKey::Unary(UnOp::Neg, operand.0))
            } else {
                None
            }
        }
        Value::Unary { op, operand } => Some(ExprKey::Unary(*op, operand.0)),
        // Int arith may trap (§2.4); IEEE Float does not.
        Value::Binary {
            op: op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem),
            left,
            right,
        } => {
            if float_locals.contains(&left.0) && float_locals.contains(&right.0) {
                Some(ExprKey::Binary(*op, left.0, right.0))
            } else {
                None
            }
        }
        Value::Binary { op, left, right } => Some(ExprKey::Binary(*op, left.0, right.0)),
        Value::Builtin { name, args, .. } if builtin_is_pure(name) => Some(ExprKey::Builtin(
            *name,
            args.iter().map(|a| a.0).collect(),
        )),
        Value::Call { fun, args } if pure_funs.contains(fun) => Some(ExprKey::Call(
            fun.clone(),
            args.iter().map(|a| a.0).collect(),
        )),
        _ => None,
    }
}

fn builtin_is_pure(b: &Builtin) -> bool {
    // Align with LICM: do not CSE traps / effects / parallel map (same key in
    // divergent control flow must not erase a failing path).
    if super::licm::builtin_may_trap_or_effect(b) {
        return false;
    }
    // Heap-returning builtins must not CSE: shared identity breaks under COW
    // (same class of bug as CSE of pure heap-returning calls).
    !matches!(b.result_heap(), lumia_hir::ResultHeap::Always)
}

pub(crate) fn rewrite_value(v: &mut Value, rewrite: &HashMap<u32, u32>) {
    // Shallow: const-fold / CSE rewrite operands on this node only (not nested blocks).
    lumia_core::for_each_local_mut(v, &mut |l| {
        if let Some(&r) = rewrite.get(&l.0) {
            *l = Local(r);
        }
    });
}
