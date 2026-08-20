//! Runtime extern declarations for `lumia_rt` — table-driven to avoid declare drift.
//!
//! Declarations are organized by subsystem. Each sub-table is a `&[RtDecl]`
//! constant; [`RUNTIME_DECLS`] concatenates them at init. To add a new RT
//! symbol, find or create the relevant sub-table and append there.
//!
//! CI validates this table bidirectionally against `lumia_rt` `#[no_mangle]`
//! exports: see [`tests::runtime_decls_cover_rt_no_mangle_exports`].

use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::types::{BasicMetadataTypeEnum, BasicTypeEnum, FunctionType};
use inkwell::AddressSpace;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum RtTy {
    Void,
    I8,
    I32,
    I64,
    F64,
    Ptr,
}

/// Human-readable subsystem tag — used only for diagnostics and the
/// `every_subsystem_has_decls` test; has no runtime effect.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RtSubsystem {
    Io,
    Adt,
    Gc,
    List,
    MapSet,
    String,
    Compare,
    Trap,
    Dict,
    FloatAbi,
    DenseFloat,
    DomainSr,
    Cn,
    Memo,
    Task,
    Abi,
}

#[derive(Clone, Copy, Debug)]
struct RtDecl {
    name: &'static str,
    ret: RtTy,
    args: &'static [RtTy],
    #[cfg(test)]
    subsystem: RtSubsystem,
}

// ---------------------------------------------------------------------------
// Compact declaration macro
// ---------------------------------------------------------------------------

/// `rt!(SUBSYSTEM, "name", RET, [ARG, …])`
macro_rules! rt {
    ($sub:ident, $name:expr, $ret:ident, [$($a:ident),*]) => {
        RtDecl {
            name: $name,
            ret: RtTy::$ret,
            args: &[$(RtTy::$a),*],
            #[cfg(test)]
            subsystem: RtSubsystem::$sub,
        }
    };
}

// ---------------------------------------------------------------------------
// Sub-tables by subsystem
// ---------------------------------------------------------------------------

const IO_DECLS: &[RtDecl] = &[
    rt!(Io, "lumia_println_int",   Void, [I64]),
    rt!(Io, "lumia_println_auto",  Void, [I64]),
    rt!(Io, "lumia_println_float", Void, [F64]),
    rt!(Io, "lumia_println_str",   Void, [Ptr, I64]),
    rt!(Io, "lumia_println_cstr",  Void, [Ptr]),
    rt!(Io, "lumia_println_bool",  Void, [I8]),
    rt!(Io, "lumia_println_unit",  Void, []),
    rt!(Io, "lumia_show",          Ptr, [I64]),
    rt!(Io, "lumia_show_float",    Ptr, [F64]),
    rt!(Io, "lumia_show_bool",     Ptr, [I8]),
    rt!(Io, "lumia_read_stdin",    Ptr, []),
];

const ADT_DECLS: &[RtDecl] = &[
    rt!(Adt, lumia_abi::ADT_SET_FLOAT_MASK,  Void, [Ptr, I64]),
    rt!(Adt, lumia_abi::ADT_SET_BOOL_MASK,   Void, [Ptr, I64]),
    rt!(Adt, "lumia_adt_ensure_unique",              Ptr, [Ptr]),
    rt!(Adt, "lumia_adt_ensure_unique_mask",          Ptr, [Ptr, I64]),
    rt!(Adt, "lumia_adt_ensure_unique_consume",       Ptr, [Ptr]),
    rt!(Adt, "lumia_adt_ensure_unique_consume_mask",  Ptr, [Ptr, I64]),
    rt!(Adt, "lumia_adt_set_field",                   Void, [Ptr, I64, I64]),
    rt!(Adt, "lumia_adt_tag",                         I64, [Ptr]),
    rt!(Adt, "lumia_adt_field",                       I64, [Ptr, I64]),
    rt!(Adt, "lumia_adt_eq",                          I64, [I64, I64, I64]),
    rt!(Adt, "lumia_show_adt",                        Ptr, [I64, I64, I64]),
    rt!(Adt, "lumia_show_adt_named",                  Ptr, [I64, I64, I64, Ptr, I64]),
    rt!(Adt, "lumia_show_list_bool",                  Ptr, [I64]),
    rt!(Adt, "lumia_show_set_bool",                   Ptr, [I64, I32]),
    rt!(Adt, "lumia_show_map_bool",                   Ptr, [I64, I32, I32]),
    rt!(Adt, "lumia_show_list_adt",                   Ptr, [I64, I64, I64]),
    rt!(Adt, "lumia_adt_register_show",               Void, [I32, Ptr, I64]),
    rt!(Adt, "lumia_adt_retain",                      Void, [Ptr]),
    rt!(Adt, "lumia_adt_release",                     Void, [Ptr]),
];

const GC_DECLS: &[RtDecl] = &[
    rt!(Gc, "lumia_alloc",              Ptr, [I64, I32]),
    rt!(Gc, "lumia_gc_collect",         Void, []),
    rt!(Gc, "lumia_root_push",          Void, [Ptr]),
    rt!(Gc, "lumia_root_pop",           Void, []),
    rt!(Gc, "lumia_root_swap_remove",   Void, [I64]),
    rt!(Gc, "lumia_frame_push",         Void, [Ptr]),
    rt!(Gc, "lumia_frame_pop",          Void, []),
    rt!(Gc, "lumia_write_barrier",      Void, [Ptr, I32, Ptr]),
];

const LIST_DECLS: &[RtDecl] = &[
    rt!(List, "lumia_list_empty",     Ptr, []),
    rt!(List, "lumia_list_len",       I64, [Ptr]),
    rt!(List, "lumia_list_get",       I64, [Ptr, I64]),
    rt!(List, "lumia_list_set",       Ptr, [Ptr, I64, I64]),
    rt!(List, "lumia_list_slice",     Ptr, [Ptr, I64]),
    rt!(List, "lumia_list_append",    Ptr, [Ptr, I64]),
    rt!(List, "lumia_list_concat",    Ptr, [Ptr, Ptr]),
    rt!(List, "lumia_list_take",      Ptr, [Ptr, I64]),
    rt!(List, "lumia_list_reverse",   Ptr, [Ptr]),
    rt!(List, "lumia_list_sort",      Ptr, [Ptr]),
    rt!(List, "lumia_list_sort_by_keys", Ptr, [Ptr, Ptr]),
    rt!(List, "lumia_list_par_map",   Ptr, [Ptr, Ptr, I32]),
    rt!(List, "lumia_list_par_fold",  I64, [Ptr, I64, Ptr]),
    rt!(List, "lumia_list_join",      Ptr, [Ptr, Ptr]),
    rt!(List, "lumia_list_promote",   Ptr, [Ptr]),
    rt!(List, "lumia_list_retain",    Void, [Ptr]),
    rt!(List, "lumia_list_release",   Void, [Ptr]),
    rt!(List, "lumia_range",          Ptr, [I64, I64]),
    rt!(List, "lumia_range_inclusive", Ptr, [I64, I64]),
];

const MAP_SET_DECLS: &[RtDecl] = &[
    rt!(MapSet, "lumia_map_empty",    Ptr, []),
    rt!(MapSet, "lumia_set_empty",    Ptr, []),
    rt!(MapSet, "lumia_map_finish",   Ptr, [Ptr]),
    rt!(MapSet, "lumia_set_finish",   Ptr, [Ptr]),
    rt!(MapSet, "lumia_map_set",      Ptr, [Ptr, I64, I64]),
    rt!(MapSet, "lumia_map_get",      Ptr, [Ptr, I64, I64, I64, I64, I64]),
    rt!(MapSet, "lumia_map_contains", I64, [Ptr, I64]),
    rt!(MapSet, "lumia_map_remove",   Ptr, [Ptr, I64]),
    rt!(MapSet, "lumia_map_keys",     Ptr, [Ptr]),
    rt!(MapSet, "lumia_map_values",   Ptr, [Ptr]),
    rt!(MapSet, "lumia_map_items",    Ptr, [Ptr, I64]),
    rt!(MapSet, "lumia_set_insert",   Ptr, [Ptr, I64]),
    rt!(MapSet, "lumia_set_union",    Ptr, [Ptr, Ptr]),
    rt!(MapSet, "lumia_set_intersect",Ptr, [Ptr, Ptr]),
    rt!(MapSet, "lumia_set_diff",     Ptr, [Ptr, Ptr]),
    rt!(MapSet, "lumia_set_contains", I64, [Ptr, I64]),
    rt!(MapSet, "lumia_set_remove",   Ptr, [Ptr, I64]),
    // Generic collection ops dispatched by type tag.
    rt!(MapSet, "lumia_len",          I64, [Ptr]),
    rt!(MapSet, "lumia_get",          I64, [Ptr, I64, I64, I64, I64, I64]),
    rt!(MapSet, "lumia_set",          Ptr, [Ptr, I64, I64]),
    rt!(MapSet, "lumia_contains",     I64, [Ptr, I64]),
    rt!(MapSet, "lumia_remove",       Ptr, [Ptr, I64]),
    rt!(MapSet, "lumia_concat",       Ptr, [Ptr, Ptr]),
    rt!(MapSet, "lumia_elems",        Ptr, [Ptr]),
];

const STRING_DECLS: &[RtDecl] = &[
    rt!(String, "lumia_alloc_string",   Ptr, [Ptr, I64]),
    rt!(String, "lumia_alloc_char",     Ptr, [I64]),
    rt!(String, "lumia_string_cstr",    Ptr, [Ptr]),
    rt!(String, "lumia_cstr_to_string", Ptr, [Ptr]),
    rt!(String, "lumia_str_len",        I64, [Ptr]),
    rt!(String, "lumia_str_byte_len",   I64, [Ptr]),
    rt!(String, "lumia_str_trim",       Ptr, [Ptr]),
    rt!(String, "lumia_str_take",       Ptr, [Ptr, I64]),
    rt!(String, "lumia_str_slice",      Ptr, [Ptr, I64]),
    rt!(String, "lumia_str_reverse",    Ptr, [Ptr]),
    rt!(String, "lumia_str_to_lower",   Ptr, [Ptr]),
    rt!(String, "lumia_str_to_upper",   Ptr, [Ptr]),
    rt!(String, "lumia_str_split",      Ptr, [Ptr, I64]),
    rt!(String, "lumia_str_substring",  Ptr, [Ptr, I64, I64]),
    rt!(String, "lumia_str_starts_with", I64, [Ptr, Ptr]),
    rt!(String, "lumia_str_ends_with",   I64, [Ptr, Ptr]),
    rt!(String, "lumia_str_contains",    I64, [Ptr, Ptr]),
    rt!(String, "lumia_str_concat",      Ptr, [Ptr, Ptr]),
];

const COMPARE_DECLS: &[RtDecl] = &[
    rt!(Compare, "lumia_eq",     I64, [I64, I64]),
    rt!(Compare, "lumia_cmp",    I64, [I64, I64]),
    rt!(Compare, "lumia_ptr_eq", I64, [Ptr, Ptr]),
];

const TRAP_DECLS: &[RtDecl] = &[
    rt!(Trap, "lumia_match_fail",     Void, []),
    rt!(Trap, "lumia_assert",         Void, [I64, Ptr, I64]),
    rt!(Trap, "lumia_trap_div0",      Void, []),
    rt!(Trap, "lumia_trap_overflow",  Void, []),
];

const DICT_DECLS: &[RtDecl] = &[
    rt!(Dict, "lumia_dict_register", Void, [I32, Ptr, Ptr]),
    rt!(Dict, "lumia_dict_lookup",   Ptr, [I32, Ptr]),
    rt!(Dict, "lumia_dict_show",     Ptr, [I32, Ptr, I64]),
];

const FLOAT_ABI_DECLS: &[RtDecl] = &[
    rt!(FloatAbi, lumia_abi::ENSURE_MAP_F64,   Ptr, [Ptr]),
    rt!(FloatAbi, lumia_abi::ENSURE_MAP_VF64,  Ptr, [Ptr]),
    rt!(FloatAbi, lumia_abi::ENSURE_SET_F64,   Ptr, [Ptr]),
    rt!(FloatAbi, lumia_abi::ENSURE_LIST_F64,  Ptr, [Ptr]),
    rt!(FloatAbi, lumia_abi::ENSURE_MAP_BOOL,  Ptr, [Ptr]),
    rt!(FloatAbi, lumia_abi::ENSURE_MAP_VBOOL, Ptr, [Ptr]),
    rt!(FloatAbi, lumia_abi::ENSURE_SET_BOOL,  Ptr, [Ptr]),
    rt!(FloatAbi, lumia_abi::ENSURE_LIST_BOOL, Ptr, [Ptr]),
];

const DENSE_FLOAT_DECLS: &[RtDecl] = &[
    rt!(DenseFloat, "lumia_list_f64_zeros",  Ptr, [I64]),
    rt!(DenseFloat, "lumia_f64_fill",        Ptr, [Ptr, F64]),
    rt!(DenseFloat, "lumia_f64_scale",       Ptr, [Ptr, F64]),
    rt!(DenseFloat, "lumia_f64_sqrt",        F64, [F64]),
    rt!(DenseFloat, "lumia_f64_exp",         F64, [F64]),
    rt!(DenseFloat, "lumia_f64_sin",         F64, [F64]),
    rt!(DenseFloat, "lumia_f64_cos",         F64, [F64]),
    rt!(DenseFloat, "lumia_f64_atan2",       F64, [F64, F64]),
    rt!(DenseFloat, "lumia_f64_hypot",       F64, [F64, F64]),
    rt!(DenseFloat, "lumia_f64_mul",         Ptr, [Ptr, Ptr, Ptr]),
    rt!(DenseFloat, "lumia_f64_add",         Ptr, [Ptr, Ptr, Ptr]),
    rt!(DenseFloat, "lumia_f64_sub",         Ptr, [Ptr, Ptr, Ptr]),
    rt!(DenseFloat, "lumia_f64_copy",        Ptr, [Ptr, Ptr]),
    rt!(DenseFloat, "lumia_f64_l2_norm",     F64, [Ptr]),
    rt!(DenseFloat, "lumia_f64_sum_sq",      F64, [Ptr]),
    rt!(DenseFloat, "lumia_f64_mean",        F64, [Ptr]),
    rt!(DenseFloat, "lumia_f64_std",         F64, [Ptr]),
    rt!(DenseFloat, "lumia_f64_softmax",     Ptr, [Ptr]),
    rt!(DenseFloat, "lumia_f64_l2_normalize", Ptr, [Ptr, F64]),
    rt!(DenseFloat, "lumia_f64_clamp",       Ptr, [Ptr, F64, F64]),
    rt!(DenseFloat, "lumia_f64_gemv",        Ptr, [I64, I64, Ptr, Ptr, Ptr]),
    rt!(DenseFloat, "lumia_f64_gemv_t",      Ptr, [I64, I64, Ptr, Ptr, Ptr]),
    rt!(DenseFloat, "lumia_f64_addmm",       Ptr, [I64, I64, Ptr, Ptr, Ptr, F64]),
    rt!(DenseFloat, "lumia_f64_axpy",        Ptr, [Ptr, F64, Ptr]),
    rt!(DenseFloat, "lumia_f64_checksum",    I64, [Ptr]),
];

const DOMAIN_SR_DECLS: &[RtDecl] = &[
    rt!(DomainSr, "lumia_collatz_steps",           I64, [I64]),
    rt!(DomainSr, "lumia_collatz_total",           I64, [I64]),
    rt!(DomainSr, "lumia_collatz_strided",         I64, [I64, I64, I64]),
    rt!(DomainSr, "lumia_mandelbrot_checksum",     I64, [I64]),
    rt!(DomainSr, "lumia_count_primes",            I64, [I64]),
    rt!(DomainSr, "lumia_affine2_rem_sum",         I64, [I64, I64, I64, I64, I64]),
    rt!(DomainSr, "lumia_gcd_sum",                 I64, [I64]),
    rt!(DomainSr, "lumia_divisor_sum",             I64, [I64]),
    rt!(DomainSr, "lumia_product_rem_sum",         I64, [I64, I64]),
    rt!(DomainSr, "lumia_affine1_rem_sum",         I64, [I64, I64, I64, I64]),
    rt!(DomainSr, "lumia_matmul_affine_checksum",  I64, [I64, I64]),
    rt!(DomainSr, "lumia_mem_traffic_checksum",    I64, [I64, I64, I64]),
    rt!(DomainSr, "lumia_float_orbit_checksum",    I64, [I64, I64]),
];

const CN_DECLS: &[RtDecl] = &[
    rt!(Cn, "lumia_efe_action_scores",             Ptr, [Ptr, Ptr, Ptr, I64, I64, I64, I64, F64, F64, F64, F64, F64, F64, F64]),
    rt!(Cn, "lumia_efe_embodied_action_scores",    Ptr, [Ptr, Ptr, Ptr, I64, I64, I64, I64, F64, F64, F64, F64, F64, F64, F64, F64, F64, F64]),
    rt!(Cn, "lumia_efe_apply_embodied_reflexes",   Ptr, [Ptr, Ptr, Ptr, I64, I64, F64]),
    rt!(Cn, "lumia_cn_nucleus_step",               Ptr, [Ptr, Ptr, Ptr, Ptr, Ptr, Ptr, Ptr, I64, F64, F64, F64]),
    rt!(Cn, "lumia_cn_hebbian",                    Ptr, [Ptr, Ptr, Ptr, Ptr, I64, I64, F64, F64, F64, F64]),
    rt!(Cn, "lumia_cn_project_clamp",              Ptr, [I64, I64, Ptr, Ptr, Ptr, F64]),
    rt!(Cn, "lumia_cn_backproj_clamp",             Ptr, [I64, I64, Ptr, Ptr, Ptr, F64]),
    rt!(Cn, "lumia_cn_axpy_clamp",                 Ptr, [Ptr, F64, Ptr, F64]),
    rt!(Cn, "lumia_cn_argmax",                     I64, [Ptr]),
    rt!(Cn, "lumia_cn_cluster_rates",              Ptr, [Ptr, Ptr, Ptr, I64, I64, F64, F64]),
    rt!(Cn, "lumia_cn_learn_generative",           Ptr, [Ptr, Ptr, Ptr, Ptr, I64, F64, F64, F64, F64]),
    rt!(Cn, "lumia_cn_update_state",               Ptr, [Ptr, Ptr, Ptr, I64, F64, F64, F64]),
];

const MEMO_DECLS: &[RtDecl] = &[
    rt!(Memo, "lumia_memo_l2_lookup",  I64, [I64, I64, I64, I64, I64, I64, Ptr]),
    rt!(Memo, "lumia_memo_l2_store",   Void, [I64, I64, I64, I64, I64, I64, I64]),
    rt!(Memo, "lumia_memo_l2_hits",    I64, []),
    rt!(Memo, "lumia_memo_l2_misses",  I64, []),
    rt!(Memo, "lumia_memo_l2_reset",   Void, []),
    rt!(Memo, "lumia_memo_idx_lookup", I64, [I64, I64, Ptr]),
    rt!(Memo, "lumia_memo_idx_store",  Void, [I64, I64, I64]),
    rt!(Memo, "lumia_memo_idx_hits",   I64, []),
    rt!(Memo, "lumia_memo_idx_misses", I64, []),
    rt!(Memo, "lumia_memo_idx_reset",  Void, []),
];

const TASK_DECLS: &[RtDecl] = &[
    rt!(Task, "lumia_channel_new",       Ptr, [I64]),
    rt!(Task, "lumia_channel_send",      Void, [Ptr, I64]),
    rt!(Task, "lumia_channel_recv",      I64, [Ptr]),
    rt!(Task, "lumia_channel_recv_opt",  I64, [Ptr, Ptr]),
    rt!(Task, "lumia_channel_close",     Void, [Ptr]),
    rt!(Task, "lumia_task_spawn",        Ptr, [Ptr, I64]),
    rt!(Task, "lumia_task_spawn_nullary", Ptr, [Ptr]),
    rt!(Task, "lumia_task_join",         I64, [Ptr]),
    rt!(Task, "lumia_task_join_opt",     I64, [Ptr, Ptr]),
    rt!(Task, "lumia_scope_enter",       Void, [I64]),
    rt!(Task, "lumia_scope_leave",       Void, []),
    rt!(Task, "lumia_scope_cancel",      Void, []),
    rt!(Task, "lumia_scheduler_drain",   Void, []),
    rt!(Task, "lumia_scheduler_kind",    I64, [I64]),
];

const ABI_DECLS: &[RtDecl] = &[
    rt!(Abi, "lumia_abi_handoff_set", Void, [I64]),
];

// ---------------------------------------------------------------------------
// All sub-tables, in a fixed order for deterministic iteration.
// ---------------------------------------------------------------------------

const ALL_SUBTABLES: &[&[RtDecl]] = &[
    IO_DECLS,
    ADT_DECLS,
    GC_DECLS,
    LIST_DECLS,
    MAP_SET_DECLS,
    STRING_DECLS,
    COMPARE_DECLS,
    TRAP_DECLS,
    DICT_DECLS,
    FLOAT_ABI_DECLS,
    DENSE_FLOAT_DECLS,
    DOMAIN_SR_DECLS,
    CN_DECLS,
    MEMO_DECLS,
    TASK_DECLS,
    ABI_DECLS,
];

// ---------------------------------------------------------------------------
// Emit helpers
// ---------------------------------------------------------------------------

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
    for subtable in ALL_SUBTABLES {
        for decl in *subtable {
            if module.get_function(decl.name).is_some() {
                continue;
            }
            let fv = module.add_function(decl.name, fn_type(context, decl), None);
            crate::attrs::add_nounwind(context, fv);
        }
    }
}

/// Declared `lumia_*` names (tests: specialized Show table ↔ decls).
#[cfg(test)]
pub(crate) fn runtime_decl_names_for_test() -> rustc_hash::FxHashSet<&'static str> {
    ALL_SUBTABLES
        .iter()
        .flat_map(|t| t.iter())
        .map(|d| d.name)
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use inkwell::context::Context;
    use lumia_hir::Builtin;
    use rustc_hash::FxHashSet as HashSet;

    fn all_decls() -> impl Iterator<Item = &'static RtDecl> {
        ALL_SUBTABLES.iter().flat_map(|t| t.iter())
    }

    #[test]
    fn runtime_decl_names_are_unique() {
        let mut seen = HashSet::default();
        for d in all_decls() {
            assert!(
                seen.insert(d.name),
                "duplicate runtime decl name {}",
                d.name
            );
        }
    }

    #[test]
    fn every_dense_f64_trampoline_is_declared() {
        let names: HashSet<&str> = all_decls().map(|d| d.name).collect();
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
        declare_runtime(&context, &module);
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
        let decl_names: HashSet<&str> = all_decls().map(|d| d.name).collect();
        let rt_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../lumia_rt");
        let mut rt_syms = HashSet::default();
        collect_rt_c_exports(&rt_root, &mut rt_syms);

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

    /// Every [`RtSubsystem`] variant is covered by at least one declaration.
    #[test]
    fn every_subsystem_has_decls() {
        let used: HashSet<RtSubsystem> = all_decls().map(|d| d.subsystem).collect();
        let all = [
            RtSubsystem::Io,
            RtSubsystem::Adt,
            RtSubsystem::Gc,
            RtSubsystem::List,
            RtSubsystem::MapSet,
            RtSubsystem::String,
            RtSubsystem::Compare,
            RtSubsystem::Trap,
            RtSubsystem::Dict,
            RtSubsystem::FloatAbi,
            RtSubsystem::DenseFloat,
            RtSubsystem::DomainSr,
            RtSubsystem::Cn,
            RtSubsystem::Memo,
            RtSubsystem::Task,
            RtSubsystem::Abi,
        ];
        for &s in &all {
            assert!(used.contains(&s), "RtSubsystem::{s:?} has no declarations");
        }
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
