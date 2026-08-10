//! Value emission — list builtins (Custom shapes only; simple ones use BuiltinEmit).

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, PointerValue};
use inkwell::{AddressSpace, IntPredicate};
use lumia_core::{Local, Value};
use lumia_hir::Builtin;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_list_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::ListGet => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let idx = self.coerce_i64(self.local(args[1])?)?;
                let list = self.i64_as_ptr(list_i, "col_ptr")?;
                let some = self
                    .llvm
                    .i64_ty
                    .const_int(self.option_some_tag as u64, true);
                let none = self
                    .llvm
                    .i64_ty
                    .const_int(self.option_none_tag as u64, true);
                let sym = Self::builtin_symbol(name)?;
                self.call_rt_basic(
                    sym,
                    &[list.into(), idx.into(), some.into(), none.into()],
                    "get",
                )
            }
            Builtin::ListParMap => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let fun_i = self.coerce_i64(self.local(args[1])?)?;
                let list = self.i64_as_ptr(list_i, "pmap_list")?;
                let fptr = self.ensure_funref_ptr(fun_i, "pmap")?;
                let elem_is_float = matches!(
                    lumia_core::infer_value_ty_ctx(
                        &Value::Builtin {
                            name: Builtin::ListParMap,
                            args: args.to_vec(),
                        },
                        lumia_core::InferValueCtx {
                            local_tys: &self.frame.local_tys,
                            slot_tys: Some(&self.frame.slot_tys),
                            fun_ret_tys: Some(&self.funs.fun_ret_tys),
                            fun_param_tys: Some(&self.funs.fun_param_tys),
                            fun_param0_identity: Some(&self.funs.fun_param0_identity),
                            funref_locals: Some(&self.funs.funref_locals),
                        },
                        None,
                    ),
                    Type::List(e) if matches!(e.as_ref(), Type::Float)
                );
                let result_tid = lumia_abi::list_type_id(elem_is_float) as u64;
                let sym = Self::builtin_symbol(name)?;
                let tid_v = self.llvm.context.i32_type().const_int(result_tid, false);
                self.call_rt_ptr_as_i64(sym, &[list.into(), fptr.into(), tid_v.into()], "par_map")
            }
            Builtin::ListParFold => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let init_i = self.coerce_i64(self.local(args[1])?)?;
                let fun_i = self.coerce_i64(self.local(args[2])?)?;
                let list = self.i64_as_ptr(list_i, "pfold_list")?;
                let fptr = self.ensure_funref_ptr(fun_i, "pfold")?;
                let sym = Self::builtin_symbol(name)?;
                self.call_rt_basic(sym, &[list.into(), init_i.into(), fptr.into()], "par_fold")
            }
            _ => unreachable!(
                "non-custom list builtin `{}` should use BuiltinEmit",
                name.display_name()
            ),
        }
    }

    /// FunRef values are tagged with the low bit; refuse heap closures for par_* workers.
    fn ensure_funref_ptr(
        &mut self,
        fun_i: inkwell::values::IntValue<'ctx>,
        prefix: &str,
    ) -> Result<PointerValue<'ctx>> {
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let one = self.llvm.i64_ty.const_int(1, false);
        let tagged = crate::error::llvm(self.llvm.builder.build_and(
            fun_i,
            one,
            &format!("{prefix}_tag"),
        ))?;
        let is_funref = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::EQ,
            tagged,
            one,
            &format!("{prefix}_is_fr"),
        ))?;
        let cur = self
            .llvm
            .builder
            .get_insert_block()
            .with_context(|| format!("{prefix} needs insert block"))?;
        let parent = cur
            .get_parent()
            .with_context(|| format!("{prefix} bb parent"))?;
        let ok_bb = self
            .llvm
            .context
            .append_basic_block(parent, &format!("{prefix}_ok"));
        let bad_bb = self
            .llvm
            .context
            .append_basic_block(parent, &format!("{prefix}_bad"));
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(is_funref, ok_bb, bad_bb),
        )?;
        self.llvm.builder.position_at_end(bad_bb);
        let fail = self.runtime_fn("lumia_match_fail")?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_call(fail, &[], &format!("{prefix}_bad_fn")),
        )?;
        crate::error::llvm(self.llvm.builder.build_unreachable())?;
        self.llvm.builder.position_at_end(ok_bb);
        let not1 = crate::error::llvm(self.llvm.builder.build_not(one, &format!("{prefix}_not1")))?;
        let cleared = crate::error::llvm(self.llvm.builder.build_and(
            fun_i,
            not1,
            &format!("{prefix}_clear"),
        ))?;
        crate::error::llvm(self.llvm.builder.build_int_to_ptr(
            cleared,
            ptr_ty,
            &format!("{prefix}_fn"),
        ))
    }
}
