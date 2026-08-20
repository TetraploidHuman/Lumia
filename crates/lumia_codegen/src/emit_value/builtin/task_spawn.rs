//! Task spawn emit (`FunRef` vs heap closure dispatch).

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::BasicValueEnum;
use lumia_core::Local;

impl<'ctx> Codegen<'ctx> {
    /// FunRef: `spawn_nullary(untagged_fn)`. Heap closure: `spawn(fn_from_clos, clos_bits)`.
    pub(super) fn emit_task_spawn(&mut self, args: &[Local]) -> Result<BasicValueEnum<'ctx>> {
        use inkwell::{AddressSpace, IntPredicate};

        let fun_i = self.coerce_i64(self.local(args[0])?)?;
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let one = self
            .llvm
            .i64_ty
            .const_int(lumia_abi::FUNREF_TAG as u64, false);
        let tagged = crate::error::llvm(self.llvm.builder.build_and(fun_i, one, "spawn_tag"))?;
        let is_funref = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::EQ,
            tagged,
            one,
            "spawn_is_fr",
        ))?;
        let cur = self
            .llvm
            .builder
            .get_insert_block()
            .context("spawn insert block")?;
        let parent = cur.get_parent().context("spawn parent")?;
        let fr_bb = self.llvm.context.append_basic_block(parent, "spawn_fr");
        let cl_bb = self.llvm.context.append_basic_block(parent, "spawn_cl");
        let merge = self.llvm.context.append_basic_block(parent, "spawn_merge");
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(is_funref, fr_bb, cl_bb),
        )?;

        self.llvm.builder.position_at_end(fr_bb);
        let not1 = crate::error::llvm(self.llvm.builder.build_not(one, "spawn_not1"))?;
        let cleared = crate::error::llvm(self.llvm.builder.build_and(fun_i, not1, "spawn_fr_clr"))?;
        let fr_ptr = crate::error::llvm(self.llvm.builder.build_int_to_ptr(
            cleared,
            ptr_ty,
            "spawn_fr_ptr",
        ))?;
        let fr_task = self
            .call_rt_ptr_as_i64(
                "lumia_task_spawn_nullary",
                &[fr_ptr.into()],
                "spawn_fr_call",
            )?
            .into_int_value();
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(merge))?;
        let fr_end = self.llvm.builder.get_insert_block().context("fr end")?;

        self.llvm.builder.position_at_end(cl_bb);
        let env_ptr = crate::error::llvm(self.llvm.builder.build_int_to_ptr(
            fun_i,
            ptr_ty,
            "spawn_clos",
        ))?;
        let fn_slot = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                env_ptr,
                &[self.llvm.i64_ty.const_int(0, false)],
                "spawn_fn_slot",
            ))?
        };
        let fn_i = crate::error::llvm(self.llvm.builder.build_load(
            self.llvm.i64_ty,
            fn_slot,
            "spawn_fn",
        ))?
        .into_int_value();
        let cl_fptr = crate::error::llvm(self.llvm.builder.build_int_to_ptr(
            fn_i,
            ptr_ty,
            "spawn_cl_fptr",
        ))?;
        let cl_task = self
            .call_rt_ptr_as_i64(
                "lumia_task_spawn",
                &[cl_fptr.into(), fun_i.into()],
                "spawn_cl_call",
            )?
            .into_int_value();
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(merge))?;
        let cl_end = self.llvm.builder.get_insert_block().context("cl end")?;

        self.llvm.builder.position_at_end(merge);
        let phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "spawn_task"))?;
        phi.add_incoming(&[(&fr_task, fr_end), (&cl_task, cl_end)]);
        Ok(phi.as_basic_value())
    }
}

