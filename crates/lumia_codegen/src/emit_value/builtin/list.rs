//! Value emission — list builtins.

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::BasicValueEnum;
use inkwell::{AddressSpace, IntPredicate};
use lumia_core::{Local, Value};
use lumia_hir::Builtin;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_list_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::ListLen => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let list = crate::error::llvm(
                    self.llvm
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "obj_ptr"),
                )?;
                let f = self.runtime_fn(
                    Builtin::ListLen
                        .info()
                        .runtime_symbol
                        .context("runtime symbol")?,
                )?;
                let call =
                    crate::error::llvm(self.llvm.builder.build_call(f, &[list.into()], "len"))?;
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?)
            }
            Builtin::ListGet => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let idx = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let list = crate::error::llvm(
                    self.llvm
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "col_ptr"),
                )?;
                let some = self
                    .llvm
                    .i64_ty
                    .const_int(self.option_some_tag as u64, true);
                let none = self
                    .llvm
                    .i64_ty
                    .const_int(self.option_none_tag as u64, true);
                let f = self.runtime_fn(
                    Builtin::ListGet
                        .info()
                        .runtime_symbol
                        .context("runtime symbol")?,
                )?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[list.into(), idx.into(), some.into(), none.into()],
                    "get",
                ))?;
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?)
            }
            Builtin::Elems => {
                let col_i = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let col = crate::error::llvm(
                    self.llvm.builder.build_int_to_ptr(col_i, ptr_ty, "col_ptr"),
                )?;
                let f = self.runtime_fn("lumia_elems")?;
                let call =
                    crate::error::llvm(self.llvm.builder.build_call(f, &[col.into()], "elems"))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "elems_i64",
                ))?
                .into())
            }
            Builtin::ListSlice => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let start = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let list = crate::error::llvm(
                    self.llvm
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "list_ptr"),
                )?;
                let f = self.runtime_fn("lumia_list_slice")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[list.into(), start.into()],
                    "slice",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "slice_i64",
                ))?
                .into())
            }
            Builtin::ListAppend => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let elem = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let mut list = crate::error::llvm(
                    self.llvm
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "list_ptr"),
                )?;
                if matches!(self.frame.local_tys.get(&args[1].0), Some(Type::Float)) {
                    let ens = self.runtime_fn(lumia_abi::ENSURE_LIST_F64)?;
                    list = crate::error::llvm(self.llvm.builder.build_call(
                        ens,
                        &[list.into()],
                        "ens_lf64",
                    ))?
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                }
                let f = self.runtime_fn("lumia_list_append")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[list.into(), elem.into()],
                    "append",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "append_i64",
                ))?
                .into())
            }
            Builtin::ListConcat => {
                let a_i = self.coerce_i64(self.local(args[0])?)?;
                let b_i = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let a = crate::error::llvm(
                    self.llvm.builder.build_int_to_ptr(a_i, ptr_ty, "concat_a"),
                )?;
                let b = crate::error::llvm(
                    self.llvm.builder.build_int_to_ptr(b_i, ptr_ty, "concat_b"),
                )?;
                let f = self.runtime_fn("lumia_concat")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[a.into(), b.into()],
                    "concat",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "concat_i64",
                ))?
                .into())
            }
            Builtin::ListTake => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let n = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let list =
                    crate::error::llvm(self.llvm.builder.build_int_to_ptr(list_i, ptr_ty, "list"))?;
                let f = self.runtime_fn("lumia_list_take")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[list.into(), n.into()],
                    "take",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "take_i64",
                ))?
                .into())
            }
            Builtin::ListReverse | Builtin::ListSort => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let list =
                    crate::error::llvm(self.llvm.builder.build_int_to_ptr(list_i, ptr_ty, "list"))?;
                let fname = match name {
                    Builtin::ListReverse => "lumia_list_reverse",
                    _ => "lumia_list_sort",
                };
                let f = self.runtime_fn(fname)?;
                let call =
                    crate::error::llvm(self.llvm.builder.build_call(f, &[list.into()], "list_op"))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "list_op_i64",
                ))?
                .into())
            }
            Builtin::ListSortByKeys => {
                let vals_i = self.coerce_i64(self.local(args[0])?)?;
                let keys_i = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let vals = crate::error::llvm(
                    self.llvm
                        .builder
                        .build_int_to_ptr(vals_i, ptr_ty, "sby_vals"),
                )?;
                let keys = crate::error::llvm(
                    self.llvm
                        .builder
                        .build_int_to_ptr(keys_i, ptr_ty, "sby_keys"),
                )?;
                let f = self.runtime_fn("lumia_list_sort_by_keys")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[vals.into(), keys.into()],
                    "sort_by",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "sort_by_i64",
                ))?
                .into())
            }
            Builtin::ListParMap => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let fun_i = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let list = crate::error::llvm(self.llvm.builder.build_int_to_ptr(
                    list_i,
                    ptr_ty,
                    "pmap_list",
                ))?;
                // FunRef is tagged with low bit; refuse heap closures.
                let one = self.llvm.i64_ty.const_int(1, false);
                let tagged =
                    crate::error::llvm(self.llvm.builder.build_and(fun_i, one, "pmap_tag"))?;
                let is_funref = crate::error::llvm(self.llvm.builder.build_int_compare(
                    IntPredicate::EQ,
                    tagged,
                    one,
                    "pmap_is_fr",
                ))?;
                let cur = self
                    .llvm
                    .builder
                    .get_insert_block()
                    .context("par_map needs insert block")?;
                let parent = cur.get_parent().context("bb parent")?;
                let ok_bb = self.llvm.context.append_basic_block(parent, "pmap_ok");
                let bad_bb = self.llvm.context.append_basic_block(parent, "pmap_bad");
                crate::error::llvm(
                    self.llvm
                        .builder
                        .build_conditional_branch(is_funref, ok_bb, bad_bb),
                )?;
                self.llvm.builder.position_at_end(bad_bb);
                let fail = self.runtime_fn("lumia_match_fail")?;
                crate::error::llvm(self.llvm.builder.build_call(fail, &[], "pmap_bad_fn"))?;
                crate::error::llvm(self.llvm.builder.build_unreachable())?;
                self.llvm.builder.position_at_end(ok_bb);
                let not1 = crate::error::llvm(self.llvm.builder.build_not(one, "not1"))?;
                let cleared =
                    crate::error::llvm(self.llvm.builder.build_and(fun_i, not1, "fun_clear"))?;
                let fptr = crate::error::llvm(
                    self.llvm
                        .builder
                        .build_int_to_ptr(cleared, ptr_ty, "pmap_fn"),
                )?;
                let elem_is_float = matches!(
                    lumia_core::infer_value_ty_ctx(
                        &Value::Builtin {
                            name: Builtin::ListParMap,
                            args: args.to_vec(),
                        },
                        lumia_core::InferValueCtx {
                            local_tys: &self.frame.local_tys,
                            slot_tys: Some(&self.frame.slot_tys),
                            fun_ret_tys: Some(&self.funs.fun_ret_tys),
                            fun_param_tys: Some(&self.funs.fun_param_tys),
                            fun_param0_identity: Some(&self.funs.fun_param0_identity),
                            funref_locals: Some(&self.funs.funref_locals),
                        },
                        None,
                    ),
                    Type::List(e) if matches!(e.as_ref(), Type::Float)
                );
                let result_tid = lumia_abi::list_type_id(elem_is_float) as u64;
                let f = self.runtime_fn("lumia_list_par_map")?;
                let tid_v = self.llvm.context.i32_type().const_int(result_tid, false);
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[list.into(), fptr.into(), tid_v.into()],
                    "par_map",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "par_map_i64",
                ))?
                .into())
            }
            Builtin::ListParFold => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let init_i = self.coerce_i64(self.local(args[1])?)?;
                let fun_i = self.coerce_i64(self.local(args[2])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let list = crate::error::llvm(self.llvm.builder.build_int_to_ptr(
                    list_i,
                    ptr_ty,
                    "pfold_list",
                ))?;
                let one = self.llvm.i64_ty.const_int(1, false);
                let tagged =
                    crate::error::llvm(self.llvm.builder.build_and(fun_i, one, "pfold_tag"))?;
                let is_funref = crate::error::llvm(self.llvm.builder.build_int_compare(
                    IntPredicate::EQ,
                    tagged,
                    one,
                    "pfold_is_fr",
                ))?;
                let cur = self
                    .llvm
                    .builder
                    .get_insert_block()
                    .context("par_fold needs insert block")?;
                let parent = cur.get_parent().context("bb parent")?;
                let ok_bb = self.llvm.context.append_basic_block(parent, "pfold_ok");
                let bad_bb = self.llvm.context.append_basic_block(parent, "pfold_bad");
                crate::error::llvm(
                    self.llvm
                        .builder
                        .build_conditional_branch(is_funref, ok_bb, bad_bb),
                )?;
                self.llvm.builder.position_at_end(bad_bb);
                let fail = self.runtime_fn("lumia_match_fail")?;
                crate::error::llvm(self.llvm.builder.build_call(fail, &[], "pfold_bad_fn"))?;
                crate::error::llvm(self.llvm.builder.build_unreachable())?;
                self.llvm.builder.position_at_end(ok_bb);
                let not1 = crate::error::llvm(self.llvm.builder.build_not(one, "pfold_not1"))?;
                let cleared =
                    crate::error::llvm(self.llvm.builder.build_and(fun_i, not1, "pfold_clear"))?;
                let fptr = crate::error::llvm(
                    self.llvm
                        .builder
                        .build_int_to_ptr(cleared, ptr_ty, "pfold_fn"),
                )?;
                let f = self.runtime_fn("lumia_list_par_fold")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[list.into(), init_i.into(), fptr.into()],
                    "par_fold",
                ))?;
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_int_value()
                    .into())
            }
            Builtin::ListJoin => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let sep_i = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                let list =
                    crate::error::llvm(self.llvm.builder.build_int_to_ptr(list_i, ptr_ty, "list"))?;
                let sep =
                    crate::error::llvm(self.llvm.builder.build_int_to_ptr(sep_i, ptr_ty, "sep"))?;
                let f = self.runtime_fn("lumia_list_join")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[list.into(), sep.into()],
                    "join",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "join_i64",
                ))?
                .into())
            }
            Builtin::Range => {
                let a = self.coerce_i64(self.local(args[0])?)?;
                let b = self.coerce_i64(self.local(args[1])?)?;
                let f = self.runtime_fn("lumia_range")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[a.into(), b.into()],
                    "range",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "range_i64",
                ))?
                .into())
            }
            Builtin::RangeInclusive => {
                let a = self.coerce_i64(self.local(args[0])?)?;
                let b = self.coerce_i64(self.local(args[1])?)?;
                let f = self.runtime_fn("lumia_range_inclusive")?;
                let call = crate::error::llvm(self.llvm.builder.build_call(
                    f,
                    &[a.into(), b.into()],
                    "range_inc",
                ))?;
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "range_i64",
                ))?
                .into())
            }
            _ => unreachable!("non-list builtin in emit_list_builtin"),
        }
    }
}
