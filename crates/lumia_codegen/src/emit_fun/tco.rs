//! Musttail self/mutual recursion (DESIGN §4.4).

use super::super::Codegen;
use anyhow::Result;
use lumia_core::{resolve_tco_tail_call, Block, Local, Op, Value};

impl<'ctx> Codegen<'ctx> {
    /// Emit musttail when `value` is a direct or alias-resolved recursive tail call.
    fn try_emit_tco_tail(&mut self, block: &Block, value: &Value) -> Result<bool> {
        if self.funs.tco_peers.is_empty() {
            return Ok(false);
        }
        let Some(tail) =
            resolve_tco_tail_call(block, value, &self.funs.tco_peers, &self.funs.funref)
        else {
            return Ok(false);
        };
        if !self.funs.functions.contains_key(&tail.fun) {
            return Ok(false);
        }
        self.root_pop_to(0)?;
        self.emit_frame_pop()?;
        let ok = self.emit_musttail_call(tail.fun.as_str(), &tail.args)?;
        debug_assert!(ok, "musttail callee was declared");
        Ok(ok)
    }

    /// Pure self/mutual recursion in tail position → musttail (DESIGN §4.4).
    /// Returns `Ok(true)` if the block was terminated by a musttail call.
    pub(super) fn try_emit_tco_let(
        &mut self,
        block: &Block,
        local: Local,
        value: &Value,
    ) -> Result<bool> {
        let is_block_tail = block.result == Some(local)
            && matches!(
                block.ops.last(),
                Some(Op::Let { local: last, .. }) if *last == local
            );
        if !is_block_tail {
            return Ok(false);
        }
        self.try_emit_tco_tail(block, value)
    }

    /// `Op::Return` carrying a recursive tail call (explicit early return in source).
    pub(super) fn try_emit_tco_return(&mut self, block: &Block, value: Local) -> Result<bool> {
        self.try_emit_tco_tail(block, &Value::Local(value))
    }
}
