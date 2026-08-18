//! Runtime extern declarations for `lumia_rt` — table-driven to avoid declare drift.

use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::types::{BasicMetadataTypeEnum, BasicTypeEnum, FunctionType};
use inkwell::AddressSpace;

#[derive(Clone, Copy)]
enum RtTy {
    Void,
    I8,
    I32,
    I64,
    F64,
    Ptr,
}

#[derive(Clone, Copy)]
struct RtDecl {
    name: &'static str,
    ret: RtTy,
    args: &'static [RtTy],
}

/// Non-builtin / ABI extras plus every direct `lumia_*` the runtime ships.
/// Keep in sync with `lumia_rt` exports; builtin symbols are also checked by test.
const RUNTIME_DECLS: &[RtDecl] = &[
    RtDecl {
        name: "lumia_println_int",
        ret: RtTy::Void,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_println_auto",
        ret: RtTy::Void,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_println_float",
        ret: RtTy::Void,
        args: &[RtTy::F64],
    },
    RtDecl {
        name: "lumia_eq",
        ret: RtTy::I64,
        args: &[RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_adt_eq",
        ret: RtTy::I64,
        args: &[RtTy::I64, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: lumia_abi::ADT_SET_FLOAT_MASK,
        ret: RtTy::Void,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: lumia_abi::ADT_SET_BOOL_MASK,
        ret: RtTy::Void,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_adt_ensure_unique",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_adt_ensure_unique_mask",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_adt_ensure_unique_consume",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_adt_ensure_unique_consume_mask",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_adt_set_field",
        ret: RtTy::Void,
        args: &[RtTy::Ptr, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_show_adt",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_show_adt_named",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I64, RtTy::I64, RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_show_list_bool",
        ret: RtTy::Ptr,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_show_set_bool",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I32],
    },
    RtDecl {
        name: "lumia_show_map_bool",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I32, RtTy::I32],
    },
    RtDecl {
        name: "lumia_show_list_adt",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_adt_register_show",
        ret: RtTy::Void,
        args: &[RtTy::I32, RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_cmp",
        ret: RtTy::I64,
        args: &[RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_println_str",
        ret: RtTy::Void,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_println_cstr",
        ret: RtTy::Void,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_println_bool",
        ret: RtTy::Void,
        args: &[RtTy::I8],
    },
    RtDecl {
        name: "lumia_println_unit",
        ret: RtTy::Void,
        args: &[],
    },
    RtDecl {
        name: "lumia_alloc",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I32],
    },
    RtDecl {
        name: "lumia_alloc_string",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_string_cstr",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_cstr_to_string",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_alloc_char",
        ret: RtTy::Ptr,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_show",
        ret: RtTy::Ptr,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_show_float",
        ret: RtTy::Ptr,
        args: &[RtTy::F64],
    },
    RtDecl {
        name: "lumia_show_bool",
        ret: RtTy::Ptr,
        args: &[RtTy::I8],
    },
    RtDecl {
        name: "lumia_gc_collect",
        ret: RtTy::Void,
        args: &[],
    },
    RtDecl {
        name: "lumia_list_retain",
        ret: RtTy::Void,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_list_release",
        ret: RtTy::Void,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_adt_retain",
        ret: RtTy::Void,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_adt_release",
        ret: RtTy::Void,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_abi_handoff_set",
        ret: RtTy::Void,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_root_push",
        ret: RtTy::Void,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_frame_push",
        ret: RtTy::Void,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_frame_pop",
        ret: RtTy::Void,
        args: &[],
    },
    RtDecl {
        name: "lumia_root_pop",
        ret: RtTy::Void,
        args: &[],
    },
    RtDecl {
        name: "lumia_dict_register",
        ret: RtTy::Void,
        args: &[RtTy::I32, RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_dict_lookup",
        ret: RtTy::Ptr,
        args: &[RtTy::I32, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_dict_show",
        ret: RtTy::Ptr,
        args: &[RtTy::I32, RtTy::Ptr, RtTy::I64],
    },
    // `lumia_write_barrier` — remembered set for minor GC (old→young stores).
    RtDecl {
        name: "lumia_write_barrier",
        ret: RtTy::Void,
        args: &[RtTy::Ptr, RtTy::I32, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_list_promote",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_list_len",
        ret: RtTy::I64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_list_get",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_list_slice",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_list_append",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_list_concat",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_list_empty",
        ret: RtTy::Ptr,
        args: &[],
    },
    RtDecl {
        name: "lumia_map_empty",
        ret: RtTy::Ptr,
        args: &[],
    },
    RtDecl {
        name: "lumia_set_empty",
        ret: RtTy::Ptr,
        args: &[],
    },
    RtDecl {
        name: "lumia_map_finish",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_set_finish",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_len",
        ret: RtTy::I64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_concat",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_get",
        ret: RtTy::I64,
        args: &[
            RtTy::Ptr,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
        ],
    },
    RtDecl {
        name: "lumia_contains",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: lumia_abi::ENSURE_MAP_F64,
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: lumia_abi::ENSURE_MAP_VF64,
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: lumia_abi::ENSURE_SET_F64,
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: lumia_abi::ENSURE_LIST_F64,
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: lumia_abi::ENSURE_MAP_BOOL,
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: lumia_abi::ENSURE_MAP_VBOOL,
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: lumia_abi::ENSURE_SET_BOOL,
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: lumia_abi::ENSURE_LIST_BOOL,
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_map_set",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_map_get",
        ret: RtTy::Ptr,
        args: &[
            RtTy::Ptr,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
        ],
    },
    RtDecl {
        name: "lumia_map_contains",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_list_set",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_set",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_map_remove",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_set_insert",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_set_contains",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_set_remove",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_remove",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_map_keys",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_elems",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_map_values",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_map_items",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_adt_tag",
        ret: RtTy::I64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_adt_field",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_range",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_range_inclusive",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_str_trim",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_str_len",
        ret: RtTy::I64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_str_byte_len",
        ret: RtTy::I64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_str_take",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_str_slice",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_str_reverse",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_str_to_lower",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_str_to_upper",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_str_split",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_str_substring",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_list_take",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_list_reverse",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_list_sort",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_list_sort_by_keys",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_list_par_map",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::Ptr, RtTy::I32],
    },
    RtDecl {
        name: "lumia_list_par_fold",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::I64, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_list_join",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_read_stdin",
        ret: RtTy::Ptr,
        args: &[],
    },
    RtDecl {
        name: "lumia_match_fail",
        ret: RtTy::Void,
        args: &[],
    },
    RtDecl {
        name: "lumia_assert",
        ret: RtTy::Void,
        args: &[RtTy::I64, RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_trap_div0",
        ret: RtTy::Void,
        args: &[],
    },
    RtDecl {
        name: "lumia_trap_overflow",
        ret: RtTy::Void,
        args: &[],
    },
    RtDecl {
        name: "lumia_collatz_total",
        ret: RtTy::I64,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_mandelbrot_checksum",
        ret: RtTy::I64,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_collatz_strided",
        ret: RtTy::I64,
        args: &[RtTy::I64, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_count_primes",
        ret: RtTy::I64,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_affine2_rem_sum",
        ret: RtTy::I64,
        args: &[RtTy::I64, RtTy::I64, RtTy::I64, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_gcd_sum",
        ret: RtTy::I64,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_divisor_sum",
        ret: RtTy::I64,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_product_rem_sum",
        ret: RtTy::I64,
        args: &[RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_affine1_rem_sum",
        ret: RtTy::I64,
        args: &[RtTy::I64, RtTy::I64, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_matmul_affine_checksum",
        ret: RtTy::I64,
        args: &[RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_mem_traffic_checksum",
        ret: RtTy::I64,
        args: &[RtTy::I64, RtTy::I64, RtTy::I64],
    },
    // Dense List[Float] kernels (flat row-major; unique buffers stay in-place).
    RtDecl {
        name: "lumia_list_f64_zeros",
        ret: RtTy::Ptr,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_f64_fill",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::F64],
    },
    RtDecl {
        name: "lumia_f64_scale",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::F64],
    },
    RtDecl {
        name: "lumia_f64_sqrt",
        ret: RtTy::F64,
        args: &[RtTy::F64],
    },
    RtDecl {
        name: "lumia_f64_exp",
        ret: RtTy::F64,
        args: &[RtTy::F64],
    },
    RtDecl {
        name: "lumia_f64_sin",
        ret: RtTy::F64,
        args: &[RtTy::F64],
    },
    RtDecl {
        name: "lumia_f64_cos",
        ret: RtTy::F64,
        args: &[RtTy::F64],
    },
    RtDecl {
        name: "lumia_f64_atan2",
        ret: RtTy::F64,
        args: &[RtTy::F64, RtTy::F64],
    },
    RtDecl {
        name: "lumia_f64_hypot",
        ret: RtTy::F64,
        args: &[RtTy::F64, RtTy::F64],
    },
    RtDecl {
        name: "lumia_f64_mul",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_add",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_l2_norm",
        ret: RtTy::F64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_sum_sq",
        ret: RtTy::F64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_mean",
        ret: RtTy::F64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_std",
        ret: RtTy::F64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_softmax",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_l2_normalize",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::F64],
    },
    RtDecl {
        name: "lumia_f64_clamp",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::F64, RtTy::F64],
    },
    RtDecl {
        name: "lumia_f64_gemv",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I64, RtTy::Ptr, RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_gemv_t",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I64, RtTy::Ptr, RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_addmm",
        ret: RtTy::Ptr,
        args: &[
            RtTy::I64,
            RtTy::I64,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::F64,
        ],
    },
    RtDecl {
        name: "lumia_f64_axpy",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::F64, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_sub",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_copy",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_f64_checksum",
        ret: RtTy::I64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_efe_action_scores",
        ret: RtTy::Ptr,
        args: &[
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
        ],
    },
    RtDecl {
        name: "lumia_efe_embodied_action_scores",
        ret: RtTy::Ptr,
        args: &[
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
        ],
    },
    RtDecl {
        name: "lumia_efe_apply_embodied_reflexes",
        ret: RtTy::Ptr,
        args: &[
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::I64,
            RtTy::I64,
            RtTy::F64,
        ],
    },
    RtDecl {
        name: "lumia_cn_nucleus_step",
        ret: RtTy::Ptr,
        args: &[
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::I64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
        ],
    },
    RtDecl {
        name: "lumia_cn_hebbian",
        ret: RtTy::Ptr,
        args: &[
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::I64,
            RtTy::I64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
        ],
    },
    RtDecl {
        name: "lumia_cn_project_clamp",
        ret: RtTy::Ptr,
        args: &[
            RtTy::I64,
            RtTy::I64,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::F64,
        ],
    },
    RtDecl {
        name: "lumia_cn_backproj_clamp",
        ret: RtTy::Ptr,
        args: &[
            RtTy::I64,
            RtTy::I64,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::F64,
        ],
    },
    RtDecl {
        name: "lumia_cn_axpy_clamp",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::F64, RtTy::Ptr, RtTy::F64],
    },
    RtDecl {
        name: "lumia_cn_argmax",
        ret: RtTy::I64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_cn_cluster_rates",
        ret: RtTy::Ptr,
        args: &[
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::I64,
            RtTy::I64,
            RtTy::F64,
            RtTy::F64,
        ],
    },
    RtDecl {
        name: "lumia_cn_learn_generative",
        ret: RtTy::Ptr,
        args: &[
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::I64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
        ],
    },
    RtDecl {
        name: "lumia_cn_update_state",
        ret: RtTy::Ptr,
        args: &[
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::Ptr,
            RtTy::I64,
            RtTy::F64,
            RtTy::F64,
            RtTy::F64,
        ],
    },
    RtDecl {
        name: "lumia_ptr_eq",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_str_starts_with",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_str_ends_with",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_str_contains",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_str_concat",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    // Frozen C ABI names (`T_f` planner; DESIGN vocabulary is not "L2").
    RtDecl {
        name: "lumia_memo_l2_lookup",
        ret: RtTy::I64,
        args: &[
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::Ptr,
        ],
    },
    RtDecl {
        name: "lumia_memo_l2_hits",
        ret: RtTy::I64,
        args: &[],
    },
    RtDecl {
        name: "lumia_memo_l2_misses",
        ret: RtTy::I64,
        args: &[],
    },
    RtDecl {
        name: "lumia_memo_l2_reset",
        ret: RtTy::Void,
        args: &[],
    },
    RtDecl {
        name: "lumia_memo_l2_store",
        ret: RtTy::Void,
        args: &[
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
            RtTy::I64,
        ],
    },
    RtDecl {
        name: "lumia_memo_idx_lookup",
        ret: RtTy::I64,
        args: &[RtTy::I64, RtTy::I64, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_memo_idx_store",
        ret: RtTy::Void,
        args: &[RtTy::I64, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_memo_idx_hits",
        ret: RtTy::I64,
        args: &[],
    },
    RtDecl {
        name: "lumia_memo_idx_misses",
        ret: RtTy::I64,
        args: &[],
    },
    RtDecl {
        name: "lumia_memo_idx_reset",
        ret: RtTy::Void,
        args: &[],
    },
    // Task / Channel concurrency (lumia_rt::task).
    RtDecl {
        name: "lumia_channel_new",
        ret: RtTy::Ptr,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_channel_send",
        ret: RtTy::Void,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_channel_recv",
        ret: RtTy::I64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_channel_recv_opt",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_channel_close",
        ret: RtTy::Void,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_task_spawn",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_task_spawn_nullary",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_task_join",
        ret: RtTy::I64,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_task_join_opt",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_scope_enter",
        ret: RtTy::Void,
        args: &[RtTy::I64],
    },
    RtDecl {
        name: "lumia_scope_leave",
        ret: RtTy::Void,
        args: &[],
    },
    RtDecl {
        name: "lumia_scope_cancel",
        ret: RtTy::Void,
        args: &[],
    },
    RtDecl {
        name: "lumia_scheduler_drain",
        ret: RtTy::Void,
        args: &[],
    },
    RtDecl {
        name: "lumia_scheduler_kind",
        ret: RtTy::I64,
        args: &[RtTy::I64],
    },
];

fn basic_ty<'ctx>(context: &'ctx Context, ty: RtTy) -> BasicTypeEnum<'ctx> {
    match ty {
        RtTy::Void => unreachable!("void is not a basic type"),
        RtTy::I8 => context.i8_type().into(),
        RtTy::I32 => context.i32_type().into(),
        RtTy::I64 => context.i64_type().into(),
        RtTy::F64 => context.f64_type().into(),
        RtTy::Ptr => context.ptr_type(AddressSpace::default()).into(),
    }
}

fn fn_type<'ctx>(context: &'ctx Context, decl: &RtDecl) -> FunctionType<'ctx> {
    let args: Vec<BasicMetadataTypeEnum<'ctx>> = decl
        .args
        .iter()
        .map(|&a| basic_ty(context, a).into())
        .collect();
    match decl.ret {
        RtTy::Void => context.void_type().fn_type(&args, false),
        RtTy::I8 => context.i8_type().fn_type(&args, false),
        RtTy::I32 => context.i32_type().fn_type(&args, false),
        RtTy::I64 => context.i64_type().fn_type(&args, false),
        RtTy::F64 => context.f64_type().fn_type(&args, false),
        RtTy::Ptr => context
            .ptr_type(AddressSpace::default())
            .fn_type(&args, false),
    }
}

pub(crate) fn declare_runtime<'ctx>(context: &'ctx Context, module: &LlvmModule<'ctx>) {
    for decl in RUNTIME_DECLS {
        // Idempotent: duplicates in the table would hit LLVM's add_function assert.
        if module.get_function(decl.name).is_some() {
            continue;
        }
        let fv = module.add_function(decl.name, fn_type(context, decl), None);
        crate::attrs::add_nounwind(context, fv);
    }
}

/// Declared `lumia_*` names (tests: specialized Show table ↔ decls).
#[cfg(test)]
pub(crate) fn runtime_decl_names_for_test() -> rustc_hash::FxHashSet<&'static str> {
    RUNTIME_DECLS.iter().map(|d| d.name).collect()
}

#[cfg(test)]
mod tests {
    use super::RUNTIME_DECLS;
    use inkwell::context::Context;
    use lumia_hir::Builtin;
    use rustc_hash::FxHashSet as HashSet;

    #[test]
    fn runtime_decl_names_are_unique() {
        let mut seen = HashSet::default();
        for d in RUNTIME_DECLS {
            assert!(
                seen.insert(d.name),
                "duplicate runtime decl name {}",
                d.name
            );
        }
    }

    #[test]
    fn every_dense_f64_trampoline_is_declared() {
        let names: HashSet<&str> = RUNTIME_DECLS.iter().map(|d| d.name).collect();
        let mut missing = Vec::new();
        for sym in lumia_abi::DENSE_F64_TRAMPOLINE_SYMS {
            if !names.contains(sym) {
                missing.push(*sym);
            }
        }
        assert!(
            missing.is_empty(),
            "DENSE_F64_TRAMPOLINE_SYMS missing from declare_runtime:\n  {}",
            missing.join("\n  ")
        );
        assert!(
            names.contains(lumia_abi::ADT_SET_FLOAT_MASK),
            "ADT_SET_FLOAT_MASK must be in RUNTIME_DECLS"
        );
        assert!(
            names.contains(lumia_abi::ADT_SET_BOOL_MASK),
            "ADT_SET_BOOL_MASK must be in RUNTIME_DECLS"
        );
    }

    #[test]
    fn every_builtin_runtime_symbol_is_declared() {
        let context = Context::create();
        let module = context.create_module("rt_decl_test");
        super::declare_runtime(&context, &module);
        let mut missing = Vec::new();
        for &b in Builtin::ALL {
            let Some(sym) = b.runtime_symbol() else {
                continue;
            };
            if module.get_function(sym).is_none() {
                missing.push(format!("{} → {sym}", b.display_name()));
            }
        }
        assert!(
            missing.is_empty(),
            "BuiltinInfo.runtime_symbol missing from declare_runtime:\n  {}",
            missing.join("\n  ")
        );
    }

    /// Diff `lumia_rt` `#[no_mangle] extern "C"` exports against `RUNTIME_DECLS`.
    /// Test-only / internal symbols can be listed in `RT_EXPORT_ALLOWLIST`.
    #[test]
    fn runtime_decls_cover_rt_no_mangle_exports() {
        let decl_names: HashSet<&str> = RUNTIME_DECLS.iter().map(|d| d.name).collect();
        let rt_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../lumia_rt");
        let mut rt_syms = HashSet::default();
        collect_rt_c_exports(&rt_root, &mut rt_syms);

        // Empty for now: every `lumia_*` C export should be declare-able.
        const RT_EXPORT_ALLOWLIST: &[&str] = &[];

        let mut missing = Vec::new();
        for sym in &rt_syms {
            if decl_names.contains(sym.as_str()) || RT_EXPORT_ALLOWLIST.contains(&sym.as_str()) {
                continue;
            }
            missing.push(sym.clone());
        }
        missing.sort();
        assert!(
            missing.is_empty(),
            "lumia_rt #[no_mangle] exports missing from RUNTIME_DECLS:\n  {}\n\
             (add decls or RT_EXPORT_ALLOWLIST for test-only symbols)",
            missing.join("\n  ")
        );

        let mut orphan = Vec::new();
        for name in &decl_names {
            if !rt_syms.contains(*name) {
                orphan.push(*name);
            }
        }
        orphan.sort_unstable();
        assert!(
            orphan.is_empty(),
            "RUNTIME_DECLS names with no lumia_rt #[no_mangle] export:\n  {}",
            orphan.join("\n  ")
        );
    }

    fn collect_rt_c_exports(dir: &std::path::Path, out: &mut HashSet<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for ent in entries.flatten() {
            let path = ent.path();
            if path.is_dir() {
                collect_rt_c_exports(&path, out);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let bytes = text.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i..].starts_with(b"#[no_mangle]") {
                    let rest = &bytes[i + b"#[no_mangle]".len()..];
                    if let Some(rel) = find_ascii(rest, b"extern \"C\" fn ") {
                        let after = &rest[rel + b"extern \"C\" fn ".len()..];
                        let name = take_ident(after);
                        if !name.is_empty() {
                            out.insert(name);
                        }
                    }
                    i += b"#[no_mangle]".len();
                    continue;
                }
                if bytes[i..].starts_with(b"#[export_name = \"") {
                    let after = &bytes[i + b"#[export_name = \"".len()..];
                    if let Some(end) = after.iter().position(|&b| b == b'"') {
                        if let Ok(name) = std::str::from_utf8(&after[..end]) {
                            out.insert(name.to_string());
                        }
                        i += b"#[export_name = \"".len() + end + 1;
                        continue;
                    }
                }
                i += 1;
            }
        }
    }

    fn find_ascii(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    fn take_ident(bytes: &[u8]) -> String {
        let n = bytes
            .iter()
            .take_while(|&&b| b.is_ascii_alphanumeric() || b == b'_')
            .count();
        String::from_utf8_lossy(&bytes[..n]).into_owned()
    }
}
