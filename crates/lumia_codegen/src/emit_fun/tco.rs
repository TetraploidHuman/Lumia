//! Musttail self/mutual recursion (DESIGN §4.4).

use super::super::Codegen;
use anyhow::Result;
use lumia_core::{Block, Local, Op, Value};

impl<'ctx> Codegen<'ctx> {
    /// Pure self/mutual recursion in tail position → musttail (DESIGN §4.4).
    /// Returns `Ok(true)` if the block was terminated by a musttail call.
    pub(super) fn try_emit_tco_let(&mut self, block: &Block, local: Local, value: &Value) -> Result<bool> {
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
                    // Only drop roots/frame once the callee is known to exist;
                    // otherwise fall through to a normal call with roots intact.
                    if !self.funs.functions.contains_key(fun) {
                        return Ok(false);
                    }
                    self.root_pop_to(0)?;
                    self.emit_frame_pop()?;
                    let ok = self.emit_musttail_call(fun, args)?;
                    debug_assert!(ok, "musttail callee was declared");
                    return Ok(ok);
                }
            }
            Value::IndirectCall { callee, args } => {
                if let Some(fun) = self.funs.funref_locals.get(&callee.0).cloned() {
                    if self.funs.tco_peers.contains(&fun) {
                        if !self.funs.functions.contains_key(&fun) {
                            return Ok(false);
                        }
                        self.root_pop_to(0)?;
                        self.emit_frame_pop()?;
                        let ok = self.emit_musttail_call(&fun, args)?;
                        debug_assert!(ok, "musttail callee was declared");
                        return Ok(ok);
                    }
                }
            }
            _ => {}
        }
        Ok(false)
    }

}
