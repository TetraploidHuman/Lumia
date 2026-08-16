//! Dense `List[Float]` trampoline symbol table.

/// Dense `List[Float]` kernels that opt may rewrite whole helpers into and that
/// codegen may emit as frameless trampolines.
///
/// Must stay ⊆ `runtime_decls` / `lumia_rt` exports. Scalar helpers (`sqrt` /
/// `exp` / …) and `checksum` are declared separately and are **not** trampoline
/// eligible.
pub const DENSE_F64_TRAMPOLINE_SYMS: &[&str] = &[
    "lumia_f64_gemv",
    "lumia_f64_gemv_t",
    "lumia_f64_addmm",
    "lumia_f64_axpy",
    "lumia_f64_sub",
    "lumia_f64_add",
    "lumia_f64_mul",
    "lumia_f64_clamp",
    "lumia_f64_scale",
    "lumia_f64_fill",
    "lumia_f64_copy",
    "lumia_list_f64_zeros",
    "lumia_f64_sum_sq",
    "lumia_f64_mean",
    "lumia_f64_std",
    "lumia_f64_l2_norm",
    "lumia_f64_softmax",
    "lumia_f64_l2_normalize",
];

#[inline]
pub fn is_dense_f64_trampoline(sym: &str) -> bool {
    DENSE_F64_TRAMPOLINE_SYMS.iter().any(|&s| s == sym)
}
