//! Shared SR / dense-kernel pattern match helpers (opt + codegen).

use crate::{Block, Local, Op, Value};
use lumi_syntax::BinOp;
use rustc_hash::FxHashMap as HashMap;

/// First `Assign` that stores `src` into a mutable slot.
pub fn first_assign_from_local(body: &Block, src: Local) -> Option<String> {
    for op in &body.ops {
        if let Op::Assign { name, value } = op {
            if *value == src {
                return Some(name.clone());
            }
        }
    }
    None
}

/// First top-level `Loop` in `body` (header, body, latch).
pub fn first_loop(body: &Block) -> Option<(&Block, &Block, &Block)> {
    for op in &body.ops {
        if let Op::Let {
            value:
                Value::Loop {
                    header,
                    body,
                    latch,
                },
            ..
        } = op
        {
            return Some((header, body, latch));
        }
    }
    None
}

/// `Name` load for a leaf-def local.
pub fn name_of(l: Local, defs: &HashMap<u32, Value>) -> Option<String> {
    match defs.get(&l.0)? {
        Value::Name(n) => Some(n.clone()),
        _ => None,
    }
}

/// Loop header of the form `iv < bound` → `(iv_slot, bound_local)`.
pub fn header_lt_bound(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, Local)> {
    let res = header.result?;
    let Value::Binary {
        op: BinOp::Lt,
        left,
        right,
        ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    let iv = name_of(*left, defs)?;
    Some((iv, *right))
}

/// Resolve `Local` identity through leaf defs (not through `Name` loads).
pub fn same_local(got: Local, want: Local, defs: &HashMap<u32, Value>) -> bool {
    if got == want {
        return true;
    }
    match defs.get(&got.0) {
        Some(Value::Local(l)) => same_local(*l, want, defs),
        Some(Value::Name(_)) => false,
        _ => false,
    }
}

/// Latch / body defines `iv = iv + 1` (either operand order).
pub fn is_unit_inc(dest: u32, iv: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&dest)
    else {
        return false;
    };
    let one_l = matches!(defs.get(&left.0), Some(Value::Int(1)));
    let one_r = matches!(defs.get(&right.0), Some(Value::Int(1)));
    let name_l = name_of(*left, defs).as_deref() == Some(iv);
    let name_r = name_of(*right, defs).as_deref() == Some(iv);
    (name_l && one_r) || (name_r && one_l)
}

/// Known `Int` constant for a leaf-def local.
pub fn const_int(l: Local, defs: &HashMap<u32, Value>) -> Option<i64> {
    match defs.get(&l.0)? {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}
