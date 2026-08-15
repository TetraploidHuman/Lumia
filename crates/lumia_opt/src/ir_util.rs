//! Shared IR helpers for opt passes.

use lumia_core::{for_each_block_dfs, Block, Local, Op, Value};
use lumia_syntax::{BinOp, UnOp};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Known Int/Bool/Char/Float locals as i64 bit patterns (for call-site matching).
#[derive(Clone, Default)]
pub(crate) struct KnownScalars {
    map: HashMap<u32, i64>,
}

impl KnownScalars {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert(&mut self, local: u32, n: i64) {
        self.map.insert(local, n);
    }

    pub(crate) fn remove(&mut self, local: u32) {
        self.map.remove(&local);
    }

    pub(crate) fn get(&self, local: u32) -> Option<i64> {
        self.map.get(&local).copied()
    }

    pub(crate) fn contains(&self, local: u32) -> bool {
        self.map.contains_key(&local)
    }

    /// Track Int / Bool / Char / Float (and Local aliases) as i64 bit patterns.
    pub(crate) fn track(&mut self, local: u32, value: &Value) {
        match value {
            Value::Int(n) => {
                self.map.insert(local, *n);
            }
            Value::Bool(b) => {
                self.map.insert(local, if *b { 1 } else { 0 });
            }
            Value::Char(c) => {
                self.map.insert(local, u32::from(*c) as i64);
            }
            Value::Float(f) => {
                self.map.insert(local, f.to_bits() as i64);
            }
            Value::Local(Local(src)) => {
                if let Some(&n) = self.map.get(src) {
                    self.map.insert(local, n);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn resolve_all(&self, args: &[Local]) -> Option<Vec<i64>> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            out.push(self.get(a.0)?);
        }
        Some(out)
    }
}

/// Fixed-point: locals that hold unboxed Float (for DCE / LICM keep/hoist rules).
pub(crate) fn collect_float_locals(block: &Block, float_locals: &mut HashSet<u32>) {
    loop {
        let before = float_locals.len();
        for_each_block_dfs(block, &mut |b| {
            for op in &b.ops {
                if let Op::Let { local, value, .. } = op {
                    if value_produces_float(value, float_locals) {
                        float_locals.insert(local.0);
                    }
                }
            }
        });
        if float_locals.len() == before {
            break;
        }
    }
}

fn value_produces_float(value: &Value, float_locals: &HashSet<u32>) -> bool {
    match value {
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
        _ => false,
    }
}
