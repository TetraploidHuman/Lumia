//! Value emission — Task / Channel builtins (Custom shapes).

#[path = "task_option.rs"]
mod task_option;
#[path = "task_spawn.rs"]
mod task_spawn;

use super::super::super::Codegen;
use anyhow::Result;
use inkwell::values::BasicValueEnum;
use lumia_core::Local;
use lumia_hir::Builtin;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_task_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::ChannelNew => {
                let cap = self.coerce_i64(self.local(args[0])?)?;
                self.call_rt_ptr_as_i64("lumia_channel_new", &[cap.into()], "channel")
            }
            Builtin::ChannelSend => {
                let ch_i = self.coerce_i64(self.local(args[0])?)?;
                let v = self.coerce_i64(self.local(args[1])?)?;
                let ch = self.i64_as_ptr(ch_i, "ch")?;
                self.call_rt_void("lumia_channel_send", &[ch.into(), v.into()], "send")?;
                Ok(self.llvm.i64_ty.const_int(0, false).into())
            }
            Builtin::ChannelClose => {
                let ch_i = self.coerce_i64(self.local(args[0])?)?;
                let ch = self.i64_as_ptr(ch_i, "ch")?;
                self.call_rt_void("lumia_channel_close", &[ch.into()], "close")?;
                Ok(self.llvm.i64_ty.const_int(0, false).into())
            }
            Builtin::ScopeEnter => {
                let kind = self.coerce_i64(self.local(args[0])?)?;
                self.call_rt_void("lumia_scope_enter", &[kind.into()], "scope_enter")?;
                Ok(self.llvm.i64_ty.const_int(0, false).into())
            }
            Builtin::ChannelRecvOpt => self.emit_channel_recv_opt(args),
            Builtin::TaskJoinOpt => self.emit_task_join_opt(args),
            Builtin::TaskSpawn => self.emit_task_spawn(args),
            _ => unreachable!(
                "non-custom task builtin `{}` should use BuiltinEmit",
                name.display_name()
            ),
        }
    }
}
