//! Value emission — builtin intrinsics

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::BasicValueEnum;
use inkwell::{AddressSpace, IntPredicate};
use lumia_core::{Local, Value};
use lumia_hir::Builtin;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_value_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        match name {
            Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr => {
                let arg = self.local(args[0])?;
                let arg_ty = self.local_tys.get(&args[0].0).cloned().unwrap_or(Type::Int);
                match arg_ty {
                    Type::Float => {
                        let f = match arg {
                            BasicValueEnum::FloatValue(f) => f,
                            other => self.promote_f64(other)?,
                        };
                        self.call_rt_void("lumia_println_float", &[f.into()], "println_float")?;
                    }
                    Type::Bool => {
                        let i = self.coerce_i64(arg)?;
                        let b = self
                            .builder
                            .build_int_truncate(i, self.context.i8_type(), "bool8")
                            .map_err(|e| anyhow::anyhow!("truncate bool8: {e}"))?;
                        self.call_rt_void("lumia_println_bool", &[b.into()], "println_bool")?;
                    }
                    Type::Adt { name, params } => {
                        let ptr = if let Some(ptr) = self.emit_show_override(&name, arg)? {
                            Some(ptr)
                        } else if params.iter().any(|p| matches!(p, Type::Float | Type::Bool)) {
                            Some(self.emit_typed_adt_show(arg, &params)?)
                        } else {
                            None
                        };
                        if let Some(ptr) = ptr {
                            let len = self
                                .call_rt_basic("lumia_str_len", &[ptr.into()], "show_len")?
                                .into_int_value();
                            self.call_rt_void(
                                "lumia_println_str",
                                &[ptr.into(), len.into()],
                                "println_show",
                            )?;
                        } else {
                            let i = self.coerce_i64(arg)?;
                            self.call_rt_void("lumia_println_auto", &[i.into()], "println")?;
                        }
                    }
                    _ => {
                        let i = self.coerce_i64(arg)?;
                        self.call_rt_void("lumia_println_auto", &[i.into()], "println")?;
                    }
                }
                Ok(self.i64_ty.const_int(0, false).into())
            }
            Builtin::ListLen => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let list = self
                    .builder
                    .build_int_to_ptr(list_i, ptr_ty, "obj_ptr")
                    .unwrap();
                let f = self.module.get_function("lumia_len").unwrap();
                let call = self.builder.build_call(f, &[list.into()], "len").unwrap();
                Ok(call.try_as_basic_value().basic().unwrap())
            }
            Builtin::ListGet => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let idx = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let list = self
                    .builder
                    .build_int_to_ptr(list_i, ptr_ty, "col_ptr")
                    .unwrap();
                let some = self.i64_ty.const_int(self.option_some_tag as u64, true);
                let none = self.i64_ty.const_int(self.option_none_tag as u64, true);
                let f = self.module.get_function("lumia_get").unwrap();
                let call = self
                    .builder
                    .build_call(
                        f,
                        &[list.into(), idx.into(), some.into(), none.into()],
                        "get",
                    )
                    .unwrap();
                Ok(call.try_as_basic_value().basic().unwrap())
            }
            Builtin::Contains => {
                let obj_i = self.coerce_i64(self.local(args[0])?)?;
                let key = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let obj = self
                    .builder
                    .build_int_to_ptr(obj_i, ptr_ty, "col_ptr")
                    .unwrap();
                let f = self.module.get_function("lumia_contains").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[obj.into(), key.into()], "contains")
                    .unwrap();
                Ok(call.try_as_basic_value().basic().unwrap())
            }
            Builtin::MapSet => {
                let map_i = self.coerce_i64(self.local(args[0])?)?;
                let key = self.coerce_i64(self.local(args[1])?)?;
                let val = self.coerce_i64(self.local(args[2])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let mut map = self
                    .builder
                    .build_int_to_ptr(map_i, ptr_ty, "col_ptr")
                    .unwrap();
                if matches!(self.local_tys.get(&args[1].0), Some(Type::Float)) {
                    let ens = self.module.get_function("lumia_ensure_map_f64").unwrap();
                    map = self
                        .builder
                        .build_call(ens, &[map.into()], "ens_mf64")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                }
                if matches!(self.local_tys.get(&args[2].0), Some(Type::Float)) {
                    let ens = self.module.get_function("lumia_ensure_map_vf64").unwrap();
                    map = self
                        .builder
                        .build_call(ens, &[map.into()], "ens_mvf64")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                }
                let f = self.module.get_function("lumia_set").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[map.into(), key.into(), val.into()], "col_set")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "set_i64")
                    .unwrap()
                    .into())
            }
            Builtin::MapRemove => {
                let map_i = self.coerce_i64(self.local(args[0])?)?;
                let key = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let map = self
                    .builder
                    .build_int_to_ptr(map_i, ptr_ty, "col_ptr")
                    .unwrap();
                let f = self.module.get_function("lumia_remove").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[map.into(), key.into()], "col_rm")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "rm_i64")
                    .unwrap()
                    .into())
            }
            Builtin::SetInsert => {
                let set_i = self.coerce_i64(self.local(args[0])?)?;
                let elem = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let mut set = self
                    .builder
                    .build_int_to_ptr(set_i, ptr_ty, "set_ptr")
                    .unwrap();
                if matches!(self.local_tys.get(&args[1].0), Some(Type::Float)) {
                    let ens = self.module.get_function("lumia_ensure_set_f64").unwrap();
                    set = self
                        .builder
                        .build_call(ens, &[set.into()], "ens_sf64")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                }
                let f = self.module.get_function("lumia_set_insert").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[set.into(), elem.into()], "set_ins")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "set_ins_i64")
                    .unwrap()
                    .into())
            }
            Builtin::MapKeys | Builtin::MapValues | Builtin::MapItems | Builtin::Elems => {
                let map_i = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let map = self
                    .builder
                    .build_int_to_ptr(map_i, ptr_ty, "map_ptr")
                    .unwrap();
                let fname = match name {
                    Builtin::MapKeys => "lumia_map_keys",
                    Builtin::MapValues => "lumia_map_values",
                    Builtin::Elems => "lumia_elems",
                    _ => "lumia_map_items",
                };
                let f = self.module.get_function(fname).unwrap();
                let call = self.builder.build_call(f, &[map.into()], "map_kv").unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "map_kv_i64")
                    .unwrap()
                    .into())
            }
            Builtin::AdtTag => {
                let obj_i = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let obj = self
                    .builder
                    .build_int_to_ptr(obj_i, ptr_ty, "adt_ptr")
                    .unwrap();
                let f = self.module.get_function("lumia_adt_tag").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[obj.into()], "adt_tag")
                    .unwrap();
                Ok(call.try_as_basic_value().basic().unwrap())
            }
            Builtin::AdtField => {
                let obj_i = self.coerce_i64(self.local(args[0])?)?;
                let idx = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let obj = self
                    .builder
                    .build_int_to_ptr(obj_i, ptr_ty, "adt_ptr")
                    .unwrap();
                let f = self.module.get_function("lumia_adt_field").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[obj.into(), idx.into()], "adt_field")
                    .unwrap();
                Ok(call.try_as_basic_value().basic().unwrap())
            }
            Builtin::ListSlice => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let start = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let list = self
                    .builder
                    .build_int_to_ptr(list_i, ptr_ty, "list_ptr")
                    .unwrap();
                let f = self.module.get_function("lumia_list_slice").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[list.into(), start.into()], "slice")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "slice_i64")
                    .unwrap()
                    .into())
            }
            Builtin::ListAppend => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let elem = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let mut list = self
                    .builder
                    .build_int_to_ptr(list_i, ptr_ty, "list_ptr")
                    .unwrap();
                if matches!(self.local_tys.get(&args[1].0), Some(Type::Float)) {
                    let ens = self.module.get_function("lumia_ensure_list_f64").unwrap();
                    list = self
                        .builder
                        .build_call(ens, &[list.into()], "ens_lf64")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                }
                let f = self.module.get_function("lumia_list_append").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[list.into(), elem.into()], "append")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "append_i64")
                    .unwrap()
                    .into())
            }
            Builtin::ListConcat => {
                let a_i = self.coerce_i64(self.local(args[0])?)?;
                let b_i = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let a = self
                    .builder
                    .build_int_to_ptr(a_i, ptr_ty, "concat_a")
                    .unwrap();
                let b = self
                    .builder
                    .build_int_to_ptr(b_i, ptr_ty, "concat_b")
                    .unwrap();
                let f = self.module.get_function("lumia_concat").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[a.into(), b.into()], "concat")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "concat_i64")
                    .unwrap()
                    .into())
            }
            Builtin::Show => {
                let arg = self.local(args[0])?;
                let arg_ty = self.local_tys.get(&args[0].0).cloned().unwrap_or(Type::Int);
                let ptr = match arg_ty {
                    Type::Float => {
                        let f = match arg {
                            BasicValueEnum::FloatValue(f) => f,
                            other => self.promote_f64(other)?,
                        };
                        let fun = self.module.get_function("lumia_show_float").unwrap();
                        self.builder
                            .build_call(fun, &[f.into()], "show_float")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_pointer_value()
                    }
                    Type::Bool => {
                        let i = self.coerce_i64(arg)?;
                        let b = self
                            .builder
                            .build_int_truncate(i, self.context.i8_type(), "bool8")
                            .unwrap();
                        let fun = self.module.get_function("lumia_show_bool").unwrap();
                        self.builder
                            .build_call(fun, &[b.into()], "show_bool")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_pointer_value()
                    }
                    Type::Adt { name, params } => {
                        if let Some(ptr) = self.emit_show_override(&name, arg)? {
                            ptr
                        } else if params.iter().any(|p| matches!(p, Type::Float | Type::Bool)) {
                            self.emit_typed_adt_show(arg, &params)?
                        } else {
                            let i = self.coerce_i64(arg)?;
                            let fun = self.module.get_function("lumia_show").unwrap();
                            self.builder
                                .build_call(fun, &[i.into()], "show")
                                .unwrap()
                                .try_as_basic_value()
                                .basic()
                                .unwrap()
                                .into_pointer_value()
                        }
                    }
                    _ => {
                        let i = self.coerce_i64(arg)?;
                        let fun = self.module.get_function("lumia_show").unwrap();
                        self.builder
                            .build_call(fun, &[i.into()], "show")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_pointer_value()
                    }
                };
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "show_i64")
                    .unwrap()
                    .into())
            }
            Builtin::StrTrim | Builtin::StrToLower | Builtin::StrToUpper => {
                let s_i = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let s = self.builder.build_int_to_ptr(s_i, ptr_ty, "str").unwrap();
                let fname = match name {
                    Builtin::StrTrim => "lumia_str_trim",
                    Builtin::StrToLower => "lumia_str_to_lower",
                    _ => "lumia_str_to_upper",
                };
                let f = self.module.get_function(fname).unwrap();
                let call = self.builder.build_call(f, &[s.into()], "str_op").unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "str_i64")
                    .unwrap()
                    .into())
            }
            Builtin::StrSplit => {
                let s_i = self.coerce_i64(self.local(args[0])?)?;
                let sep = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let s = self.builder.build_int_to_ptr(s_i, ptr_ty, "str").unwrap();
                let f = self.module.get_function("lumia_str_split").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[s.into(), sep.into()], "split")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "split_i64")
                    .unwrap()
                    .into())
            }
            Builtin::StrSubstring => {
                let s_i = self.coerce_i64(self.local(args[0])?)?;
                let a = self.coerce_i64(self.local(args[1])?)?;
                let b = self.coerce_i64(self.local(args[2])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let s = self.builder.build_int_to_ptr(s_i, ptr_ty, "str").unwrap();
                let f = self.module.get_function("lumia_str_substring").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[s.into(), a.into(), b.into()], "substr")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "substr_i64")
                    .unwrap()
                    .into())
            }
            Builtin::ListTake => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let n = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let list = self
                    .builder
                    .build_int_to_ptr(list_i, ptr_ty, "list")
                    .unwrap();
                let f = self.module.get_function("lumia_list_take").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[list.into(), n.into()], "take")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "take_i64")
                    .unwrap()
                    .into())
            }
            Builtin::ListReverse | Builtin::ListSort => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let list = self
                    .builder
                    .build_int_to_ptr(list_i, ptr_ty, "list")
                    .unwrap();
                let fname = match name {
                    Builtin::ListReverse => "lumia_list_reverse",
                    _ => "lumia_list_sort",
                };
                let f = self.module.get_function(fname).unwrap();
                let call = self
                    .builder
                    .build_call(f, &[list.into()], "list_op")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "list_op_i64")
                    .unwrap()
                    .into())
            }
            Builtin::ListSortByKeys => {
                let vals_i = self.coerce_i64(self.local(args[0])?)?;
                let keys_i = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let vals = self
                    .builder
                    .build_int_to_ptr(vals_i, ptr_ty, "sby_vals")
                    .unwrap();
                let keys = self
                    .builder
                    .build_int_to_ptr(keys_i, ptr_ty, "sby_keys")
                    .unwrap();
                let f = self.module.get_function("lumia_list_sort_by_keys").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[vals.into(), keys.into()], "sort_by")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "sort_by_i64")
                    .unwrap()
                    .into())
            }
            Builtin::ListParMap => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let fun_i = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let list = self
                    .builder
                    .build_int_to_ptr(list_i, ptr_ty, "pmap_list")
                    .unwrap();
                // FunRef is tagged with low bit; refuse heap closures.
                let one = self.i64_ty.const_int(1, false);
                let tagged = self.builder.build_and(fun_i, one, "pmap_tag").unwrap();
                let is_funref = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, tagged, one, "pmap_is_fr")
                    .unwrap();
                let cur = self
                    .builder
                    .get_insert_block()
                    .context("par_map needs insert block")?;
                let parent = cur.get_parent().context("bb parent")?;
                let ok_bb = self.context.append_basic_block(parent, "pmap_ok");
                let bad_bb = self.context.append_basic_block(parent, "pmap_bad");
                self.builder
                    .build_conditional_branch(is_funref, ok_bb, bad_bb)
                    .unwrap();
                self.builder.position_at_end(bad_bb);
                let fail = self.module.get_function("lumia_match_fail").unwrap();
                self.builder.build_call(fail, &[], "pmap_bad_fn").unwrap();
                self.builder.build_unreachable().unwrap();
                self.builder.position_at_end(ok_bb);
                let cleared = self
                    .builder
                    .build_and(
                        fun_i,
                        self.builder.build_not(one, "not1").unwrap(),
                        "fun_clear",
                    )
                    .unwrap();
                let fptr = self
                    .builder
                    .build_int_to_ptr(cleared, ptr_ty, "pmap_fn")
                    .unwrap();
                let elem_is_float = matches!(
                    lumia_core::infer_value_ty_ctx(
                        &Value::Builtin {
                            name: Builtin::ListParMap,
                            args: args.to_vec(),
                        },
                        lumia_core::InferValueCtx {
                            local_tys: &self.local_tys,
                            slot_tys: Some(&self.slot_tys),
                            fun_ret_tys: Some(&self.fun_ret_tys),
                            fun_param_tys: Some(&self.fun_param_tys),
                            fun_param0_identity: Some(&self.fun_param0_identity),
                            funref_locals: Some(&self.funref_locals),
                        },
                        None,
                    ),
                    Type::List(e) if matches!(e.as_ref(), Type::Float)
                );
                let result_tid = lumia_abi::list_type_id(elem_is_float) as u64;
                let f = self.module.get_function("lumia_list_par_map").unwrap();
                let tid_v = self.context.i32_type().const_int(result_tid, false);
                let call = self
                    .builder
                    .build_call(f, &[list.into(), fptr.into(), tid_v.into()], "par_map")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "par_map_i64")
                    .unwrap()
                    .into())
            }
            Builtin::ListParFold => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let init_i = self.coerce_i64(self.local(args[1])?)?;
                let fun_i = self.coerce_i64(self.local(args[2])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let list = self
                    .builder
                    .build_int_to_ptr(list_i, ptr_ty, "pfold_list")
                    .unwrap();
                let one = self.i64_ty.const_int(1, false);
                let tagged = self.builder.build_and(fun_i, one, "pfold_tag").unwrap();
                let is_funref = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, tagged, one, "pfold_is_fr")
                    .unwrap();
                let cur = self
                    .builder
                    .get_insert_block()
                    .context("par_fold needs insert block")?;
                let parent = cur.get_parent().context("bb parent")?;
                let ok_bb = self.context.append_basic_block(parent, "pfold_ok");
                let bad_bb = self.context.append_basic_block(parent, "pfold_bad");
                self.builder
                    .build_conditional_branch(is_funref, ok_bb, bad_bb)
                    .unwrap();
                self.builder.position_at_end(bad_bb);
                let fail = self.module.get_function("lumia_match_fail").unwrap();
                self.builder.build_call(fail, &[], "pfold_bad_fn").unwrap();
                self.builder.build_unreachable().unwrap();
                self.builder.position_at_end(ok_bb);
                let cleared = self
                    .builder
                    .build_and(
                        fun_i,
                        self.builder.build_not(one, "pfold_not1").unwrap(),
                        "pfold_clear",
                    )
                    .unwrap();
                let fptr = self
                    .builder
                    .build_int_to_ptr(cleared, ptr_ty, "pfold_fn")
                    .unwrap();
                let f = self.module.get_function("lumia_list_par_fold").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[list.into(), init_i.into(), fptr.into()], "par_fold")
                    .unwrap();
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_int_value()
                    .into())
            }
            Builtin::ListJoin => {
                let list_i = self.coerce_i64(self.local(args[0])?)?;
                let sep_i = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let list = self
                    .builder
                    .build_int_to_ptr(list_i, ptr_ty, "list")
                    .unwrap();
                let sep = self.builder.build_int_to_ptr(sep_i, ptr_ty, "sep").unwrap();
                let f = self.module.get_function("lumia_list_join").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[list.into(), sep.into()], "join")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "join_i64")
                    .unwrap()
                    .into())
            }
            Builtin::ReadStdin => {
                let f = self.module.get_function("lumia_read_stdin").unwrap();
                let call = self.builder.build_call(f, &[], "stdin").unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "stdin_i64")
                    .unwrap()
                    .into())
            }
            Builtin::MatchFail => {
                let f = self.module.get_function("lumia_match_fail").unwrap();
                self.builder.build_call(f, &[], "match_fail").unwrap();
                // Unreachable in practice; keep SSA well-typed.
                Ok(self.i64_ty.const_int(0, false).into())
            }
            Builtin::Assert => {
                let cond = self.coerce_i64(self.local(args[0])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let (msg_ptr, msg_len) = if args.len() >= 2 {
                    let msg_i = self.coerce_i64(self.local(args[1])?)?;
                    let msg_ptr = self
                        .builder
                        .build_int_to_ptr(msg_i, ptr_ty, "assert_msg")
                        .unwrap();
                    let len_f = self.module.get_function("lumia_str_len").unwrap();
                    let len = self
                        .builder
                        .build_call(len_f, &[msg_ptr.into()], "assert_len")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap();
                    (msg_ptr, len)
                } else {
                    (ptr_ty.const_null(), self.i64_ty.const_int(0, false).into())
                };
                let f = self.module.get_function("lumia_assert").unwrap();
                self.builder
                    .build_call(f, &[cond.into(), msg_ptr.into(), msg_len.into()], "assert")
                    .unwrap();
                Ok(self.i64_ty.const_int(0, false).into())
            }
            Builtin::StrStartsWith | Builtin::StrEndsWith => {
                let a_i = self.coerce_i64(self.local(args[0])?)?;
                let b_i = self.coerce_i64(self.local(args[1])?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let a = self.builder.build_int_to_ptr(a_i, ptr_ty, "a").unwrap();
                let b = self.builder.build_int_to_ptr(b_i, ptr_ty, "b").unwrap();
                let fname = match name {
                    Builtin::StrStartsWith => "lumia_str_starts_with",
                    _ => "lumia_str_ends_with",
                };
                let f = self.module.get_function(fname).unwrap();
                let call = self
                    .builder
                    .build_call(f, &[a.into(), b.into()], "str_affix")
                    .unwrap();
                Ok(call.try_as_basic_value().basic().unwrap())
            }
            Builtin::Range => {
                let a = self.coerce_i64(self.local(args[0])?)?;
                let b = self.coerce_i64(self.local(args[1])?)?;
                let f = self.module.get_function("lumia_range").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[a.into(), b.into()], "range")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "range_i64")
                    .unwrap()
                    .into())
            }
            Builtin::RangeInclusive => {
                let a = self.coerce_i64(self.local(args[0])?)?;
                let b = self.coerce_i64(self.local(args[1])?)?;
                let f = self.module.get_function("lumia_range_inclusive").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[a.into(), b.into()], "range_inc")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "range_i64")
                    .unwrap()
                    .into())
            }
        }
    }
}
