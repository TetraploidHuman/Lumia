//! Mutable slot alloca / load / store (including List COW release).

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, PointerValue};
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn ensure_slot(&mut self, name: &str) -> Result<PointerValue<'ctx>> {
        if let Some(p) = self.frame.slots.get(name) {
            return Ok(*p);
        }
        // Must be entry alloca — loop-body alloca grows the native stack each iteration.
        let alloca = self.alloca_in_entry(self.llvm.i64_ty, &format!("mut_{name}"))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(alloca, self.llvm.i64_ty.const_int(0, false)),
        )?;
        self.root_register_slot(alloca, name)?;
        self.frame.slots.insert(name.to_string(), alloca);
        Ok(alloca)
    }

    pub(crate) fn store_slot(&mut self, name: &str, v: BasicValueEnum<'ctx>) -> Result<()> {
        if matches!(v, BasicValueEnum::FloatValue(_)) {
            // Float slots are not heap roots; create without rooting.
            if !self.frame.slots.contains_key(name) {
                let alloca = self.alloca_in_entry(self.llvm.i64_ty, &format!("mut_{name}"))?;
                self.frame.slots.insert(name.to_string(), alloca);
            }
            self.frame.float_slots.insert(name.to_string());
            self.frame.slot_tys.insert(name.to_string(), Type::Float);
        }
        let slot = self.ensure_slot(name)?;
        let i = self.coerce_i64(v)?;
        // COW: releasing the previous List when the pointer changes keeps uniqueness
        // accurate for `xs = xs.append(e)` (in-place) vs aliased snapshots.
        if !self.frame.float_slots.contains(name) {
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
            self.list_release_i64(old)?;
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
        Ok(())
    }

    pub(crate) fn load_slot(&self, name: &str) -> Result<BasicValueEnum<'ctx>> {
        let slot = self
            .frame
            .slots
            .get(name)
            .copied()
            .with_context(|| format!("unbound mutable `{name}`"))?;
        let bits = crate::error::llvm(self.llvm.builder.build_load(self.llvm.i64_ty, slot, name))?;
        if self.frame.float_slots.contains(name) {
            crate::error::llvm(self.llvm.builder.build_bit_cast(
                bits.into_int_value(),
                self.llvm.context.f64_type(),
                "mut_f64",
            ))
        } else {
            Ok(bits)
        }
    }
}
