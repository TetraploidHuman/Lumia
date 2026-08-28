//! Value emission — builtin intrinsics (split by family).

mod io;
mod list;
mod show;

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, IntValue, PointerValue};
use inkwell::AddressSpace;
use lumi_core::Local;
use lumi_hir::{Builtin, BuiltinEmit, BuiltinFamily};
use lumi_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_value_builtin(
        &mut self,
        name: &Builtin,
        args: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        let emit = name.info().emit;
        if emit != BuiltinEmit::Custom {
            return self.emit_by_convention(name, args, emit);
        }
        match name.family() {
            BuiltinFamily::Io => self.emit_io_builtin(name, args),
            BuiltinFamily::List => self.emit_list_builtin(name, args),
            BuiltinFamily::MapSet | BuiltinFamily::String | BuiltinFamily::Adt => unreachable!(
                "builtin `{}` marked Custom but family has no hand-written emit",
                name.display_name()
            ),
        }
    }

    fn emit_by_convention(
        &mut self,
        b: &Builtin,
        args: &[Local],
        emit: BuiltinEmit,
    ) -> Result<BasicValueEnum<'ctx>> {
        let label = b.display_name();
        match emit {
            BuiltinEmit::Custom => unreachable!("Custom handled by family emit"),
            BuiltinEmit::NullaryPtr => {
                let sym = Self::builtin_symbol(b)?;
                self.call_rt_ptr_as_i64(sym, &[], label)
            }
            BuiltinEmit::NullaryVoid => {
                let sym = Self::builtin_symbol(b)?;
                self.call_rt_void(sym, &[], label)?;
                Ok(self.llvm.i64_ty.const_int(0, false).into())
            }
            BuiltinEmit::UnaryObjPtr => self.emit_rt_unary_obj(b, args, label),
            BuiltinEmit::UnaryObjScalar => {
                let obj_i = self.coerce_i64(self.local(args[0])?)?;
                let obj = self.i64_as_ptr(obj_i, "obj")?;
                let sym = if matches!(b, Builtin::ListLen)
                    && matches!(self.frame.local_tys.get(&args[0].0), Some(Type::List(_)))
                {
                    "lumi_list_len"
                } else {
                    Self::builtin_symbol(b)?
                };
                self.call_rt_basic(sym, &[obj.into()], label)
            }
            BuiltinEmit::ObjI64Ptr => self.emit_rt_obj_i64(b, args, label),
            BuiltinEmit::ObjI64Scalar => self.emit_rt_obj_i64_scalar(b, args, label),
            BuiltinEmit::ObjObjPtr => self.emit_rt_obj_obj(b, args, label),
            BuiltinEmit::ObjObjScalar => self.emit_rt_obj_obj_scalar(b, args, label),
            BuiltinEmit::I64I64Ptr => self.emit_rt_i64_i64_ptr(b, args, label),
            BuiltinEmit::ObjI64I64Ptr => {
                let obj_i = self.coerce_i64(self.local(args[0])?)?;
                let a = self.coerce_i64(self.local(args[1])?)?;
                let b_i = self.coerce_i64(self.local(args[2])?)?;
                let mut obj = self.i64_as_ptr(obj_i, "obj")?;
                obj = self.ensure_float_container(b, args, obj)?;
                // List/Map `set`: retain source when the old binding stays live.
                // Skipped for proven `xs = xs.set(…)` so unique RC can write in place.
                let cow_retain_src =
                    matches!(b, Builtin::MapSet) && !self.frame.cow_consume_unique;
                if cow_retain_src {
                    self.list_retain_i64(obj_i)?;
                }
                // Retained value/elem so container alias does not leave nested COW at rc==1.
                if matches!(b, Builtin::MapSet) {
                    if let Some(ty) = self.frame.local_tys.get(&args[2].0) {
                        if Self::type_needs_cow_retain(ty) {
                            self.adt_retain_i64(b_i)?;
                        }
                    }
                }
                // Known `List` → skip polymorphic `lumi_set` dispatch.
                let sym = if matches!(b, Builtin::MapSet)
                    && matches!(self.frame.local_tys.get(&args[0].0), Some(Type::List(_)))
                {
                    "lumi_list_set"
                } else {
                    Self::builtin_symbol(b)?
                };
                let out = self.call_rt_ptr_as_i64(sym, &[obj.into(), a.into(), b_i.into()], label)?;
                if cow_retain_src {
                    self.list_release_i64(obj_i)?;
                }
                Ok(out)
            }
            BuiltinEmit::ObjI64OptionTags => {
                let obj_i = self.coerce_i64(self.local(args[0])?)?;
                let key = self.coerce_i64(self.local(args[1])?)?;
                let obj = self.i64_as_ptr(obj_i, "obj")?;
                // Known `List` → `lumi_list_get` (no Option tags / map dispatch).
                if matches!(b, Builtin::ListGet)
                    && matches!(self.frame.local_tys.get(&args[0].0), Some(Type::List(_)))
                {
                    return self.call_rt_basic("lumi_list_get", &[obj.into(), key.into()], label);
                }
                let some = self
                    .llvm
                    .i64_ty
                    .const_int(self.option_some_tag as u64, true);
                let none = self
                    .llvm
                    .i64_ty
                    .const_int(self.option_none_tag as u64, true);
                let sym = Self::builtin_symbol(b)?;
                self.call_rt_basic(
                    sym,
                    &[obj.into(), key.into(), some.into(), none.into()],
                    label,
                )
            }
        }
    }

    /// Required `lumi_*` symbol from [`Builtin::info`].
    pub(crate) fn builtin_symbol(b: &Builtin) -> Result<&'static str> {
        b.runtime_symbol()
            .with_context(|| format!("builtin `{}` has no runtime_symbol", b.display_name()))
    }

    /// Apply [`BuiltinInfo::float_ensures`](lumi_hir::BuiltinInfo::float_ensures) to `obj`.
    ///
    /// `MapSet` is overloaded for `List.set` / `Map.set`. List destinations must
    /// use `ENSURE_LIST_F64` when the written elem is Float — never map ensures.
    fn ensure_float_container(
        &mut self,
        b: &Builtin,
        args: &[Local],
        mut obj: PointerValue<'ctx>,
    ) -> Result<PointerValue<'ctx>> {
        if matches!(b, Builtin::MapSet) {
            let val_float = matches!(self.frame.local_tys.get(&args[2].0), Some(Type::Float));
            let key_float = matches!(self.frame.local_tys.get(&args[1].0), Some(Type::Float));
            match self.frame.local_tys.get(&args[0].0) {
                Some(Type::List(_)) if val_float => {
                    return self.call_ensure(obj, lumi_abi::ENSURE_LIST_F64);
                }
                Some(Type::Map(_, _)) => {
                    if key_float {
                        obj = self.call_ensure(obj, lumi_abi::ENSURE_MAP_F64)?;
                    }
                    if val_float {
                        obj = self.call_ensure(obj, lumi_abi::ENSURE_MAP_VF64)?;
                    }
                    return Ok(obj);
                }
                // Unknown / poly: skip compile-time ensure; RT `lumi_set` dispatches.
                _ => return Ok(obj),
            }
        }
        for &(idx, sym) in b.info().float_ensures {
            let i = idx as usize;
            if matches!(self.frame.local_tys.get(&args[i].0), Some(Type::Float)) {
                obj = self.call_ensure(obj, sym)?;
            }
        }
        Ok(obj)
    }

    fn call_ensure(&mut self, obj: PointerValue<'ctx>, sym: &str) -> Result<PointerValue<'ctx>> {
        let ens = self.runtime_fn(sym)?;
        Ok(
            crate::error::llvm(self.llvm.builder.build_call(ens, &[obj.into()], "ens_f"))?
                .try_as_basic_value()
                .basic()
                .with_context(|| format!("ensure `{sym}` return"))?
                .into_pointer_value(),
        )
    }

    pub(crate) fn i64_as_ptr(&self, i: IntValue<'ctx>, name: &str) -> Result<PointerValue<'ctx>> {
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        crate::error::llvm(self.llvm.builder.build_int_to_ptr(i, ptr_ty, name))
    }

    pub(crate) fn ptr_as_i64(
        &self,
        ptr: PointerValue<'ctx>,
        name: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        Ok(crate::error::llvm(
            self.llvm
                .builder
                .build_ptr_to_int(ptr, self.llvm.i64_ty, name),
        )?
        .into())
    }

    /// Call a runtime fn that returns a pointer; box as i64.
    pub(crate) fn call_rt_ptr_as_i64(
        &self,
        symbol: &'static str,
        args: &[BasicMetadataValueEnum<'ctx>],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        let ptr = self
            .call_rt_basic(symbol, args, label)?
            .into_pointer_value();
        self.ptr_as_i64(ptr, &format!("{label}_i64"))
    }

    /// `args[0]` as heap ptr → call unary runtime returning ptr→i64.
    pub(crate) fn emit_rt_unary_obj(
        &mut self,
        b: &Builtin,
        args: &[Local],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        let obj_i = self.coerce_i64(self.local(args[0])?)?;
        let obj = self.i64_as_ptr(obj_i, "obj")?;
        // `xs = xs.reverse|sort()` → in-place when unique; else always fresh copy.
        let sym = if self.frame.cow_consume_unique {
            match b {
                Builtin::ListReverse => "lumi_list_reverse_consume",
                Builtin::ListSort => "lumi_list_sort_consume",
                _ => Self::builtin_symbol(b)?,
            }
        } else {
            Self::builtin_symbol(b)?
        };
        self.call_rt_ptr_as_i64(sym, &[obj.into()], label)
    }

    /// `args[0]` ptr + `args[1]` i64 → runtime ptr→i64.
    pub(crate) fn emit_rt_obj_i64(
        &mut self,
        b: &Builtin,
        args: &[Local],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        let obj_i = self.coerce_i64(self.local(args[0])?)?;
        let n = self.coerce_i64(self.local(args[1])?)?;
        let mut obj = self.i64_as_ptr(obj_i, "obj")?;
        obj = self.ensure_float_container(b, args, obj)?;
        // List append / Set insert: temporary retain when old binding stays live.
        let cow_retain_src = matches!(b, Builtin::ListAppend | Builtin::SetInsert)
            && !self.frame.cow_consume_unique;
        if cow_retain_src {
            self.list_retain_i64(obj_i)?;
        }
        if matches!(b, Builtin::ListAppend | Builtin::SetInsert) {
            if let Some(ty) = self.frame.local_tys.get(&args[1].0) {
                if Self::type_needs_cow_retain(ty) {
                    self.adt_retain_i64(n)?;
                }
            }
        }
        // `xs = xs.take|slice` → consume; SetInsert uses runtime unique check.
        let sym = if self.frame.cow_consume_unique {
            match b {
                Builtin::ListTake => "lumi_list_take_consume",
                Builtin::ListSlice => "lumi_list_slice_consume",
                _ => Self::builtin_symbol(b)?,
            }
        } else {
            Self::builtin_symbol(b)?
        };
        let out = self.call_rt_ptr_as_i64(sym, &[obj.into(), n.into()], label)?;
        if cow_retain_src {
            self.list_release_i64(obj_i)?;
        }
        Ok(out)
    }

    /// Two heap objects → runtime ptr→i64.
    pub(crate) fn emit_rt_obj_obj(
        &mut self,
        b: &Builtin,
        args: &[Local],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        let a_i = self.coerce_i64(self.local(args[0])?)?;
        let b_i = self.coerce_i64(self.local(args[1])?)?;
        let a = self.i64_as_ptr(a_i, "a")?;
        let bb = self.i64_as_ptr(b_i, "b")?;
        let cow_retain_a =
            matches!(b, Builtin::ListConcat) && !self.frame.cow_consume_unique;
        if cow_retain_a {
            self.list_retain_i64(a_i)?;
        }
        let sym = Self::builtin_symbol(b)?;
        let out = self.call_rt_ptr_as_i64(sym, &[a.into(), bb.into()], label)?;
        if cow_retain_a {
            self.list_release_i64(a_i)?;
        }
        Ok(out)
    }

    /// Two bare i64 args → runtime ptr→i64 (e.g. range).
    pub(crate) fn emit_rt_i64_i64_ptr(
        &mut self,
        b: &Builtin,
        args: &[Local],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        let a = self.coerce_i64(self.local(args[0])?)?;
        let b_i = self.coerce_i64(self.local(args[1])?)?;
        let sym = Self::builtin_symbol(b)?;
        self.call_rt_ptr_as_i64(sym, &[a.into(), b_i.into()], label)
    }

    /// Object + i64 → scalar return (no ptr boxing), e.g. `contains`.
    pub(crate) fn emit_rt_obj_i64_scalar(
        &mut self,
        b: &Builtin,
        args: &[Local],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        let obj_i = self.coerce_i64(self.local(args[0])?)?;
        let key = self.coerce_i64(self.local(args[1])?)?;
        let obj = self.i64_as_ptr(obj_i, "obj")?;
        let sym = Self::builtin_symbol(b)?;
        self.call_rt_basic(sym, &[obj.into(), key.into()], label)
    }

    /// Two heap objects → scalar return (e.g. startsWith).
    pub(crate) fn emit_rt_obj_obj_scalar(
        &mut self,
        b: &Builtin,
        args: &[Local],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        let a_i = self.coerce_i64(self.local(args[0])?)?;
        let b_i = self.coerce_i64(self.local(args[1])?)?;
        let a = self.i64_as_ptr(a_i, "a")?;
        let bb = self.i64_as_ptr(b_i, "b")?;
        let sym = Self::builtin_symbol(b)?;
        self.call_rt_basic(sym, &[a.into(), bb.into()], label)
    }
}
