//! Mutable slot alloca / load / store (including List COW release).

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, PointerValue};
use lumia_ty::Type;

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
                Some(Type::String) | Some(Type::Char) => false,
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

    /// True when the slot's last store was exactly the const `expect`.
    pub(crate) fn slot_known_eq(&self, name: &str, expect: i64) -> bool {
        self.frame.slot_i64_const.get(name) == Some(&Some(expect))
    }
}
