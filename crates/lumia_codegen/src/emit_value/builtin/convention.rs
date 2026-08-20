//! Builtin emit conventions (table-driven RT call shapes).

use crate::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, IntValue, PointerValue};
use inkwell::AddressSpace;
use lumia_core::Local;
use lumia_hir::{Builtin, BuiltinEmit};
use lumia_ty::Type;

/// Prefer String vs List RT override when picking a receiver symbol.
pub(super) enum RecvRtPrefer {
    String,
    List,
}

impl<'ctx> Codegen<'ctx> {
    pub(super) fn emit_by_convention(
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
                let sym = self.rt_symbol_for_list_receiver(b, args[0])?;
                self.call_rt_basic(sym, &[obj.into()], label)
            }
            BuiltinEmit::ObjI64Ptr => self.emit_rt_obj_i64(b, args, label),
            BuiltinEmit::ObjI64Scalar => self.emit_rt_obj_i64_scalar(b, args, label),
            BuiltinEmit::ObjObjPtr => self.emit_rt_obj_obj(b, args, label),
            BuiltinEmit::ObjObjScalar => self.emit_rt_obj_obj_scalar(b, args, label),
            BuiltinEmit::I64I64Ptr => self.emit_rt_i64_i64_ptr(b, args, label),
            BuiltinEmit::ObjI64I64Ptr => self.emit_obj_i64_i64_ptr(b, args, label),
            BuiltinEmit::ObjI64OptionTags => self.emit_obj_i64_option_tags(b, args, label),
            BuiltinEmit::UnaryObjBoolMask => self.emit_rt_unary_obj_bool_mask(b, args, label),
        }
    }

    /// `MapItems`: pass Bool key/val layout bits so RT pair ADTs get `_pad` bool masks
    /// (Float still comes from the map's TID_F_* bits).
    fn emit_rt_unary_obj_bool_mask(
        &mut self,
        b: &Builtin,
        args: &[Local],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        let obj_i = self.coerce_i64(self.local(args[0])?)?;
        let obj = self.i64_as_ptr(obj_i, "obj")?;
        let bool_mask = Self::map_bool_mask_bits(self.frame.local_tys.get(&args[0].0));
        let bool_mask_v = self.llvm.i64_ty.const_int(bool_mask, false);
        let sym = Self::builtin_symbol(b)?;
        self.call_rt_ptr_as_i64(sym, &[obj.into(), bool_mask_v.into()], label)
    }

    /// `MapSet` / list-index set: retain + float ensure + list-receiver symbol.
    fn emit_obj_i64_i64_ptr(
        &mut self,
        b: &Builtin,
        args: &[Local],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        let obj_i = self.coerce_i64(self.local(args[0])?)?;
        let a = self.coerce_i64(self.local(args[1])?)?;
        let b_i = self.coerce_i64(self.local(args[2])?)?;
        let mut obj = self.i64_as_ptr(obj_i, "obj")?;
        obj = self.ensure_float_container(b, args, obj)?;
        // List/Map `set`: retain source (+ nested val) when the old binding
        // stays live. Skipped for proven `xs = xs.set(…)` (unique RC in-place).
        // StrSubstring shares ObjI64I64Ptr — gate on MapSet only.
        if matches!(b, Builtin::MapSet) {
            self.cow_retain_mutator_args(obj_i, Some((args[2], b_i)))?;
        }
        // Known `List` → skip polymorphic `lumia_set` dispatch.
        let sym = self.rt_symbol_for_list_receiver(b, args[0])?;
        self.call_rt_ptr_as_i64(sym, &[obj.into(), a.into(), b_i.into()], label)
    }

    /// `ListGet` / map get: typed List fast-path vs Option-tagged map lookup.
    fn emit_obj_i64_option_tags(
        &mut self,
        b: &Builtin,
        args: &[Local],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        let obj_i = self.coerce_i64(self.local(args[0])?)?;
        let key = self.coerce_i64(self.local(args[1])?)?;
        let obj = self.i64_as_ptr(obj_i, "obj")?;
        // Known `List` → `lumia_list_get` (no Option tags / map dispatch).
        if matches!(b, Builtin::ListGet)
            && matches!(self.frame.local_tys.get(&args[0].0), Some(Type::List(_)))
        {
            return self.call_rt_basic(
                Self::list_receiver_rt_override(Builtin::ListGet).unwrap_or("lumia_list_get"),
                &[obj.into(), key.into()],
                label,
            );
        }
        let some = self
            .llvm
            .i64_ty
            .const_int(self.option_variant_tag("Some") as u64, true);
        let none = self
            .llvm
            .i64_ty
            .const_int(self.option_variant_tag("None") as u64, true);
        let show_kind = self
            .funs
            .adt_show_kinds
            .get(lumia_hir::OPTION.name)
            .copied()
            .unwrap_or(0);
        let show_kind_v = self.llvm.i64_ty.const_int(show_kind as u64, false);
        // Bool payload mask for RT-built Option (Float comes from map TID_F_VAL).
        let bool_mask =
            (Self::map_bool_mask_bits(self.frame.local_tys.get(&args[0].0)) & 0b10) >> 1;
        let bool_mask_v = self.llvm.i64_ty.const_int(bool_mask, false);
        let sym = Self::builtin_symbol(b)?;
        self.call_rt_basic(
            sym,
            &[
                obj.into(),
                key.into(),
                some.into(),
                none.into(),
                show_kind_v.into(),
                bool_mask_v.into(),
            ],
            label,
        )
    }

    /// Required `lumia_*` symbol from [`Builtin::info`].
    pub(crate) fn builtin_symbol(b: &Builtin) -> Result<&'static str> {
        b.runtime_symbol()
            .with_context(|| format!("builtin `{}` has no runtime_symbol", b.display_name()))
    }

    /// List-family builtins that share the List emit convention but need a
    /// dedicated String RT entry when the receiver is typed `String`.
    ///
    /// Authority: [`Builtin::string_receiver_rt_override`].
    pub(crate) fn string_receiver_rt_override(b: Builtin) -> Option<&'static str> {
        b.string_receiver_rt_override()
    }

    /// When the receiver is a known `List`, use the monomorphic list RT entry.
    /// Authority: [`Builtin::list_receiver_rt_override`].
    pub(crate) fn list_receiver_rt_override(b: Builtin) -> Option<&'static str> {
        b.list_receiver_rt_override()
    }

    fn rt_symbol_for_receiver(&self, b: &Builtin, recv: Local) -> Result<&'static str> {
        self.rt_symbol_for_typed_receiver(b, recv, RecvRtPrefer::String)
    }

    fn rt_symbol_for_list_receiver(&self, b: &Builtin, recv: Local) -> Result<&'static str> {
        self.rt_symbol_for_typed_receiver(b, recv, RecvRtPrefer::List)
    }

    fn rt_symbol_for_typed_receiver(
        &self,
        b: &Builtin,
        recv: Local,
        prefer: RecvRtPrefer,
    ) -> Result<&'static str> {
        let ty = self.frame.local_tys.get(&recv.0);
        let override_sym = match prefer {
            RecvRtPrefer::String if matches!(ty, Some(Type::String)) => {
                Self::string_receiver_rt_override(*b)
            }
            RecvRtPrefer::List if matches!(ty, Some(Type::List(_))) => {
                Self::list_receiver_rt_override(*b)
            }
            _ => None,
        };
        override_sym
            .map(Ok)
            .unwrap_or_else(|| Self::builtin_symbol(b))
    }

    /// Map Bool layout bits: bit0 = key Bool, bit1 = val Bool.
    fn map_bool_mask_bits(ty: Option<&Type>) -> u64 {
        match ty {
            Some(Type::Map(k, v)) => {
                let mut m = 0u64;
                if matches!(k.as_ref(), Type::Bool) {
                    m |= 0b1;
                }
                if matches!(v.as_ref(), Type::Bool) {
                    m |= 0b10;
                }
                m
            }
            _ => 0u64,
        }
    }

    /// Apply float/bool ensure tables to `obj`.
    ///
    /// `MapSet` is overloaded for `List.set` / `Map.set`. List destinations must
    /// use `ENSURE_LIST_*` when the written elem is Float/Bool — never map ensures.
    fn ensure_float_container(
        &mut self,
        b: &Builtin,
        args: &[Local],
        mut obj: PointerValue<'ctx>,
    ) -> Result<PointerValue<'ctx>> {
        if matches!(b, Builtin::MapSet) {
            let val_float = matches!(self.frame.local_tys.get(&args[2].0), Some(Type::Float));
            let val_bool = matches!(self.frame.local_tys.get(&args[2].0), Some(Type::Bool));
            match self.frame.local_tys.get(&args[0].0) {
                Some(Type::List(_)) if val_float => {
                    return self.call_ensure(obj, lumia_abi::ENSURE_LIST_F64);
                }
                Some(Type::List(_)) if val_bool => {
                    return self.call_ensure(obj, lumia_abi::ENSURE_LIST_BOOL);
                }
                Some(Type::Map(_, _)) => {
                    obj = self.apply_ensure_table(obj, args, b.info().float_ensures, |t| {
                        matches!(t, Type::Float)
                    })?;
                    return self.apply_ensure_table(obj, args, b.info().bool_ensures, |t| {
                        matches!(t, Type::Bool)
                    });
                }
                // Unknown / poly: skip compile-time ensure; RT `lumia_set` dispatches.
                _ => return Ok(obj),
            }
        }
        obj = self.apply_ensure_table(obj, args, b.info().float_ensures, |t| {
            matches!(t, Type::Float)
        })?;
        self.apply_ensure_table(obj, args, b.info().bool_ensures, |t| {
            matches!(t, Type::Bool)
        })
    }

    fn apply_ensure_table(
        &mut self,
        mut obj: PointerValue<'ctx>,
        args: &[Local],
        table: &[(u8, &str)],
        pred: impl Fn(&Type) -> bool,
    ) -> Result<PointerValue<'ctx>> {
        for &(idx, sym) in table {
            let i = idx as usize;
            if self
                .frame
                .local_tys
                .get(&args[i].0)
                .is_some_and(|t| pred(t))
            {
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
        if matches!(b, Builtin::ListReverse | Builtin::ListSort) {
            self.cow_retain_mutator_args(obj_i, None)?;
        }
        let sym = self.rt_symbol_for_receiver(b, args[0])?;
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
        // List append / Set insert: retain source (+ nested elem) unless
        // `xs = xs.append/insert(…)` consumed uniqueness.
        if matches!(b, Builtin::ListAppend | Builtin::SetInsert) {
            self.cow_retain_mutator_args(obj_i, Some((args[1], n)))?;
        } else if matches!(b, Builtin::MapRemove) {
            self.cow_retain_mutator_args(obj_i, None)?;
        }
        let sym = self.rt_symbol_for_receiver(b, args[0])?;
        self.call_rt_ptr_as_i64(sym, &[obj.into(), n.into()], label)
    }

    /// Two heap objects → runtime ptr→i64.
    pub(crate) fn emit_rt_obj_obj(
        &mut self,
        b: &Builtin,
        args: &[Local],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        self.emit_rt_two_objs(b, args, label, /*as_i64_ptr*/ true)
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
        self.emit_rt_two_objs(b, args, label, /*as_i64_ptr*/ false)
    }

    /// Shared ObjObj Ptr/Scalar: two heap ptrs + receiver symbol.
    fn emit_rt_two_objs(
        &mut self,
        b: &Builtin,
        args: &[Local],
        label: &str,
        as_i64_ptr: bool,
    ) -> Result<BasicValueEnum<'ctx>> {
        let a_i = self.coerce_i64(self.local(args[0])?)?;
        let b_i = self.coerce_i64(self.local(args[1])?)?;
        let a = self.i64_as_ptr(a_i, "a")?;
        let bb = self.i64_as_ptr(b_i, "b")?;
        if as_i64_ptr
            && matches!(
                b,
                Builtin::ListConcat
                    | Builtin::ListSortByKeys
                    | Builtin::SetUnion
                    | Builtin::SetIntersect
                    | Builtin::SetDiff
            )
        {
            self.cow_retain_mutator_args(a_i, None)?;
        }
        let sym = if as_i64_ptr {
            self.rt_symbol_for_receiver(b, args[0])?
        } else {
            Self::builtin_symbol(b)?
        };
        let call_args: [BasicMetadataValueEnum; 2] = [a.into(), bb.into()];
        if as_i64_ptr {
            self.call_rt_ptr_as_i64(sym, &call_args, label)
        } else {
            self.call_rt_basic(sym, &call_args, label)
        }
    }
}
