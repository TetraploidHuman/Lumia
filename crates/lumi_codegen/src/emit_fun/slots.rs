//! Mutable slot alloca / load / store (including List COW release).

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, IntValue, PointerValue};
use lumi_ty::Type;

impl<'ctx> Codegen<'ctx> {
    fn slot_may_heap(&self, name: &str) -> bool {
        self.frame
            .slot_tys
            .get(name)
            .map(Self::type_may_heap)
            .unwrap_or(true)
    }

    /// Re-push a heap mut slot if a scoped `root_pop_to` unwound its prior root.
    pub(crate) fn ensure_slot_rooted(&mut self, name: &str) -> Result<()> {
        if self.frame.float_slots.contains(name) || !self.slot_may_heap(name) {
            return Ok(());
        }
        if self.frame.rooted_slots.contains_key(name) {
            return Ok(());
        }
        let Some(alloca) = self.frame.slots.get(name).copied() else {
            return Ok(());
        };
        self.root_register_slot(alloca, name)
    }

    pub(crate) fn ensure_slot(&mut self, name: &str) -> Result<PointerValue<'ctx>> {
        if let Some(p) = self.frame.slots.get(name).copied() {
            // Scoped if/loop may have popped this slot's root while leaving the alloca.
            self.ensure_slot_rooted(name)?;
            return Ok(p);
        }
        // Must be entry alloca — loop-body alloca grows the native stack each iteration.
        let alloca = self.alloca_in_entry(self.llvm.i64_ty, &format!("mut_{name}"))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(alloca, self.llvm.i64_ty.const_int(0, false)),
        )?;
        self.frame.slot_i64_const.insert(name.to_string(), Some(0));
        // Int/Bool vars are not GC roots (same as Float). Unknown / heap-capable
        // slots stay rooted. Assign sets `slot_tys` before the first store.
        self.frame.slots.insert(name.to_string(), alloca);
        if self.slot_may_heap(name) {
            self.root_register_slot(alloca, name)?;
        }
        Ok(alloca)
    }

    pub(crate) fn store_slot(&mut self, name: &str, v: BasicValueEnum<'ctx>) -> Result<()> {
        if let BasicValueEnum::FloatValue(f) = v {
            // Native f64 mut slots — avoid bitcast round-trips in hot float loops.
            if !self.frame.slots.contains_key(name) {
                let fty = self.llvm.context.f64_type();
                let alloca = self.alloca_in_entry_ty(fty.into(), &format!("mut_{name}"))?;
                crate::error::llvm(self.llvm.builder.build_store(alloca, fty.const_float(0.0)))?;
                self.frame.slots.insert(name.to_string(), alloca);
            }
            self.frame.float_slots.insert(name.to_string());
            self.frame.slot_tys.insert(name.to_string(), Type::Float);
            let slot = *self.frame.slots.get(name).context("float slot")?;
            crate::error::llvm(self.llvm.builder.build_store(slot, f))?;
            return Ok(());
        }
        let slot = self.ensure_slot(name)?;
        let i = self.coerce_i64(v)?;
        // COW: releasing the previous List/ADT when the pointer changes keeps
        // uniqueness accurate for `xs = xs.append` / `p = p with` vs snapshots.
        // Skip for known scalars (loop latches: `i = i + 1`).
        let need_cow_release = !self.frame.float_slots.contains(name)
            && match self.frame.slot_tys.get(name) {
                Some(Type::List(_))
                | Some(Type::Map(_, _))
                | Some(Type::Set(_))
                | Some(Type::Adt { .. }) => true,
                Some(t) if Self::is_bit_identity_scalar(t) || matches!(t, Type::Float) => false,
                Some(Type::String) | Some(Type::Fun(_, _, _)) => self.mm_arc,
                Some(Type::Char) => false,
                Some(_) => true, // unknown heap-ish
                None => true,    // unknown — conservative
            };
        if need_cow_release {
            let old = self
                .llvm
                .builder
                .build_load(self.llvm.i64_ty, slot, "slot_old")
                .map_err(|e| anyhow::anyhow!("load slot_old: {e}"))?
                .into_int_value();
            let same = self
                .llvm
                .builder
                .build_int_compare(inkwell::IntPredicate::EQ, old, i, "slot_same")
                .map_err(|e| anyhow::anyhow!("icmp slot_same: {e}"))?;
            let cur_bb = self
                .llvm
                .builder
                .get_insert_block()
                .context("store_slot insert block")?;
            let fv = cur_bb.get_parent().context("store_slot parent")?;
            let rel_bb = self.llvm.context.append_basic_block(fv, "slot_release");
            let cont_bb = self.llvm.context.append_basic_block(fv, "slot_store");
            self.llvm
                .builder
                .build_conditional_branch(same, cont_bb, rel_bb)
                .map_err(|e| anyhow::anyhow!("br slot_same: {e}"))?;
            self.llvm.builder.position_at_end(rel_bb);
            match self.frame.slot_tys.get(name) {
                Some(Type::List(_)) | Some(Type::Map(_, _)) | Some(Type::Set(_)) => {
                    self.list_release_i64(old)?;
                }
                Some(Type::String) | Some(Type::Fun(_, _, _)) if self.mm_arc => {
                    self.heap_release_i64(old)?;
                }
                _ => {
                    self.adt_release_i64(old)?;
                }
            }
            self.llvm
                .builder
                .build_unconditional_branch(cont_bb)
                .map_err(|e| anyhow::anyhow!("br cont: {e}"))?;
            self.llvm.builder.position_at_end(cont_bb);
        }
        self.llvm
            .builder
            .build_store(slot, i)
            .map_err(|e| anyhow::anyhow!("store slot: {e}"))?;
        self.note_slot_i64_const(name, i);
        Ok(())
    }

    pub(crate) fn load_slot(&mut self, name: &str) -> Result<BasicValueEnum<'ctx>> {
        self.ensure_slot_rooted(name)?;
        let slot = self
            .frame
            .slots
            .get(name)
            .copied()
            .with_context(|| format!("unbound mutable `{name}`"))?;
        if self.frame.float_slots.contains(name) {
            let fty = self.llvm.context.f64_type();
            crate::error::llvm(self.llvm.builder.build_load(fty, slot, name))
        } else {
            crate::error::llvm(self.llvm.builder.build_load(self.llvm.i64_ty, slot, name))
        }
    }

    /// Record whether `name` currently holds a compile-time i64 constant.
    pub(crate) fn note_slot_i64_const(&mut self, name: &str, v: inkwell::values::IntValue<'ctx>) {
        let known = v.get_sign_extended_constant();
        self.frame.slot_i64_const.insert(name.to_string(), known);
    }

    pub(crate) fn load_slot_i64(&mut self, name: &str) -> Result<inkwell::values::IntValue<'ctx>> {
        let v = self.load_slot(name)?;
        self.as_i64(v)
    }

    pub(crate) fn store_slot_i64(
        &mut self,
        name: &str,
        v: inkwell::values::IntValue<'ctx>,
    ) -> Result<()> {
        let ptr = *self
            .frame
            .slots
            .get(name)
            .with_context(|| format!("missing slot {name}"))?;
        crate::error::llvm(self.llvm.builder.build_store(ptr, v))?;
        self.note_slot_i64_const(name, v);
        Ok(())
    }

    /// True when the slot's last store was exactly the const `expect`.
    pub(crate) fn slot_known_eq(&self, name: &str, expect: i64) -> bool {
        self.frame.slot_i64_const.get(name) == Some(&Some(expect))
    }

    /// Call an RT helper returning i64.
    pub(crate) fn call_rt_i64(
        &mut self,
        rt_sym: &str,
        label: &str,
        ctx: &str,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Result<IntValue<'ctx>> {
        let rt = self.runtime_fn(rt_sym)?;
        let call = crate::error::llvm(self.llvm.builder.build_call(rt, args, label))?;
        Ok(call
            .try_as_basic_value()
            .basic()
            .with_context(|| ctx.to_string())?
            .into_int_value())
    }

    pub(crate) fn sr_loop_zero(&self) -> BasicValueEnum<'ctx> {
        self.llvm.i64_ty.const_int(0, false).into()
    }

    /// Store RT i64 result + extra slot writes; return loop latch `0`.
    pub(crate) fn emit_rt_i64_stores_and_zero(
        &mut self,
        rt_sym: &str,
        label: &str,
        ctx: &str,
        args: &[BasicMetadataValueEnum<'ctx>],
        rt_result_slot: &str,
        extra_stores: &[(&str, IntValue<'ctx>)],
    ) -> Result<BasicValueEnum<'ctx>> {
        let result = self.call_rt_i64(rt_sym, label, ctx, args)?;
        self.store_slot_i64(rt_result_slot, result)?;
        for &(name, val) in extra_stores {
            self.store_slot_i64(name, val)?;
        }
        Ok(self.sr_loop_zero())
    }

    /// RT call → acc slot + IV slot := `n`; latch zero.
    pub(crate) fn emit_rt_n_to_slots_and_zero(
        &mut self,
        rt_sym: &str,
        label: &str,
        ctx: &str,
        acc_slot: &str,
        iv_slot: &str,
        n: i64,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>> {
        let i_end = self.llvm.i64_ty.const_int(n as u64, true);
        self.emit_rt_i64_stores_and_zero(rt_sym, label, ctx, args, acc_slot, &[(iv_slot, i_end)])
    }

    /// RT call with single `n` arg → acc slot + IV slot := `n + 1`; latch zero.
    pub(crate) fn emit_rt_n_plus1_to_slots_and_zero(
        &mut self,
        rt_sym: &str,
        label: &str,
        ctx: &str,
        acc_slot: &str,
        iv_slot: &str,
        n: i64,
    ) -> Result<BasicValueEnum<'ctx>> {
        let n_val = self.llvm.i64_ty.const_int(n as u64, true);
        let i_end = self.llvm.i64_ty.const_int((n + 1) as u64, true);
        self.emit_rt_i64_stores_and_zero(
            rt_sym,
            label,
            ctx,
            &[n_val.into()],
            acc_slot,
            &[(iv_slot, i_end)],
        )
    }
}
