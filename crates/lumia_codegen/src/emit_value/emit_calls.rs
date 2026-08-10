//! Value emission — calls, funrefs, and closures

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::types::BasicMetadataTypeEnum;
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum};
use inkwell::{AddressSpace, IntPredicate};
use lumia_abi::TYPE_CLOSURE;
use lumia_core::Local;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_value_call(
        &mut self,
        fun: &str,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        let callee = self
            .funs
            .functions
            .get(fun)
            .copied()
            .with_context(|| format!("unknown function {fun}"))?;
        let is_ext = self.funs.external_funs.contains(fun);
        let param_tys = self
            .funs
            .fun_param_tys
            .get(fun)
            .cloned()
            .unwrap_or_default();
        // Temporary `lumia_string_cstr` buffers are unmarked heap objects;
        // root them until after the foreign call so a later arg alloc / GC
        // cannot collect an earlier cstr (UAF).
        let cstr_root_depth = self.frame.root_depth;
        let mut av: Vec<BasicMetadataValueEnum> = vec![];
        for (i, a) in args.iter().enumerate() {
            let pty = param_tys.get(i).unwrap_or(&Type::Int);
            if is_ext {
                if matches!(pty, Type::String) {
                    let s_i = self.coerce_i64(self.local(*a)?)?;
                    let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
                    let s = self
                        .llvm
                        .builder
                        .build_int_to_ptr(s_i, ptr_ty, "cstr_in")
                        .unwrap();
                    let f = self.runtime_fn("lumia_string_cstr")?;
                    let call =
                        crate::error::llvm(self.llvm.builder.build_call(f, &[s.into()], "cstr"))?;
                    let cstr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    let bits = self
                        .llvm
                        .builder
                        .build_ptr_to_int(cstr, self.llvm.i64_ty, "cstr_bits")
                        .unwrap();
                    self.root_push_i64(bits)?;
                    av.push(cstr.into());
                } else {
                    av.push(self.emit_c_abi_arg(*a, pty)?);
                }
            } else {
                let v = self.coerce_i64(self.local(*a)?)?;
                av.push(v.into());
            }
        }
        let call = crate::error::llvm(self.llvm.builder.build_call(callee, &av, "call"))?;
        if is_ext {
            self.root_pop_to(cstr_root_depth)?;
            return self.restore_c_abi_ret(fun, call);
        }
        let raw = call
            .try_as_basic_value()
            .basic()
            .unwrap_or_else(|| self.llvm.i64_ty.const_int(0, false).into());
        if matches!(self.funs.fun_ret_tys.get(fun), Some(Type::Float)) {
            let bits = raw.into_int_value();
            Ok(self
                .llvm
                .builder
                .build_bit_cast(bits, self.llvm.context.f64_type(), "call_f64")
                .unwrap())
        } else {
            Ok(raw)
        }
    }

    pub(crate) fn emit_value_indirect_call(
        &mut self,
        callee: &Local,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        let _ = args;
        // Float return ABI must come from the callee's Fun type — never
        // from "any arg is float" (that breaks Float→Int HOFs).
        let float_ret = match self.frame.local_tys.get(&callee.0) {
            Some(Type::Fun(_, ret, _)) => matches!(ret.as_ref(), Type::Float),
            _ => self
                .funs
                .funref_locals
                .get(&callee.0)
                .and_then(|name| self.funs.fun_ret_tys.get(name))
                .is_some_and(|ty| matches!(ty, Type::Float)),
        };
        let cal_i = self.coerce_i64(self.local(*callee)?)?;
        let one = self.llvm.i64_ty.const_int(1, false);
        let tagged = crate::error::llvm(self.llvm.builder.build_and(cal_i, one, "ic_tag"))?;
        let is_funref = self
            .llvm
            .builder
            .build_int_compare(IntPredicate::EQ, tagged, one, "is_funref")
            .unwrap();

        let cur = self
            .llvm
            .builder
            .get_insert_block()
            .context("indirect call needs insert block")?;
        let parent = cur.get_parent().context("bb parent")?;
        let funref_bb = self.llvm.context.append_basic_block(parent, "icall_funref");
        let clos_bb = self.llvm.context.append_basic_block(parent, "icall_clos");
        let merge_bb = self.llvm.context.append_basic_block(parent, "icall_merge");
        self.llvm
            .builder
            .build_conditional_branch(is_funref, funref_bb, clos_bb)
            .unwrap();

        // Bare FunRef (low bit set): call without env.
        self.llvm.builder.position_at_end(funref_bb);
        let not_one = crate::error::llvm(self.llvm.builder.build_not(one, "not1"))?;
        let fn_i = crate::error::llvm(self.llvm.builder.build_and(cal_i, not_one, "fn_clear"))?;
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let fn_ptr = self
            .llvm
            .builder
            .build_int_to_ptr(fn_i, ptr_ty, "fn_ptr")
            .unwrap();
        let param_tys: Vec<BasicMetadataTypeEnum> =
            args.iter().map(|_| self.llvm.i64_ty.into()).collect();
        let fn_ty = self.llvm.i64_ty.fn_type(&param_tys, false);
        let mut av: Vec<BasicMetadataValueEnum> = vec![];
        for a in args {
            let v = self.coerce_i64(self.local(*a)?)?;
            av.push(v.into());
        }
        let call_fr = self
            .llvm
            .builder
            .build_indirect_call(fn_ty, fn_ptr, &av, "icall_fr")
            .unwrap();
        let fr_v = call_fr
            .try_as_basic_value()
            .basic()
            .unwrap_or_else(|| self.llvm.i64_ty.const_int(0, false).into());
        let fr_i = self.coerce_i64(fr_v)?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(merge_bb))?;
        let funref_bb_end = self.llvm.builder.get_insert_block().unwrap();

        // Heap closure: load code ptr, pass env as first arg.
        self.llvm.builder.position_at_end(clos_bb);
        let env_ptr = self
            .llvm
            .builder
            .build_int_to_ptr(cal_i, ptr_ty, "clos_env")
            .unwrap();
        let fn_slot = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    env_ptr,
                    &[self.llvm.i64_ty.const_int(0, false)],
                    "clos_fn_slot",
                )
                .unwrap()
        };
        let fn_i2 = self
            .llvm
            .builder
            .build_load(self.llvm.i64_ty, fn_slot, "clos_fn")
            .unwrap()
            .into_int_value();
        let fn_ptr2 = self
            .llvm
            .builder
            .build_int_to_ptr(fn_i2, ptr_ty, "clos_fn_ptr")
            .unwrap();
        let mut clos_param_tys: Vec<BasicMetadataTypeEnum> = vec![self.llvm.i64_ty.into()];
        for _ in args.iter() {
            clos_param_tys.push(self.llvm.i64_ty.into());
        }
        let clos_fn_ty = self.llvm.i64_ty.fn_type(&clos_param_tys, false);
        let mut cav: Vec<BasicMetadataValueEnum> = vec![cal_i.into()];
        for a in args {
            let v = self.coerce_i64(self.local(*a)?)?;
            cav.push(v.into());
        }
        let call_cl = self
            .llvm
            .builder
            .build_indirect_call(clos_fn_ty, fn_ptr2, &cav, "icall_cl")
            .unwrap();
        let cl_v = call_cl
            .try_as_basic_value()
            .basic()
            .unwrap_or_else(|| self.llvm.i64_ty.const_int(0, false).into());
        let cl_i = self.coerce_i64(cl_v)?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(merge_bb))?;
        let clos_bb_end = self.llvm.builder.get_insert_block().unwrap();

        self.llvm.builder.position_at_end(merge_bb);
        let phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "icall_res"))?;
        phi.add_incoming(&[(&fr_i, funref_bb_end), (&cl_i, clos_bb_end)]);
        let bits = phi.as_basic_value().into_int_value();
        if float_ret {
            Ok(self
                .llvm
                .builder
                .build_bit_cast(bits, self.llvm.context.f64_type(), "icall_f64")
                .unwrap())
        } else {
            Ok(bits.into())
        }
    }

    pub(crate) fn emit_value_funref(&mut self, name: &str) -> Result<BasicValueEnum<'ctx>> {
        let fv = self
            .funs
            .functions
            .get(name)
            .copied()
            .with_context(|| format!("unknown funref {name}"))?;
        let ptr = fv.as_global_value().as_pointer_value();
        let as_i = self
            .llvm
            .builder
            .build_ptr_to_int(ptr, self.llvm.i64_ty, "funref_i64")
            .unwrap();
        // Tag low bit so IndirectCall can tell FunRef from heap closure.
        let tagged = self
            .llvm
            .builder
            .build_or(as_i, self.llvm.i64_ty.const_int(1, false), "funref_tag")
            .unwrap();
        Ok(tagged.into())
    }

    pub(crate) fn emit_value_alloc_closure(
        &mut self,
        fun: &str,
        captures: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        let n = captures.len() as u64;
        let nbytes = self.llvm.i64_ty.const_int((1 + n) * 8, false);
        let type_id = self
            .llvm
            .context
            .i32_type()
            .const_int(TYPE_CLOSURE as u64, false);
        let alloc = self.runtime_fn("lumia_alloc")?;
        let ptr = self
            .llvm
            .builder
            .build_call(alloc, &[nbytes.into(), type_id.into()], "clos_alloc")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        let fv = self
            .funs
            .functions
            .get(fun)
            .copied()
            .with_context(|| format!("unknown closure fun {fun}"))?;
        let fn_as_i = self
            .llvm
            .builder
            .build_ptr_to_int(
                fv.as_global_value().as_pointer_value(),
                self.llvm.i64_ty,
                "clos_fn_i",
            )
            .unwrap();
        let fn_slot = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    ptr,
                    &[self.llvm.i64_ty.const_int(0, false)],
                    "clos_fn_slot",
                )
                .unwrap()
        };
        crate::error::llvm(self.llvm.builder.build_store(fn_slot, fn_as_i))?;
        for (i, e) in captures.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                self.llvm
                    .builder
                    .build_gep(
                        self.llvm.i64_ty,
                        ptr,
                        &[self.llvm.i64_ty.const_int((i + 1) as u64, false)],
                        "clos_cap",
                    )
                    .unwrap()
            };
            crate::error::llvm(self.llvm.builder.build_store(slot, v))?;
        }
        Ok(self
            .llvm
            .builder
            .build_ptr_to_int(ptr, self.llvm.i64_ty, "clos_as_i64")
            .unwrap()
            .into())
    }

    pub(crate) fn emit_value_closure_cap(
        &mut self,
        env: &Local,
        index: u32,
        as_float: bool,
    ) -> Result<BasicValueEnum<'ctx>> {
        let env_i = self.coerce_i64(self.local(*env)?)?;
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let env_ptr = self
            .llvm
            .builder
            .build_int_to_ptr(env_i, ptr_ty, "cap_env")
            .unwrap();
        let slot = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    env_ptr,
                    &[self.llvm.i64_ty.const_int((index as u64) + 1, false)],
                    "cap_slot",
                )
                .unwrap()
        };
        let loaded =
            crate::error::llvm(self.llvm.builder.build_load(self.llvm.i64_ty, slot, "cap"))?;
        if as_float {
            Ok(self
                .llvm
                .builder
                .build_bit_cast(
                    loaded.into_int_value(),
                    self.llvm.context.f64_type(),
                    "cap_f64",
                )
                .unwrap())
        } else {
            Ok(loaded)
        }
    }
}
