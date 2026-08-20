//! Task / Channel Option-shaped RT wrappers (`*_opt(recv, &out_ok)`).

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, IntValue, PhiValue, PointerValue};
use inkwell::IntPredicate;
use lumia_abi::adt_type_id;
use lumia_core::Local;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    /// `lumia_task_join_opt(t, &out_ok)` → Option Some/None (same shape as recvOpt).
    pub(super) fn emit_task_join_opt(&mut self, args: &[Local]) -> Result<BasicValueEnum<'ctx>> {
        let t_i = self.coerce_i64(self.local(args[0])?)?;
        let t = self.i64_as_ptr(t_i, "join_opt_task")?;
        let field_ty = match self.frame.local_tys.get(&args[0].0) {
            Some(Type::Task(e)) => e.as_ref().clone(),
            _ => Type::Int,
        };
        self.emit_opt_from_rt_out_ok("lumia_task_join_opt", t, "join", field_ty)
    }

    /// `lumia_channel_recv_opt(ch, &out_ok)` → Option Some/None ADT (like map get tags).
    pub(super) fn emit_channel_recv_opt(&mut self, args: &[Local]) -> Result<BasicValueEnum<'ctx>> {
        let ch_i = self.coerce_i64(self.local(args[0])?)?;
        let ch = self.i64_as_ptr(ch_i, "ch")?;
        let field_ty = match self.frame.local_tys.get(&args[0].0) {
            Some(Type::Channel(e)) => {
                let elem = e.as_ref().clone();
                if matches!(elem, Type::Int | Type::Var(_)) {
                    self.funs.channel_elem_hint.clone().unwrap_or(elem)
                } else {
                    elem
                }
            }
            _ => self.funs.channel_elem_hint.clone().unwrap_or(Type::Int),
        };
        self.emit_opt_from_rt_out_ok("lumia_channel_recv_opt", ch, "recv", field_ty)
    }

    /// Shared `*_opt(recv, &out_ok)` → Option prologue for joinOpt / recvOpt.
    fn emit_opt_from_rt_out_ok(
        &mut self,
        sym: &'static str,
        recv: PointerValue<'ctx>,
        prefix: &str,
        field_ty: Type,
    ) -> Result<BasicValueEnum<'ctx>> {
        let out_ok = self.alloca_in_entry(self.llvm.i64_ty, &format!("{prefix}_opt_ok"))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(out_ok, self.llvm.i64_ty.const_int(0, false)),
        )?;
        let val = self
            .call_rt_basic(sym, &[recv.into(), out_ok.into()], &format!("{prefix}_opt"))?
            .into_int_value();
        let ok = crate::error::llvm(self.llvm.builder.build_load(self.llvm.i64_ty, out_ok, "ok"))?
            .into_int_value();
        self.emit_option_from_ok_val(ok, val, prefix, &field_ty)
    }

    /// Build `Option` from RT `(ok, val)` with one shared GC root across both CFG arms.
    fn emit_option_from_ok_val(
        &mut self,
        ok: IntValue<'ctx>,
        val: IntValue<'ctx>,
        prefix: &str,
        field_ty: &Type,
    ) -> Result<BasicValueEnum<'ctx>> {
        let is_some = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::NE,
            ok,
            self.llvm.i64_ty.const_int(0, false),
            &format!("{prefix}_is_some"),
        ))?;

        let ptr_ty = self.llvm.context.ptr_type(inkwell::AddressSpace::default());
        let root_slot = self.alloca_in_entry_ty(ptr_ty.into(), &format!("{prefix}_opt_root"))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(root_slot, ptr_ty.const_null()),
        )?;
        let push = self.runtime_fn("lumia_root_push")?;
        crate::error::llvm(self.llvm.builder.build_call(
            push,
            &[root_slot.into()],
            &format!("{prefix}_opt_root_push"),
        ))?;
        self.frame.root_depth += 1;

        let parent = self
            .llvm
            .builder
            .get_insert_block()
            .context("opt insert block")?
            .get_parent()
            .context("opt parent fn")?;
        let some_bb = self
            .llvm
            .context
            .append_basic_block(parent, &format!("{prefix}_some"));
        let none_bb = self
            .llvm
            .context
            .append_basic_block(parent, &format!("{prefix}_none"));
        let merge_bb = self
            .llvm
            .context
            .append_basic_block(parent, &format!("{prefix}_merge"));
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(is_some, some_bb, none_bb),
        )?;

        self.llvm.builder.position_at_end(some_bb);
        let some = self.emit_option_adt_into(
            root_slot,
            self.option_variant_tag("Some"),
            Some(val),
            field_ty,
        )?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(merge_bb))?;
        let some_bb_end = self
            .llvm
            .builder
            .get_insert_block()
            .context("opt some end")?;

        self.llvm.builder.position_at_end(none_bb);
        let none =
            self.emit_option_adt_into(root_slot, self.option_variant_tag("None"), None, field_ty)?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(merge_bb))?;
        let none_bb_end = self
            .llvm
            .builder
            .get_insert_block()
            .context("opt none end")?;

        self.llvm.builder.position_at_end(merge_bb);
        let phi: PhiValue<'ctx> = crate::error::llvm(
            self.llvm
                .builder
                .build_phi(self.llvm.i64_ty, &format!("{prefix}_opt")),
        )?;
        phi.add_incoming(&[(&some, some_bb_end), (&none, none_bb_end)]);
        Ok(phi.as_basic_value())
    }

    /// Allocate/init Option into an already-pushed `root_slot` (no extra `root_depth`).
    fn emit_option_adt_into(
        &mut self,
        root_slot: PointerValue<'ctx>,
        tag: i64,
        field: Option<IntValue<'ctx>>,
        field_ty: &Type,
    ) -> Result<IntValue<'ctx>> {
        let n_fields = if field.is_some() { 1u64 } else { 0 };
        let nbytes = self.llvm.i64_ty.const_int((1 + n_fields) * 8, false);
        let kind = self
            .funs
            .adt_show_kinds
            .get(lumia_hir::OPTION.name)
            .copied()
            .unwrap_or(0);
        let type_id = self
            .llvm
            .context
            .i32_type()
            .const_int(adt_type_id(kind) as u64, false);
        let alloc = self.runtime_fn("lumia_alloc")?;
        let ptr = crate::error::llvm(self.llvm.builder.build_call(
            alloc,
            &[nbytes.into(), type_id.into()],
            "opt_alloc",
        ))?
        .try_as_basic_value()
        .basic()
        .context("opt alloc return")?
        .into_pointer_value();
        crate::error::llvm(self.llvm.builder.build_store(root_slot, ptr))?;
        let tag_slot = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                ptr,
                &[self.llvm.i64_ty.const_int(0, false)],
                "opt_tag",
            ))?
        };
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(tag_slot, self.llvm.i64_ty.const_int(tag as u64, false)),
        )?;
        if let Some(v) = field {
            if Self::type_needs_cow_retain(field_ty) {
                self.adt_retain_i64(v)?;
            }
            let slot = unsafe {
                crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                    self.llvm.i64_ty,
                    ptr,
                    &[self.llvm.i64_ty.const_int(1, false)],
                    "opt_f0",
                ))?
            };
            crate::error::llvm(self.llvm.builder.build_store(slot, v))?;
            self.emit_adt_payload_masks_for_ty(ptr, field_ty)?;
        }
        crate::error::llvm(
            self.llvm
                .builder
                .build_ptr_to_int(ptr, self.llvm.i64_ty, "opt_i64"),
        )
    }
}

