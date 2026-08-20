//! Function / block emission and mutable slots.

mod abi;
mod block;
mod cow;
mod helpers;
mod let_bind;
mod slots;
mod tco;

use super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, FunctionValue};
use lumia_core::{CoreFun, MemoTf, Value};
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    /// Emit one Core function: prologue → body → epilogue (Todo: emit_fun 上帝模块).
    pub(crate) fn emit_function(&mut self, fun: &CoreFun) -> Result<()> {
        let fv = *self
            .funs
            .functions
            .get(fun.name.as_str())
            .context("missing function decl")?;
        let entry = self.emit_function_prologue(fun, fv)?;
        if self.try_emit_dense_f64_specialization(fun, fv)? {
            return Ok(());
        }
        self.emit_function_frame_and_params(fun, fv)?;
        let compute_bb = self.emit_function_memo_gate(fun, fv, entry)?;
        if fun.memo.is_some() {
            self.llvm.builder.position_at_end(compute_bb);
        }
        let result = self.emit_block_from(&fun.body, fv, 0)?;
        self.emit_function_epilogue(fun, result)
    }

    /// Clear frame state, open entry BB, install NSW / TCO peers / memo stamp.
    fn emit_function_prologue(
        &mut self,
        fun: &CoreFun,
        fv: FunctionValue<'ctx>,
    ) -> Result<inkwell::basic_block::BasicBlock<'ctx>> {
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
        self.frame.rooted_slots = Default::default();
        self.frame.ssa_root_stack.clear();
        self.frame.cow_consume_unique = false;
        self.frame.adt_with_inplace = None;
        self.funs.funref = Default::default();
        self.frame.local_tys.clear();
        self.frame.local_int_consts.clear();
        self.frame.slot_tys.clear();
        self.frame.emit_dest = None;
        self.frame.expect_alloc_ty = None;
        self.frame.install_nsw_from_fun(fun);
        self.funs.current_fun = fun.name.clone();
        self.memo.current_memo = fun.memo;
        self.funs.tco_peers = self
            .funs
            .tco_sccs
            .get(fun.name.as_str())
            .cloned()
            .unwrap_or_default();
        Ok(entry)
    }

    /// Dense List[Float] helpers → thin RT trampoline (no frame / root traffic).
    fn try_emit_dense_f64_specialization(
        &mut self,
        fun: &CoreFun,
        fv: FunctionValue<'ctx>,
    ) -> Result<bool> {
        if fun.memo.is_some() {
            return Ok(false);
        }
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
            }
        }
        if self.dense_f64_sr && self.try_emit_dense_f64_fun(fun, fv)?.is_some() {
            return Ok(true);
        }
        // Fall through: clear param bindings; normal path re-binds with roots.
        self.frame.locals.clear();
        self.frame.local_tys.clear();
        self.frame.local_int_consts.clear();
        Ok(false)
    }

    /// Debug frame push + bind params (with GC roots for heap types).
    fn emit_function_frame_and_params(
        &mut self,
        fun: &CoreFun,
        fv: FunctionValue<'ctx>,
    ) -> Result<()> {
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
                    self.note_ssa_root(&fun.body, 0, *p);
                }
            }
        }
        Ok(())
    }

    fn emit_function_memo_gate(
        &mut self,
        fun: &CoreFun,
        fv: FunctionValue<'ctx>,
        entry: inkwell::basic_block::BasicBlock<'ctx>,
    ) -> Result<inkwell::basic_block::BasicBlock<'ctx>> {
        Ok(match fun.memo {
            Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_prologue(fun, fv, id)?,
            Some(MemoTf::Slots { id }) => self.emit_memo_tf_prologue(fun, fv, id)?,
            None => entry,
        })
    }

    /// Memo store (if any) + return. No-op when the insert block is already terminated.
    fn emit_function_epilogue(
        &mut self,
        fun: &CoreFun,
        result: Option<BasicValueEnum<'ctx>>,
    ) -> Result<()> {
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
}
