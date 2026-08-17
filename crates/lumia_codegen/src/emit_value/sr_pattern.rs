//! Thin re-export of Core SR peeps for codegen `*_sr` modules.

pub(crate) use lumia_core::{
    acc_add_rem_const_mod, add_name_other, body_assigns_const, body_assigns_name_div_const,
    body_assigns_name_mul_const_plus_const, body_assigns_rem, body_assigns_unit_inc,
    body_assigns_zero_or_false, const_of, first_direct_loop, has_float_approx,
    has_float_binop_with_const, header_gt_eq, header_le_const, header_lt_const,
    header_name_sq_le_name, is_add_name_plus_any, is_add_name_plus_name, is_affine_ik1,
    is_affine_kj1, is_name_add_const, is_name_mul_const, is_name_ne_zero, is_name_rem_eq_const,
    is_unit_inc, local_is_zero_or_false, name_ne_zero, name_of, rem_eq_zero_names,
};
