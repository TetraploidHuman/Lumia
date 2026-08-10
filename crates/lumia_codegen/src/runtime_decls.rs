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
        name: "lumia_adt_set_float_mask",
        ret: RtTy::Void,
        args: &[RtTy::Ptr, RtTy::I32],
    },
    RtDecl {
        name: "lumia_show_adt",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_show_adt_named",
        ret: RtTy::Ptr,
        args: &[RtTy::I64, RtTy::I64, RtTy::Ptr, RtTy::I64],
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
    // `lumia_write_barrier` stays in `lumia_rt` ABI for future concurrent GC;
    // STW mark-sweep does not emit calls.
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
        args: &[RtTy::Ptr, RtTy::I64, RtTy::I64, RtTy::I64],
    },
    RtDecl {
        name: "lumia_contains",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::I64],
    },
    RtDecl {
        name: "lumia_ensure_map_f64",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_ensure_map_vf64",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_ensure_set_f64",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_ensure_list_f64",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_map_set",
        ret: RtTy::Ptr,
        args: &[RtTy::Ptr, RtTy::I64, RtTy::I64],
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
        args: &[RtTy::Ptr],
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
        name: "lumia_str_starts_with",
        ret: RtTy::I64,
        args: &[RtTy::Ptr, RtTy::Ptr],
    },
    RtDecl {
        name: "lumia_str_ends_with",
        ret: RtTy::I64,
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
        module.add_function(decl.name, fn_type(context, decl), None);
    }
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
}
