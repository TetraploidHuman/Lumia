use super::blas_shape::{
    body_has_gemv_inner, fun_has_add_shape, fun_has_addmm_shape, fun_has_axpy_shape,
    fun_has_gemv_t_shape, fun_has_mul_shape, fun_has_sub_shape,
};
use super::shape_util::{
    first_assign_from_local, first_loop, param_float, param_int, param_list_f64, ret_list_f64,
};
use lumia_core::{header_lt_bound, same_local, Value};
use rustc_hash::FxHashMap as HashMap;

pub(super) fn match_gemv_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 5
        || !param_int(fun, 0)
        || !param_int(fun, 1)
        || !param_list_f64(fun, 2)
        || !param_list_f64(fun, 3)
        || !param_list_f64(fun, 4)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (m, n, a, x, y) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    let (header, loop_body, latch) = first_loop(body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let (i_slot, bound) = header_lt_bound(header, defs)?;
    if !same_local(bound, m, defs) {
        return None;
    }
    if !body_has_gemv_inner(loop_body, defs, &out_slot, &i_slot, a, x, n) {
        return None;
    }
    Some(())
}

pub(super) fn match_gemv_t_fun(
    fun: &lumia_core::CoreFun,
    defs: &HashMap<u32, Value>,
) -> Option<()> {
    if fun.params.len() != 5
        || !param_int(fun, 0)
        || !param_int(fun, 1)
        || !param_list_f64(fun, 2)
        || !param_list_f64(fun, 3)
        || !param_list_f64(fun, 4)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (m, n, a, x, y) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    if !fun_has_gemv_t_shape(body, defs, &out_slot, a, x, m, n) {
        return None;
    }
    Some(())
}

pub(super) fn match_addmm_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 6
        || !param_int(fun, 0)
        || !param_int(fun, 1)
        || !param_list_f64(fun, 2)
        || !param_list_f64(fun, 3)
        || !param_list_f64(fun, 4)
        || !param_float(fun, 5)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (m, n, w, u, v, alpha) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
        fun.params[5],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, w)?;
    if !fun_has_addmm_shape(body, defs, &out_slot, u, v, alpha, m, n) {
        return None;
    }
    Some(())
}

pub(super) fn match_axpy_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 3
        || !param_list_f64(fun, 0)
        || !param_float(fun, 1)
        || !param_list_f64(fun, 2)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (y, alpha, x) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    if !fun_has_axpy_shape(body, defs, &out_slot, x, alpha) {
        return None;
    }
    Some(())
}

pub(super) fn match_sub_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 3
        || !param_list_f64(fun, 0)
        || !param_list_f64(fun, 1)
        || !param_list_f64(fun, 2)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (out, a, b) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, out)?;
    if !fun_has_sub_shape(body, defs, &out_slot, a, b) {
        return None;
    }
    Some(())
}

pub(super) fn match_add_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 3
        || !param_list_f64(fun, 0)
        || !param_list_f64(fun, 1)
        || !param_list_f64(fun, 2)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (out, a, b) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, out)?;
    if !fun_has_add_shape(body, defs, &out_slot, a, b) {
        return None;
    }
    Some(())
}

pub(super) fn match_mul_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 3
        || !param_list_f64(fun, 0)
        || !param_list_f64(fun, 1)
        || !param_list_f64(fun, 2)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (out, a, b) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, out)?;
    if !fun_has_mul_shape(body, defs, &out_slot, a, b) {
        return None;
    }
    Some(())
}
