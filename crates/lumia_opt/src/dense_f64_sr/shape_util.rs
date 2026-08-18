use lumia_core::{for_each_let_value_ctrl, Block, CoreFun, Local, Op, Value};
use lumia_ty::Type;

fn is_list_f64(t: &Type) -> bool {
    matches!(t, Type::List(e) if matches!(e.as_ref(), Type::Float))
}

pub(super) fn param_list_f64(fun: &CoreFun, i: usize) -> bool {
    fun.param_tys.get(i).is_some_and(is_list_f64)
}

pub(super) fn param_float(fun: &CoreFun, i: usize) -> bool {
    matches!(fun.param_tys.get(i), Some(Type::Float))
}

pub(super) fn param_int(fun: &CoreFun, i: usize) -> bool {
    matches!(fun.param_tys.get(i), Some(Type::Int))
}

pub(super) fn ret_list_f64(fun: &CoreFun) -> bool {
    is_list_f64(&fun.ret_ty)
}

pub(super) fn out_slot_for_list_param(body: &Block, src: Local) -> Option<String> {
    if let Some(s) = first_assign_from_local(body, src) {
        return Some(s);
    }
    // `val out = xs` SSA alias: matchers also accept `src` via `same_local`.
    for op in &body.ops {
        if let Op::Let {
            value: Value::Local(l),
            ..
        } = op
        {
            if *l == src {
                return Some(String::new());
            }
        }
    }
    None
}

pub(super) fn first_assign_from_local(body: &Block, src: Local) -> Option<String> {
    for op in &body.ops {
        if let Op::Assign { name, value } = op {
            if *value == src {
                return Some(name.clone());
            }
        }
    }
    None
}

pub(super) fn first_loop(body: &Block) -> Option<(&Block, &Block, &Block)> {
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

pub(super) fn body_calls_any(body: &Block, names: &[&str]) -> bool {
    let mut found = false;
    for_each_let_value_ctrl(body, &mut |_b, val| {
        if let Value::Call { fun, .. } = val {
            if names.iter().any(|n| fun == n) {
                found = true;
            }
        }
    });
    found
}

pub(super) fn mentions_local(v: &Value, target: Local) -> bool {
    match v {
        Value::Local(l) => *l == target,
        Value::Binary { left, right, .. } => *left == target || *right == target,
        Value::Builtin { args, .. } => args.contains(&target),
        _ => false,
    }
}
