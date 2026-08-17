//! Value emission — Task / Channel builtins (Custom shapes).

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, IntValue, PhiValue};
use inkwell::IntPredicate;
use lumia_abi::adt_type_id;
use lumia_core::Local;
use lumia_hir::Builtin;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_task_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::ChannelNew => {
                let cap = self.coerce_i64(self.local(args[0])?)?;
                self.call_rt_ptr_as_i64("lumia_channel_new", &[cap.into()], "channel")
            }
            Builtin::ChannelSend => {
                let ch_i = self.coerce_i64(self.local(args[0])?)?;
                let v = self.coerce_i64(self.local(args[1])?)?;
                let ch = self.i64_as_ptr(ch_i, "ch")?;
                self.call_rt_void("lumia_channel_send", &[ch.into(), v.into()], "send")?;
                Ok(self.llvm.i64_ty.const_int(0, false).into())
            }
            Builtin::ChannelClose => {
                let ch_i = self.coerce_i64(self.local(args[0])?)?;
                let ch = self.i64_as_ptr(ch_i, "ch")?;
                self.call_rt_void("lumia_channel_close", &[ch.into()], "close")?;
                Ok(self.llvm.i64_ty.const_int(0, false).into())
            }
            Builtin::ScopeEnter => {
                let kind = self.coerce_i64(self.local(args[0])?)?;
                self.call_rt_void("lumia_scope_enter", &[kind.into()], "scope_enter")?;
                Ok(self.llvm.i64_ty.const_int(0, false).into())
            }
            Builtin::ChannelRecvOpt => self.emit_channel_recv_opt(args),
            Builtin::TaskJoinOpt => self.emit_task_join_opt(args),
            Builtin::TaskSpawn => self.emit_task_spawn(args),
            _ => unreachable!(
                "non-custom task builtin `{}` should use BuiltinEmit",
                name.display_name()
            ),
        }
    }

    /// `lumia_task_join_opt(t, &out_ok)` → Option Some/None (same shape as recvOpt).
    fn emit_task_join_opt(&mut self, args: &[Local]) -> Result<BasicValueEnum<'ctx>> {
        let t_i = self.coerce_i64(self.local(args[0])?)?;
        let t = self.i64_as_ptr(t_i, "join_opt_task")?;
        let out_ok = self.alloca_in_entry(self.llvm.i64_ty, "join_opt_ok")?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(out_ok, self.llvm.i64_ty.const_int(0, false)),
        )?;
        let val = self
            .call_rt_basic(
                "lumia_task_join_opt",
                &[t.into(), out_ok.into()],
                "join_opt",
            )?
            .into_int_value();
        let ok = crate::error::llvm(self.llvm.builder.build_load(self.llvm.i64_ty, out_ok, "ok"))?
            .into_int_value();
        let field_ty = match self.frame.local_tys.get(&args[0].0) {
            Some(Type::Task(e)) => e.as_ref().clone(),
            _ => Type::Int,
        };
        self.emit_option_from_ok_val(ok, val, "join", &field_ty)
    }

    /// FunRef: `spawn_nullary(untagged_fn)`. Heap closure: `spawn(fn_from_clos, clos_bits)`.
    fn emit_task_spawn(&mut self, args: &[Local]) -> Result<BasicValueEnum<'ctx>> {
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
        let cleared =
            crate::error::llvm(self.llvm.builder.build_and(fun_i, not1, "spawn_fr_clr"))?;
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
        let env_ptr = crate::error::llvm(
            self.llvm
                .builder
                .build_int_to_ptr(fun_i, ptr_ty, "spawn_clos"),
        )?;
        let fn_slot = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                env_ptr,
                &[self.llvm.i64_ty.const_int(0, false)],
                "spawn_fn_slot",
            ))?
        };
        let fn_i = crate::error::llvm(
            self.llvm
                .builder
                .build_load(self.llvm.i64_ty, fn_slot, "spawn_fn"),
        )?
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

    /// `lumia_channel_recv_opt(ch, &out_ok)` → Option Some/None ADT (like map get tags).
    fn emit_channel_recv_opt(&mut self, args: &[Local]) -> Result<BasicValueEnum<'ctx>> {
        let ch_i = self.coerce_i64(self.local(args[0])?)?;
        let ch = self.i64_as_ptr(ch_i, "ch")?;
        let out_ok = self.alloca_in_entry(self.llvm.i64_ty, "recv_opt_ok")?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(out_ok, self.llvm.i64_ty.const_int(0, false)),
        )?;
        let val = self
            .call_rt_basic(
                "lumia_channel_recv_opt",
                &[ch.into(), out_ok.into()],
                "recv_opt",
            )?
            .into_int_value();
        let ok = crate::error::llvm(self.llvm.builder.build_load(self.llvm.i64_ty, out_ok, "ok"))?
            .into_int_value();
        let field_ty = match self.frame.local_tys.get(&args[0].0) {
            Some(Type::Channel(e)) => {
                let elem = e.as_ref().clone();
                if matches!(elem, Type::Int | Type::Var(_)) {
                    self.funs
                        .channel_elem_hint
                        .clone()
                        .unwrap_or(elem)
                } else {
                    elem
                }
            }
            _ => self
                .funs
                .channel_elem_hint
                .clone()
                .unwrap_or(Type::Int),
        };
        self.emit_option_from_ok_val(ok, val, "recv", &field_ty)
    }

    /// Build `Option` from RT `(ok, val)` with **one** shared GC root (both CFG arms).
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

        // One root slot for both arms — compile-time depth must match runtime pushes.
        let ptr_ty = self.llvm.context.ptr_type(inkwell::AddressSpace::default());
        let root_slot = self.alloca_in_entry_ty(ptr_ty.into(), &format!("{prefix}_opt_root"))?;
        crate::error::llvm(self.llvm.builder.build_store(root_slot, ptr_ty.const_null()))?;
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
        let some =
            self.emit_option_adt_into(root_slot, self.option_variant_tag("Some"), Some(val), field_ty)?;
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
        root_slot: inkwell::values::PointerValue<'ctx>,
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
            // COW only for heap payloads — Float IEEE bits must not be retained
            // (post-`is_heap_payload` remove, mistagged bits SEGV).
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
            if matches!(field_ty, Type::Float) {
                self.emit_adt_set_float_mask(ptr, 1)?;
            }
            if matches!(field_ty, Type::Bool) {
                self.emit_adt_set_bool_mask(ptr, 1)?;
            }
        }
        crate::error::llvm(
            self.llvm
                .builder
                .build_ptr_to_int(ptr, self.llvm.i64_ty, "opt_i64"),
        )
    }
}
