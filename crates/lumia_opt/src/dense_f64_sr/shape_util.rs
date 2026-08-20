use lumia_core::{CoreFun, Local, Value};
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

pub(super) use lumia_core::{
    block_calls_any as body_calls_any, first_direct_loop as first_loop, for_each_shape_value,
    is_out_list, is_out_set, out_slot_for_list_param, OutSlot,
};

pub(super) fn mentions_local(v: &Value, target: Local) -> bool {
    match v {
        Value::Local(l) => *l == target,
        Value::Binary { left, right, .. } => *left == target || *right == target,
        Value::Builtin { args, .. } => args.contains(&target),
        _ => false,
    }
}
