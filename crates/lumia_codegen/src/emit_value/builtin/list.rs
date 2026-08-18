//! Value emission — list builtins (Custom shapes only; simple ones use BuiltinEmit).

use super::super::super::Codegen;
use anyhow::Result;
use inkwell::values::BasicValueEnum;
use lumia_core::{list_par_map_elem_ty, Local};
use lumia_hir::Builtin;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_list_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::ListParMap => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let fun_i = self.coerce_i64(self.local(args[1])?)?;
                let list = self.i64_as_ptr(list_i, "pmap_list")?;
                let fptr = self.ensure_funref_ptr(fun_i, "pmap")?;
                let elem_ty = list_par_map_elem_ty(args, self.infer_ctx());
                let elem_is_float = matches!(elem_ty, Type::Float);
                let elem_is_bool = matches!(elem_ty, Type::Bool);
                let result_tid = if elem_is_float || elem_is_bool {
                    lumia_abi::list_type_id_flags(elem_is_float, elem_is_bool)
                } else if matches!(elem_ty, Type::Int) {
                    lumia_abi::list_type_id_int()
                } else {
                    lumia_abi::list_type_id_flags(false, false)
                } as u64;
                let sym = Self::builtin_symbol(name)?;
                let tid_v = self.llvm.context.i32_type().const_int(result_tid, false);
                self.call_rt_ptr_as_i64(sym, &[list.into(), fptr.into(), tid_v.into()], "par_map")
            }
            Builtin::ListParFold => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let init_i = self.coerce_i64(self.local(args[1])?)?;
                let fun_i = self.coerce_i64(self.local(args[2])?)?;
                let list = self.i64_as_ptr(list_i, "pfold_list")?;
                let fptr = self.ensure_funref_ptr(fun_i, "pfold")?;
                let sym = Self::builtin_symbol(name)?;
                self.call_rt_basic(sym, &[list.into(), init_i.into(), fptr.into()], "par_fold")
            }
            _ => unreachable!(
                "non-custom list builtin `{}` should use BuiltinEmit",
                name.display_name()
            ),
        }
    }
}
