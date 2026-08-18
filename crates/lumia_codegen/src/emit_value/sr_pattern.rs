//! Thin re-export of Core SR peeps for codegen `*_sr` modules.

pub(crate) use lumia_core::{
    body_assigns_const, body_assigns_name_div_const, body_assigns_name_mul_const_plus_const,
    body_assigns_unit_inc, const_of, has_float_approx, has_float_binop_with_const, header_gt_eq,
    header_lt_const, header_name_sq_le_name, is_name_rem_eq_const, is_unit_inc,
    local_is_zero_or_false, rem_eq_zero_names,
};
