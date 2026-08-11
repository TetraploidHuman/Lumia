//! Function / block emission and mutable slots.

mod abi;
mod helpers;
mod slots;

use super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, FunctionValue};
use lumia_core::{Block, CoreFun, Local, MemoTf, Op, Value};
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_function(&mut self, fun: &CoreFun) -> Result<()> {
        let fv = *self
            .funs
            .functions
            .get(&fun.name)
            .context("missing function decl")?;
        let entry = self.llvm.context.append_basic_block(fv, "entry");
        self.llvm.builder.position_at_end(entry);
        self.frame.entry_bb = Some(entry);
        self.frame.locals.clear();
        self.frame.slots.clear();
        self.frame.float_slots.clear();
        self.frame.loop_stack.clear();
        self.memo.memo_arg_slots.clear();
        self.memo.memo_idx_key = None;
        self.frame.root_depth = 0;
        self.frame.rooted_slots.clear();
        self.funs.funref_locals.clear();
        self.frame.local_tys.clear();
        self.frame.slot_tys.clear();
        self.frame.emit_dest = None;
        self.frame.nsw_binop_locals = crate::nsw_iv::collect_nsw_binop_locals(&fun.body);
        self.frame.safe_divisor_locals = crate::nsw_iv::collect_safe_divisor_locals(&fun.body);
        self.frame.nonneg_iv_load_locals = crate::nsw_iv::collect_nonneg_iv_load_locals(&fun.body);
        self.funs.current_fun = fun.name.clone();
        self.memo.current_memo = fun.memo;
        self.funs.tco_peers = self
            .funs
            .tco_sccs
            .get(&fun.name)
            .cloned()
            .unwrap_or_default();
        let frame_name = if fun.is_main {
            "main"
        } else {
            fun.name.as_str()
        };
        self.emit_frame_push(frame_name)?;

        for (i, p) in fun.params.iter().enumerate() {
            let av = fv.get_nth_param(i as u32).context("function param")?;
            let ty = fun.param_tys.get(i).cloned().unwrap_or(Type::Int);
            self.frame.local_tys.insert(p.0, ty.clone());
            if matches!(ty, Type::Float) {
                let bits = av.into_int_value();
                let f = crate::error::llvm(self.llvm.builder.build_bit_cast(
                    bits,
                    self.llvm.context.f64_type(),
                    "arg_f64",
                ))?;
                self.frame.locals.insert(p.0, f);
            } else {
                self.frame.locals.insert(p.0, av);
                if Self::type_may_heap(&ty) {
                    let bits = self.coerce_i64(av)?;
                    self.root_push_i64(bits)?;
                }
            }
        }

        let compute_bb = match fun.memo {
            Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_prologue(fun, fv, id)?,
            Some(MemoTf::Slots { id }) => self.emit_memo_tf_prologue(fun, fv, id)?,
            None => entry,
        };
        if fun.memo.is_some() {
            self.llvm.builder.position_at_end(compute_bb);
        }

        let result = self.emit_block(&fun.body, fv)?;
        // Tail-call / break paths may already have terminated the block.
        if self
            .llvm
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some()
        {
            return Ok(());
        }
        let ret = result.unwrap_or_else(|| self.llvm.i64_ty.const_int(0, false).into());
        let ret_i = if matches!(fun.ret_ty, Type::Float) {
            match ret {
                BasicValueEnum::FloatValue(f) => crate::error::llvm(
                    self.llvm
                        .builder
                        .build_bit_cast(f, self.llvm.i64_ty, "ret_f64_bits"),
                )?
                .into_int_value(),
                other => self.coerce_i64(other)?,
            }
        } else {
            self.coerce_i64(ret)?
        };

        match fun.memo {
            Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_store(id, ret_i)?,
            Some(MemoTf::Slots { id }) => self.emit_memo_tf_store(id, ret_i)?,
            None => {}
        }
        self.emit_return_i64(ret_i)?;
        Ok(())
    }

    pub(crate) fn infer_value_ty(&self, value: &Value) -> Type {
        lumia_core::infer_value_ty_ctx(value, self.infer_ctx(), None)
    }

    /// Pure self/mutual recursion in tail position → musttail (DESIGN §4.4).
    /// Returns `Ok(true)` if the block was terminated by a musttail call.
    fn try_emit_tco_let(&mut self, block: &Block, local: Local, value: &Value) -> Result<bool> {
        let is_block_tail = block.result == Some(local)
            && matches!(
                block.ops.last(),
                Some(Op::Let { local: last, .. }) if *last == local
            );
        if self.funs.tco_peers.is_empty() || !is_block_tail {
            return Ok(false);
        }
        match value {
            Value::Call { fun, args } => {
                if self.funs.tco_peers.contains(fun) {
                    self.root_pop_to(0)?;
                    self.emit_frame_pop()?;
                    if self.emit_musttail_call(fun, args)? {
                        return Ok(true);
                    }
                    // musttail failed — restore frame for normal call path.
                    self.emit_frame_push(&self.funs.current_fun.clone())?;
                }
            }
            Value::IndirectCall { callee, args } => {
                if let Some(fun) = self.funs.funref_locals.get(&callee.0).cloned() {
                    if self.funs.tco_peers.contains(&fun) {
                        self.root_pop_to(0)?;
                        self.emit_frame_pop()?;
                        if self.emit_musttail_call(&fun, args)? {
                            return Ok(true);
                        }
                        self.emit_frame_push(&self.funs.current_fun.clone())?;
                    }
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn bind_let_after_emit(
        &mut self,
        local: Local,
        value: &Value,
        v: BasicValueEnum<'ctx>,
    ) -> Result<()> {
        if self.value_may_heap(value) {
            if let Ok(bits) = self.coerce_i64(v) {
                // Alias: `val a = xs` must bump List COW refcount.
                if matches!(value, Value::Local(_) | Value::Name(_)) {
                    self.list_retain_i64(bits)?;
                }
                self.root_push_i64(bits)?;
            }
        }
        self.frame.locals.insert(local.0, v);
        self.frame
            .local_tys
            .insert(local.0, self.infer_value_ty(value));
        if let Value::FunRef(name) = value {
            self.funs.funref_locals.insert(local.0, name.clone());
        } else if let Value::Local(Local(src)) = value {
            if let Some(n) = self.funs.funref_locals.get(src).cloned() {
                self.funs.funref_locals.insert(local.0, n);
            } else {
                self.funs.funref_locals.remove(&local.0);
            }
        } else {
            self.funs.funref_locals.remove(&local.0);
        }
        Ok(())
    }

    fn emit_block(
        &mut self,
        block: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        for op in &block.ops {
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
                    self.frame.emit_dest = Some(local.0);
                    let v = self.emit_value(value, fv)?;
                    self.frame.emit_dest = None;
                    self.bind_let_after_emit(*local, value, v)?;
                }
                Op::Effect { value } => {
                    let _ = self.emit_value(value, fv)?;
                }
                Op::Assign { name, value } => {
                    let v = self.local(*value)?;
                    if let Some(ty) = self.frame.local_tys.get(&value.0).cloned() {
                        if !matches!(ty, Type::Float) {
                            self.frame.slot_tys.insert(name.clone(), ty);
                        }
                    }
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
