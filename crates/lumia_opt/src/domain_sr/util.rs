//! Shared peeps for whole-function domain SR matchers.

use lumia_core::{
    const_of, first_direct_loop, header_le_bound, header_le_const, header_lt_bound,
    header_lt_const, is_unit_inc, name_of, same_local, Block, Local, Op, Value,
};
use rustc_hash::FxHashMap as HashMap;

pub(super) fn slot_init_const(
    body: &Block,
    slot: &str,
    expect: i64,
    defs: &HashMap<u32, Value>,
) -> bool {
    for op in &body.ops {
        if matches!(
            op,
            Op::Let {
                value: Value::Loop { .. },
                ..
            }
        ) {
            break;
        }
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == slot && const_of(Local(*v), defs) == Some(expect) {
                return true;
            }
        }
    }
    false
}

pub(super) fn result_is_slot(body: &Block, slot: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(r) = body.result else {
        return false;
    };
    name_of(r, defs).as_deref() == Some(slot)
}

pub(super) fn body_unit_incs_slot(body: &Block, slot: &str, defs: &HashMap<u32, Value>) -> bool {
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == slot && is_unit_inc(*v, slot, defs) {
                return true;
            }
        }
    }
    false
}

/// Outer `Name(iv) < bound` where bound is `param` or a const `≥ min_n`.
pub(super) fn outer_lt_param_or_const(
    header: &Block,
    defs: &HashMap<u32, Value>,
    param: Local,
    param_name: Option<&String>,
    min_n: i64,
) -> Option<(String, Option<i64>)> {
    if let Some((iv, c)) = header_lt_const(header, defs) {
        if c >= min_n {
            return Some((iv, Some(c)));
        }
    }
    let (iv, bound) = header_lt_bound(header, defs)?;
    if same_local(bound, param, defs) {
        return Some((iv, None));
    }
    if let Some(Value::Name(nm)) = defs.get(&bound.0) {
        if param_name == Some(nm) {
            return Some((iv, None));
        }
    }
    None
}

/// Outer `Name(iv) <= bound` (param or const `≥ min_n`).
pub(super) fn outer_le_param_or_const(
    header: &Block,
    defs: &HashMap<u32, Value>,
    param: Local,
    param_name: Option<&String>,
    min_n: i64,
) -> Option<(String, Option<i64>)> {
    if let Some((iv, c)) = header_le_const(header, defs) {
        if c >= min_n {
            return Some((iv, Some(c)));
        }
    }
    let (iv, bound) = header_le_bound(header, defs)?;
    if same_local(bound, param, defs) {
        return Some((iv, None));
    }
    if let Some(Value::Name(nm)) = defs.get(&bound.0) {
        if param_name == Some(nm) {
            return Some((iv, None));
        }
    }
    None
}

pub(super) fn first_loop(body: &Block) -> Option<(&Block, &Block, &Block)> {
    first_direct_loop(body)
}
