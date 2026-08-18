//! Block / Op emission (body scheduling).

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, FunctionValue};
use lumia_core::{Block, MemoTf, Op};
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_block(
        &mut self,
        block: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        self.emit_block_from(block, fv, self.frame.ssa_root_stack.len())
    }

    /// Like [`Self::emit_block`] but may early-pop SSA roots down to `stack_base`.
    /// Function bodies pass `0` so heap params can die; nested blocks pass the
    /// current stack length so they cannot pop outer roots.
    pub(crate) fn emit_block_from(
        &mut self,
        block: &Block,
        fv: FunctionValue<'ctx>,
        stack_base: usize,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        self.pop_unused_ssa_roots(stack_base)?;
        for (idx, op) in block.ops.iter().enumerate() {
            // If current block already terminated (after break/continue), skip.
            if self
                .llvm
                .builder
                .get_insert_block()
                .and_then(|bb| bb.get_terminator())
                .is_some()
            {
                break;
            }
            match op {
                Op::Let { local, value, .. } => {
                    if self.try_emit_tco_let(block, *local, value)? {
                        return Ok(None);
                    }
                    self.frame.cow_consume_unique =
                        self.cow_reassign_consumes(block, idx, *local, value);
                    self.frame.adt_with_inplace =
                        self.match_adt_with_reassign(block, idx, *local, value);
                    self.frame.emit_dest = Some(local.0);
                    self.frame.expect_alloc_ty =
                        self.peek_expected_alloc_ty(block, idx, *local, value);
                    let v = self.emit_value(value, fv)?;
                    self.frame.expect_alloc_ty = None;
                    self.frame.emit_dest = None;
                    self.frame.cow_consume_unique = false;
                    self.frame.adt_with_inplace = None;
                    if self.let_only_feeds_next_assign(block, idx, *local)
                        || self.let_is_ephemeral_rooted_recv(block, idx, *local, value)
                        || self.let_is_ephemeral_call_arg(block, idx, *local, value)
                        || self.let_is_ephemeral_adt_field_base(block, idx, *local, value)
                        || self.let_is_unused_inplace_with_field(block, idx, *local, value)
                    {
                        // Skip retain+root: source is already live (mut slot / prior let).
                        // Extra retain here inflated COW RC and forced kernel-side clones.
                        let ty = self.infer_value_ty(value);
                        self.frame.locals.insert(local.0, v);
                        self.frame.local_tys.insert(local.0, ty);
                        self.note_int_const(local.0, value);
                        self.funs.funref_locals.remove(&local.0);
                    } else if self.let_skip_root_no_safepoint(block, idx, *local, value) {
                        self.bind_let_skip_root(*local, value, v)?;
                    } else {
                        let d0 = self.frame.root_depth;
                        self.bind_let_after_emit(*local, value, v)?;
                        if self.frame.root_depth > d0 {
                            self.note_ssa_root(block, idx + 1, *local);
                        }
                    }
                }
                Op::Assign { name, value } => {
                    let v = self.local(*value)?;
                    // Float ADT fields / lets travel as i64 IEEE bits. Storing them
                    // into a mut slot via coerce_i64 + Int typing makes later float
                    // arith `sitofp` the bit pattern (eco `var s = eco.ecoRng` bug).
                    // Promote to a native f64 slot whenever the RHS is Float-typed.
                    let v = if matches!(self.frame.local_tys.get(&value.0), Some(Type::Float)) {
                        self.frame.slot_tys.insert(name.clone(), Type::Float);
                        self.promote_f64(v)?.into()
                    } else {
                        // Unknown RHS → Int (not heap): keeps `slot_may_heap`
                        // aligned with the unknown→Int default elsewhere.
                        let ty = self
                            .frame
                            .local_tys
                            .get(&value.0)
                            .cloned()
                            .unwrap_or(Type::Int);
                        self.frame.slot_tys.insert(name.clone(), ty);
                        v
                    };
                    self.store_slot(name, v)?;
                }
                Op::Break => {
                    let (_, break_bb, loop_depth) = self
                        .frame
                        .loop_stack
                        .last()
                        .copied()
                        .context("break outside loop")?;
                    self.root_pop_to(loop_depth)?;
                    crate::error::llvm(self.llvm.builder.build_unconditional_branch(break_bb))?;
                }
                Op::Continue => {
                    let (cont_bb, _, loop_depth) = self
                        .frame
                        .loop_stack
                        .last()
                        .copied()
                        .context("continue outside loop")?;
                    self.root_pop_to(loop_depth)?;
                    crate::error::llvm(self.llvm.builder.build_unconditional_branch(cont_bb))?;
                }
                Op::Return { value } => {
                    let v = self.local(*value)?;
                    let ret_i = if matches!(self.frame.local_tys.get(&value.0), Some(Type::Float)) {
                        match v {
                            BasicValueEnum::FloatValue(f) => {
                                crate::error::llvm(self.llvm.builder.build_bit_cast(
                                    f,
                                    self.llvm.i64_ty,
                                    "ret_f64_bits",
                                ))?
                                .into_int_value()
                            }
                            other => self.coerce_i64(other)?,
                        }
                    } else {
                        self.coerce_i64(v)?
                    };
                    match self.memo.current_memo {
                        Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_store(id, ret_i)?,
                        Some(MemoTf::Slots { id }) => self.emit_memo_tf_store(id, ret_i)?,
                        None => {}
                    }
                    self.emit_return_i64(ret_i)?;
                }
            }
            if self
                .llvm
                .builder
                .get_insert_block()
                .and_then(|bb| bb.get_terminator())
                .is_none()
            {
                self.pop_dead_ssa_roots(idx, stack_base)?;
            }
        }
        if self
            .llvm
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some()
        {
            return Ok(None);
        }
        if let Some(r) = block.result {
            Ok(Some(self.local(r)?))
        } else {
            Ok(None)
        }
    }

    /// Emit a nested block and drop roots pushed inside it (unless it terminated
    /// via break/continue, which already restored the loop entry depth).
    pub(crate) fn emit_scoped_block(
        &mut self,
        block: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let depth = self.frame.root_depth;
        let result = self.emit_block(block, fv)?;
        let terminated = self
            .llvm
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some();
        if !terminated {
            self.root_pop_to(depth)?;
        }
        Ok(result)
    }
}
