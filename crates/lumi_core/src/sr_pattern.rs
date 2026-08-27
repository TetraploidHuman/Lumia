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

/// Loop header `iv < const` → `(iv_slot, const)`; optionally accept `const > iv`.
pub fn header_lt_const(
    header: &Block,
    defs: &HashMap<u32, Value>,
    allow_gt_swap: bool,
) -> Option<(String, i64)> {
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    match op {
        BinOp::Lt => {
            let iv = name_of(*left, defs)?;
            let k = const_int(*right, defs)?;
            Some((iv, k))
        }
        BinOp::Gt if allow_gt_swap => {
            let iv = name_of(*right, defs)?;
            let k = const_int(*left, defs)?;
            Some((iv, k))
        }
        _ => None,
    }
}

/// Loop header `iv <= const` → `(iv_slot, const)`; optionally accept `const >= iv`.
pub fn header_le_const(
    header: &Block,
    defs: &HashMap<u32, Value>,
    allow_ge_swap: bool,
) -> Option<(String, i64)> {
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    match op {
        BinOp::Le => {
            let iv = name_of(*left, defs)?;
            let k = const_int(*right, defs)?;
            Some((iv, k))
        }
        BinOp::Ge if allow_ge_swap => {
            let iv = name_of(*right, defs)?;
            let k = const_int(*left, defs)?;
            Some((iv, k))
        }
        _ => None,
    }
}

/// `Assign slot = expect` in loop body (via leaf-def local).
pub fn body_assigns_const(
    body: &Block,
    slot: &str,
    expect: i64,
    defs: &HashMap<u32, Value>,
) -> bool {
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == slot && const_int(Local(*v), defs) == Some(expect) {
                return true;
            }
        }
    }
    false
}

/// `dest = acc + term` → `term` when one side is `Name(acc)`.
pub fn split_acc_add(dest: u32, acc_name: &str, defs: &HashMap<u32, Value>) -> Option<Local> {
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = defs.get(&dest)?
    else {
        return None;
    };
    if name_of(*left, defs).as_deref() == Some(acc_name) {
        Some(*right)
    } else if name_of(*right, defs).as_deref() == Some(acc_name) {
        Some(*left)
    } else {
        None
    }
}

/// `row*n + col + 1` (either association).
pub fn is_affine_row_col_plus1(
    l: Local,
    row: &str,
    col: &str,
    n: i64,
    defs: &HashMap<u32, Value>,
) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&l.0)
    else {
        return false;
    };
    let rest = if const_int(*left, defs) == Some(1) {
        *right
    } else if const_int(*right, defs) == Some(1) {
        *left
    } else {
        return false;
    };
    let Some(Value::Binary {
        op: BinOp::Add,
        left: a,
        right: b,
        ..
    }) = defs.get(&rest.0)
    else {
        return false;
    };
    matches!(
        (
            is_name_mul_const(*a, row, n, defs),
            name_of(*b, defs).as_deref() == Some(col),
            is_name_mul_const(*b, row, n, defs),
            name_of(*a, defs).as_deref() == Some(col),
        ),
        (true, true, _, _) | (_, _, true, true)
    )
}

pub fn is_name_mul_const(l: Local, name: &str, n: i64, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Mul,
        left,
        right,
        ..
    }) = defs.get(&l.0)
    else {
        return false;
    };
    (name_of(*left, defs).as_deref() == Some(name) && const_int(*right, defs) == Some(n))
        || (name_of(*right, defs).as_deref() == Some(name) && const_int(*left, defs) == Some(n))
}

/// `dest = Name(name) + const` → increment.
pub fn acc_add_const_inc(dest: u32, name: &str, defs: &HashMap<u32, Value>) -> Option<i64> {
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = defs.get(&dest)?
    else {
        return None;
    };
    if name_of(*left, defs).as_deref() == Some(name) {
        const_int(*right, defs)
    } else if name_of(*right, defs).as_deref() == Some(name) {
        const_int(*left, defs)
    } else {
        None
    }
}

/// `dest = acc + _`-shaped update mentioning `acc`.
pub fn acc_add_has_name(dest: u32, acc_name: &str, defs: &HashMap<u32, Value>) -> bool {
    split_acc_add(dest, acc_name, defs).is_some()
}

/// `dest = Name(a) + Name(b)` (either order).
pub fn is_add_name_plus_name(dest: u32, a: &str, b: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&dest)
    else {
        return false;
    };
    let ln = name_of(*left, defs);
    let rn = name_of(*right, defs);
    (ln.as_deref() == Some(a) && rn.as_deref() == Some(b))
        || (ln.as_deref() == Some(b) && rn.as_deref() == Some(a))
}

/// Header result is `Name(x) > 1` (or `1 < Name(x)`).
pub fn header_gt1_iv(header: &Block, defs: &HashMap<u32, Value>) -> Option<String> {
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    match op {
        BinOp::Gt => {
            let x = name_of(*left, defs)?;
            if const_int(*right, defs) == Some(1) {
                Some(x)
            } else {
                None
            }
        }
        BinOp::Lt => {
            let x = name_of(*right, defs)?;
            if const_int(*left, defs) == Some(1) {
                Some(x)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Parse loop header as `(iv, const)` — `header_lt_const` or `header_le_const`.
pub type HeaderConstFn = fn(&Block, &HashMap<u32, Value>, bool) -> Option<(String, i64)>;

/// Outer `iv <(=) n { inner_iv reset; Loop { inner_iv <(=) n; … } }`.
pub struct NestedLoop<'a> {
    pub outer_iv: String,
    pub n: i64,
    pub inner_iv: String,
    pub inner_header: &'a Block,
    pub inner_body: &'a Block,
    pub inner_latch: &'a Block,
}

pub fn match_nested_loop<'a>(
    header: &Block,
    body: &'a Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
    parse_bound: HeaderConstFn,
    allow_swap: bool,
    inner_reset: i64,
) -> Option<NestedLoop<'a>> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (outer_iv, n) = parse_bound(header, defs, allow_swap)?;
    if n < 2 {
        return None;
    }
    let (ih, ib, il) = first_loop(body)?;
    if !il.ops.is_empty() {
        return None;
    }
    let (inner_iv, n2) = parse_bound(ih, defs, allow_swap)?;
    if n2 != n || inner_iv == outer_iv {
        return None;
    }
    if !body_assigns_const(body, &inner_iv, inner_reset, defs) {
        return None;
    }
    Some(NestedLoop {
        outer_iv,
        n,
        inner_iv,
        inner_header: ih,
        inner_body: ib,
        inner_latch: il,
    })
}

pub fn body_iv_unit_inc(body: &Block, iv: &str, defs: &HashMap<u32, Value>) -> bool {
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == iv && is_unit_inc(*v, iv, defs) {
                return true;
            }
        }
    }
    false
}

/// Header result is `Name(d) * Name(d) <= Name(n)`.
pub fn header_dd_le_n(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, String)> {
    let res = header.result?;
    let Value::Binary {
        op: BinOp::Le,
        left,
        right,
        ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    let n = name_of(*right, defs)?;
    let Value::Binary {
        op: BinOp::Mul,
        left: a,
        right: b,
        ..
    } = defs.get(&left.0)?
    else {
        return None;
    };
    let da = name_of(*a, defs)?;
    let db = name_of(*b, defs)?;
    if da != db {
        return None;
    }
    Some((da, n))
}
