//! Runtime extern declarations for `lumia_rt`.

use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::AddressSpace;

pub(crate) fn declare_runtime<'ctx>(context: &'ctx Context, module: &LlvmModule<'ctx>) {
    let i64_ty = context.i64_type();
    let i32_ty = context.i32_type();
    let i8_ty = context.i8_type();
    let ptr_ty = context.ptr_type(AddressSpace::default());
    let void_ty = context.void_type();

    module.add_function(
        "lumia_println_int",
        void_ty.fn_type(&[i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_println_auto",
        void_ty.fn_type(&[i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_println_float",
        void_ty.fn_type(&[context.f64_type().into()], false),
        None,
    );
    module.add_function(
        "lumia_eq",
        i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_adt_eq",
        i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_adt_set_float_mask",
        void_ty.fn_type(&[ptr_ty.into(), i32_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_show_adt",
        ptr_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_show_adt_named",
        ptr_ty.fn_type(
            &[i64_ty.into(), i64_ty.into(), ptr_ty.into(), i64_ty.into()],
            false,
        ),
        None,
    );
    module.add_function(
        "lumia_adt_register_show",
        void_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_cmp",
        i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_println_str",
        void_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_println_cstr",
        void_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_println_bool",
        void_ty.fn_type(&[i8_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_alloc",
        ptr_ty.fn_type(&[i64_ty.into(), i32_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_alloc_string",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_string_cstr",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_cstr_to_string",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_alloc_char",
        ptr_ty.fn_type(&[i64_ty.into()], false),
        None,
    );
    module.add_function("lumia_show", ptr_ty.fn_type(&[i64_ty.into()], false), None);
    module.add_function(
        "lumia_show_float",
        ptr_ty.fn_type(&[context.f64_type().into()], false),
        None,
    );
    module.add_function(
        "lumia_show_bool",
        ptr_ty.fn_type(&[i8_ty.into()], false),
        None,
    );
    module.add_function("lumia_gc_collect", void_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_list_retain",
        void_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_release",
        void_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_root_push",
        void_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_frame_push",
        void_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function("lumia_frame_pop", void_ty.fn_type(&[], false), None);
    module.add_function("lumia_root_pop", void_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_frame_push",
        void_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function("lumia_frame_pop", void_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_dict_register",
        void_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_dict_lookup",
        ptr_ty.fn_type(&[i32_ty.into(), ptr_ty.into()], false),
        None,
    );
    // `lumia_write_barrier` stays in `lumia_rt` ABI for future concurrent GC;
    // STW mark-sweep does not emit calls.
    module.add_function(
        "lumia_list_promote",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_len",
        i64_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_get",
        i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_slice",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_append",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_concat",
        ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function("lumia_list_empty", ptr_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_map_finish",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_set_finish",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function("lumia_len", i64_ty.fn_type(&[ptr_ty.into()], false), None);
    module.add_function(
        "lumia_concat",
        ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_get",
        i64_ty.fn_type(
            &[ptr_ty.into(), i64_ty.into(), i64_ty.into(), i64_ty.into()],
            false,
        ),
        None,
    );
    module.add_function(
        "lumia_contains",
        i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_ensure_map_f64",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_ensure_map_vf64",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_ensure_set_f64",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_ensure_list_f64",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_map_set",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_set",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_set",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_map_remove",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_set_insert",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_remove",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_map_keys",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function("lumia_elems", ptr_ty.fn_type(&[ptr_ty.into()], false), None);
    module.add_function(
        "lumia_map_values",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_map_items",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_adt_tag",
        i64_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_adt_field",
        i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_range",
        ptr_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_range_inclusive",
        ptr_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_trim",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_len",
        i64_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_to_lower",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_to_upper",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_split",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_substring",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_take",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_reverse",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_sort",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_sort_by_keys",
        ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_par_map",
        ptr_ty.fn_type(
            &[ptr_ty.into(), ptr_ty.into(), context.i32_type().into()],
            false,
        ),
        None,
    );
    module.add_function(
        "lumia_list_par_fold",
        i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_join",
        ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function("lumia_read_stdin", ptr_ty.fn_type(&[], false), None);
    module.add_function("lumia_match_fail", void_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_assert",
        void_ty.fn_type(&[i64_ty.into(), ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function("lumia_trap_div0", void_ty.fn_type(&[], false), None);
    module.add_function("lumia_trap_overflow", void_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_str_starts_with",
        i64_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_ends_with",
        i64_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_memo_l2_lookup",
        i64_ty.fn_type(
            &[
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                ptr_ty.into(),
            ],
            false,
        ),
        None,
    );
    module.add_function(
        "lumia_memo_l2_store",
        context.void_type().fn_type(
            &[
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
            ],
            false,
        ),
        None,
    );
    module.add_function(
        "lumia_memo_idx_lookup",
        i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_memo_idx_store",
        context
            .void_type()
            .fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false),
        None,
    );
}

#[cfg(test)]
mod tests {
    use inkwell::context::Context;
    use lumia_hir::Builtin;

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
