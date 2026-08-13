//! GC shadow-stack roots and debug frame helpers.

use super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::types::{BasicTypeEnum, IntType};
use inkwell::values::{IntValue, PointerValue};
use inkwell::AddressSpace;
use lumia_core::{Block, Op, Value};
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn type_may_heap(ty: &Type) -> bool {
        match ty {
            Type::String
            | Type::Char
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Set(_)
            | Type::Adt { .. }
            | Type::Fun(_, _, _) => true,
            Type::Tuple(ts) | Type::TuplePrefix(ts) => ts.iter().any(Self::type_may_heap),
            _ => false,
        }
    }

    pub(crate) fn value_may_heap(&self, v: &Value) -> bool {
        use lumia_core::{value_alloc_may_heap, HeapPolicy};
        if value_alloc_may_heap(v, HeapPolicy::StackLitOk) {
            return true;
        }
        match v {
            Value::IndirectCall { .. } => true,
            // Only when an arm's result may be heap — parent `Let` re-roots after
            // scoped pop. Pure Int/Unit ifs must not allocate root slots.
            Value::If {
                then_block,
                else_block,
                ..
            } => self.block_result_may_heap(then_block) || self.block_result_may_heap(else_block),
            Value::Call { fun, .. } => self
                .funs
                .fun_ret_tys
                .get(fun)
                .map(Self::type_may_heap)
                .unwrap_or(true),
            Value::Builtin { name, .. } => match name.result_heap() {
                lumia_hir::ResultHeap::Never => false,
                lumia_hir::ResultHeap::Always => true,
                lumia_hir::ResultHeap::Typed => Self::type_may_heap(&self.infer_value_ty(v)),
            },
            _ => false,
        }
    }

    /// Whether a block's SSA result may be a heap pointer (for If re-rooting).
    fn block_result_may_heap(&self, block: &Block) -> bool {
        let Some(r) = block.result else {
            return false;
        };
        for op in block.ops.iter().rev() {
            if let Op::Let { local, value, .. } = op {
                if *local == r {
                    return self.value_may_heap(value);
                }
            }
        }
        // Result is an outer local — already rooted at its definition.
        false
    }

    pub(crate) fn root_push_i64(&mut self, bits: IntValue<'ctx>) -> Result<()> {
        let slot = self.alloca_in_entry(self.llvm.i64_ty, "gc_root")?;
        crate::error::llvm(self.llvm.builder.build_store(slot, bits))?;
        let push = self.runtime_fn("lumia_root_push")?;
        crate::error::llvm(self.llvm.builder.build_call(push, &[slot.into()], ""))?;
        self.frame.root_depth += 1;
        Ok(())
    }

    /// Bump List COW refcount when aliasing a heap list as i64 bits.
    pub(crate) fn list_retain_i64(&self, bits: IntValue<'ctx>) -> Result<()> {
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let p = self
            .llvm
            .builder
            .build_int_to_ptr(bits, ptr_ty, "list_rc_ptr")
            .map_err(|e| anyhow::anyhow!("int_to_ptr retain: {e}"))?;
        self.call_rt_void("lumia_list_retain", &[p.into()], "list_retain")
    }

    /// Drop a List alias when overwriting a mut slot (no-op for non-lists).
    pub(crate) fn list_release_i64(&self, bits: IntValue<'ctx>) -> Result<()> {
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let p = self
            .llvm
            .builder
            .build_int_to_ptr(bits, ptr_ty, "list_rc_ptr")
            .map_err(|e| anyhow::anyhow!("int_to_ptr release: {e}"))?;
        self.call_rt_void("lumia_list_release", &[p.into()], "list_release")
    }

    /// Bump List **or** ADT COW refcount (`val a = p`, nested `AdtField`).
    pub(crate) fn adt_retain_i64(&self, bits: IntValue<'ctx>) -> Result<()> {
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let p = self
            .llvm
            .builder
            .build_int_to_ptr(bits, ptr_ty, "adt_rc_ptr")
            .map_err(|e| anyhow::anyhow!("int_to_ptr adt_retain: {e}"))?;
        self.call_rt_void("lumia_adt_retain", &[p.into()], "adt_retain")
    }

    /// Drop List **or** ADT alias (mut-slot overwrite).
    pub(crate) fn adt_release_i64(&self, bits: IntValue<'ctx>) -> Result<()> {
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let p = self
            .llvm
            .builder
            .build_int_to_ptr(bits, ptr_ty, "adt_rc_rel")
            .map_err(|e| anyhow::anyhow!("int_to_ptr adt_release: {e}"))?;
        self.call_rt_void("lumia_adt_release", &[p.into()], "adt_release")
    }

    /// Heap COW types that need retain on alias / extract.
    pub(crate) fn type_needs_cow_retain(ty: &Type) -> bool {
        matches!(
            ty,
            Type::List(_) | Type::Map(_, _) | Type::Set(_) | Type::Adt { .. }
        )
    }

    /// `alloca` at function entry so loops do not grow the native stack.
    pub(crate) fn alloca_in_entry(
        &mut self,
        ty: IntType<'ctx>,
        name: &str,
    ) -> Result<PointerValue<'ctx>> {
        self.alloca_in_entry_ty(ty.into(), name)
    }

    pub(crate) fn alloca_in_entry_ty(
        &mut self,
        ty: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> Result<PointerValue<'ctx>> {
        let entry = self
            .frame
            .entry_bb
            .context("alloca_in_entry before emit_function")?;
        let cur = self
            .llvm
            .builder
            .get_insert_block()
            .context("no insert block")?;
        // Insert before the first non-alloca, or at end if entry is empty/only allocas.
        match entry.get_first_instruction() {
            Some(first) => self.llvm.builder.position_before(&first),
            None => self.llvm.builder.position_at_end(entry),
        }
        let slot = crate::error::llvm(match ty {
            BasicTypeEnum::IntType(t) => self.llvm.builder.build_alloca(t, name),
            BasicTypeEnum::FloatType(t) => self.llvm.builder.build_alloca(t, name),
            BasicTypeEnum::PointerType(t) => self.llvm.builder.build_alloca(t, name),
            BasicTypeEnum::ArrayType(t) => self.llvm.builder.build_alloca(t, name),
            BasicTypeEnum::StructType(t) => self.llvm.builder.build_alloca(t, name),
            BasicTypeEnum::VectorType(t) => self.llvm.builder.build_alloca(t, name),
            BasicTypeEnum::ScalableVectorType(t) => self.llvm.builder.build_alloca(t, name),
        })?;
        self.llvm.builder.position_at_end(cur);
        Ok(slot)
    }

    pub(crate) fn root_register_slot(
        &mut self,
        slot: PointerValue<'ctx>,
        name: &str,
    ) -> Result<()> {
        if self.frame.rooted_slots.contains(name) {
            return Ok(());
        }
        let push = self.runtime_fn("lumia_root_push")?;
        crate::error::llvm(self.llvm.builder.build_call(push, &[slot.into()], ""))?;
        self.frame.root_depth += 1;
        self.frame.rooted_slots.insert(name.to_string());
        Ok(())
    }

    /// Pop shadow-stack entries until `root_depth == depth` (scope exit).
    pub(crate) fn root_pop_to(&mut self, depth: u32) -> Result<()> {
        debug_assert!(self.frame.root_depth >= depth);
        let pop = self.runtime_fn("lumia_root_pop")?;
        while self.frame.root_depth > depth {
            crate::error::llvm(self.llvm.builder.build_call(pop, &[], ""))?;
            self.frame.root_depth -= 1;
        }
        Ok(())
    }

    fn emit_root_epilogue(&mut self) -> Result<()> {
        // Emit pops for the current compile-time depth without clearing it:
        // early returns (memo hit) share the counter with the compute path.
        let pop = self.runtime_fn("lumia_root_pop")?;
        for _ in 0..self.frame.root_depth {
            crate::error::llvm(self.llvm.builder.build_call(pop, &[], ""))?;
        }
        Ok(())
    }

    pub(crate) fn emit_frame_push(&mut self, name: &str) -> Result<()> {
        // Release: omit backtrace frames — traps still abort; hot leaves (fib / isPrime)
        // no longer pay TLS Vec push/pop on every call.
        if self.release {
            return Ok(());
        }
        let push = self.runtime_fn("lumia_frame_push")?;
        let s = self
            .llvm
            .builder
            .build_global_string_ptr(name, &format!(".fname.{name}"))
            .context("global string")?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_call(push, &[s.as_pointer_value().into()], ""),
        )?;
        Ok(())
    }

    pub(crate) fn emit_frame_pop(&mut self) -> Result<()> {
        if self.release {
            return Ok(());
        }
        let pop = self.runtime_fn("lumia_frame_pop")?;
        crate::error::llvm(self.llvm.builder.build_call(pop, &[], ""))?;
        Ok(())
    }

    pub(crate) fn emit_return_i64(&mut self, ret: IntValue<'ctx>) -> Result<()> {
        self.emit_root_epilogue()?;
        self.emit_frame_pop()?;
        crate::error::llvm(self.llvm.builder.build_return(Some(&ret)))?;
        Ok(())
    }
}
