//! TCO / musttail emission (DESIGN §4.4).
//!
//! SCC analysis lives in [`lumia_core::compute_tco_sccs`].

use super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::BasicMetadataValueEnum;
use lumia_core::Local;

impl<'ctx> Codegen<'ctx> {
    /// Emit `musttail call` + `ret` for pure TCO (self or mutual; Int/Float i64 ABI).
    /// Returns true if the call was emitted as a terminator.
    pub(crate) fn emit_musttail_call(&mut self, fun: &str, args: &[Local]) -> Result<bool> {
        let callee = match self.funs.functions.get(fun).copied() {
            Some(f) => f,
            None => return Ok(false),
        };
        let mut av: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
        for a in args {
            av.push(self.coerce_i64(self.local(*a)?)?.into());
        }
        let call = crate::error::llvm(self.llvm.builder.build_call(callee, &av, "tco"))?;
        call.set_tail_call_kind(inkwell::values::LLVMTailCallKind::LLVMTailCallKindMustTail);
        let ret = call
            .try_as_basic_value()
            .basic()
            .with_context(|| {
                format!("ICE: musttail call to `{fun}` returned void; expected i64 ABI value")
            })?
            .into_int_value();
        debug_assert_eq!(self.frame.root_depth, 0);
        crate::error::llvm(self.llvm.builder.build_return(Some(&ret)))?;
        Ok(true)
    }
}

pub(crate) use lumia_core::compute_tco_sccs;
