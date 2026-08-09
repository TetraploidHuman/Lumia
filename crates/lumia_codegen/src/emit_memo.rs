//! Memo L2 / dense-index prologue and store.

use super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::basic_block::BasicBlock;
use inkwell::values::{FunctionValue, IntValue};
use inkwell::IntPredicate;
use lumia_core::CoreFun;

impl<'ctx> Codegen<'ctx> {
    fn memo_arg_values(&self) -> [IntValue<'ctx>; 4] {
        let z = self.i64_ty.const_int(0, false);
        let mut out = [z; 4];
        for (i, slot) in self.memo_arg_slots.iter().enumerate().take(4) {
            out[i] = self
                .builder
                .build_load(self.i64_ty, *slot, &format!("memo_a{i}"))
                .unwrap()
                .into_int_value();
        }
        out
    }

    /// On hit: branch to return cached. On miss: fall through to `compute` BB.
    /// Captures parameters into allocas so store uses entry-time keys.
    pub(crate) fn emit_memo_l2_prologue(
        &mut self,
        fun: &CoreFun,
        fv: FunctionValue<'ctx>,
        mid: u32,
    ) -> Result<BasicBlock<'ctx>> {
        let out_alloca = self.builder.build_alloca(self.i64_ty, "memo_out").unwrap();
        self.memo_arg_slots.clear();
        for (i, p) in fun.params.iter().enumerate().take(4) {
            let slot = self
                .builder
                .build_alloca(self.i64_ty, &format!("memo_arg{i}"))
                .unwrap();
            let v = self.coerce_i64(self.local(*p)?)?;
            self.builder.build_store(slot, v).unwrap();
            self.memo_arg_slots.push(slot);
        }
        let nargs = self.i64_ty.const_int(fun.params.len().min(4) as u64, false);
        let args = self.memo_arg_values();
        let lookup = self.module.get_function("lumia_memo_l2_lookup").unwrap();
        let hit = self
            .builder
            .build_call(
                lookup,
                &[
                    self.i64_ty.const_int(mid as u64, false).into(),
                    nargs.into(),
                    args[0].into(),
                    args[1].into(),
                    args[2].into(),
                    args[3].into(),
                    out_alloca.into(),
                ],
                "memo_hit",
            )
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_int_value();
        let is_hit = self
            .builder
            .build_int_compare(
                IntPredicate::NE,
                hit,
                self.i64_ty.const_int(0, false),
                "memo_is_hit",
            )
            .unwrap();
        let hit_bb = self.context.append_basic_block(fv, "memo_hit_ret");
        let compute_bb = self.context.append_basic_block(fv, "memo_compute");
        self.builder
            .build_conditional_branch(is_hit, hit_bb, compute_bb)
            .unwrap();

        self.builder.position_at_end(hit_bb);
        let cached = self
            .builder
            .build_load(self.i64_ty, out_alloca, "memo_cached")
            .unwrap()
            .into_int_value();
        self.emit_return_i64(cached);
        Ok(compute_bb)
    }

    pub(crate) fn emit_memo_idx_prologue(
        &mut self,
        fun: &CoreFun,
        fv: FunctionValue<'ctx>,
        mid: u32,
    ) -> Result<BasicBlock<'ctx>> {
        let p0 = fun
            .params
            .first()
            .copied()
            .context("memo_index requires one param")?;
        let key = self.coerce_i64(self.local(p0)?)?;
        let key_slot = self
            .builder
            .build_alloca(self.i64_ty, "memo_idx_key")
            .unwrap();
        self.builder.build_store(key_slot, key).unwrap();
        self.memo_idx_key = Some(key_slot);

        let out_alloca = self
            .builder
            .build_alloca(self.i64_ty, "memo_idx_out")
            .unwrap();
        let lookup = self.module.get_function("lumia_memo_idx_lookup").unwrap();
        let hit = self
            .builder
            .build_call(
                lookup,
                &[
                    self.i64_ty.const_int(mid as u64, false).into(),
                    key.into(),
                    out_alloca.into(),
                ],
                "memo_idx_hit",
            )
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_int_value();
        let is_hit = self
            .builder
            .build_int_compare(
                IntPredicate::NE,
                hit,
                self.i64_ty.const_int(0, false),
                "memo_idx_is_hit",
            )
            .unwrap();
        let hit_bb = self.context.append_basic_block(fv, "memo_idx_hit_ret");
        let compute_bb = self.context.append_basic_block(fv, "memo_idx_compute");
        self.builder
            .build_conditional_branch(is_hit, hit_bb, compute_bb)
            .unwrap();

        self.builder.position_at_end(hit_bb);
        let cached = self
            .builder
            .build_load(self.i64_ty, out_alloca, "memo_idx_cached")
            .unwrap()
            .into_int_value();
        self.emit_return_i64(cached);
        Ok(compute_bb)
    }

    pub(crate) fn emit_memo_idx_store(&mut self, mid: u32, result: IntValue<'ctx>) -> Result<()> {
        let key_slot = self
            .memo_idx_key
            .context("memo_idx store without key slot")?;
        let key = self
            .builder
            .build_load(self.i64_ty, key_slot, "memo_idx_key_ld")
            .unwrap()
            .into_int_value();
        let store = self.module.get_function("lumia_memo_idx_store").unwrap();
        self.builder
            .build_call(
                store,
                &[
                    self.i64_ty.const_int(mid as u64, false).into(),
                    key.into(),
                    result.into(),
                ],
                "",
            )
            .unwrap();
        Ok(())
    }

    pub(crate) fn emit_memo_l2_store(&mut self, mid: u32, result: IntValue<'ctx>) -> Result<()> {
        let nargs = self
            .i64_ty
            .const_int(self.memo_arg_slots.len() as u64, false);
        let args = self.memo_arg_values();
        let store = self.module.get_function("lumia_memo_l2_store").unwrap();
        self.builder
            .build_call(
                store,
                &[
                    self.i64_ty.const_int(mid as u64, false).into(),
                    nargs.into(),
                    args[0].into(),
                    args[1].into(),
                    args[2].into(),
                    args[3].into(),
                    result.into(),
                ],
                "",
            )
            .unwrap();
        Ok(())
    }
}
