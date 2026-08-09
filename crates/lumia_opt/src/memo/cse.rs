use lumia_core::{Block, CoreModule, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::{BinOp, UnOp};
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

fn expr_key(value: &Value, pure_funs: &HashSet<String>) -> Option<ExprKey> {
    match value {
        Value::Int(n) => Some(ExprKey::Int(*n)),
        Value::Bool(b) => Some(ExprKey::Bool(*b)),
        Value::Float(f) => Some(ExprKey::Float(f.to_bits())),
        Value::Char(c) => Some(ExprKey::Char(*c)),
        Value::String(s) => Some(ExprKey::String(s.clone())),
        // Trapping arithmetic must not CSE across divergent paths (§2.4).
        Value::Unary { op: UnOp::Neg, .. } => None,
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
    !super::licm::builtin_may_trap_or_effect(b)
}

pub(crate) fn rewrite_value(v: &mut Value, rewrite: &HashMap<u32, u32>) {
    // Shallow: const-fold / CSE rewrite operands on this node only (not nested blocks).
    lumia_core::for_each_local_mut(v, &mut |l| {
        if let Some(&r) = rewrite.get(&l.0) {
            *l = Local(r);
        }
    });
}
