//! Show / Eq / Ord overrides and typed ADT helpers.

use super::emit_value::builtin::show::{SHOW_ADT, SHOW_ADT_NAMED};
use super::Codegen;
use anyhow::{bail, Context as AnyhowContext, Result};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, IntValue, PointerValue};
use inkwell::AddressSpace;
use lumia_core::Local;
use lumia_ty::Type;

/// Eq RT symbols referenced from codegen. Keep in sync with `runtime_decls`.
pub(crate) const EQ_GENERIC: &str = "lumia_eq";
pub(crate) const EQ_ADT: &str = "lumia_adt_eq";

/// Lock-in table for `runtime_decls` tests (production uses `EQ_*` consts directly).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const SPECIALIZED_EQ_RT: &[&str] = &[EQ_GENERIC, EQ_ADT];

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn key_type_has_hash(&self, ty: &Type) -> bool {
        match ty {
            Type::Adt { name, .. } => self.funs.hash_adts.contains(name),
            // Scalars / collections: structural hash always available.
            Type::Int
            | Type::Float
            | Type::Bool
            | Type::String
            | Type::Char
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Set(_) => true,
            _ => false,
        }
    }

    /// Shared Show/Eq/Ord instance call (`__{Trait}_{T}_{method}`).
    fn call_trait_override(
        &mut self,
        trait_name: &str,
        type_name: &str,
        method: &str,
        args: &[BasicMetadataValueEnum<'ctx>],
        call_name: &str,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let mangled = lumia_hir::mangle_trait_method(trait_name, type_name, method);
        let Some(fv) = self.funs.functions.get(&mangled).copied() else {
            return Ok(None);
        };
        let call = crate::error::llvm(self.llvm.builder.build_call(fv, args, call_name))?;
        Ok(Some(
            call.try_as_basic_value()
                .basic()
                .context("call return value")?,
        ))
    }

    /// Call `__Show_{T}_show` when an instance provided a custom Show method.
    pub(crate) fn emit_show_override(
        &mut self,
        adt_name: &str,
        arg: BasicValueEnum<'ctx>,
    ) -> Result<Option<PointerValue<'ctx>>> {
        let i = self.coerce_i64(arg)?;
        let Some(bits) = self.call_trait_override(
            "Show",
            adt_name,
            "show",
            &[i.into()],
            "show_ov",
        )?
        else {
            return Ok(None);
        };
        let bits = bits.into_int_value();
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let ptr = crate::error::llvm(self.llvm.builder.build_int_to_ptr(
            bits,
            ptr_ty,
            "show_ov_ptr",
        ))?;
        Ok(Some(ptr))
    }

    /// Call `__Eq_{T}_eq(a,b) -> Bool` when present.
    pub(crate) fn emit_eq_override(
        &mut self,
        adt_name: &str,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> Result<Option<IntValue<'ctx>>> {
        Ok(self
            .call_trait_override(
                "Eq",
                adt_name,
                "eq",
                &[left.into(), right.into()],
                "eq_ov",
            )?
            .map(|v| v.into_int_value()))
    }

    /// Call `__Ord_{T}_less(a,b) -> Bool` when present.
    pub(crate) fn emit_less_override(
        &mut self,
        adt_name: &str,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> Result<Option<IntValue<'ctx>>> {
        Ok(self
            .call_trait_override(
                "Ord",
                adt_name,
                "less",
                &[left.into(), right.into()],
                "less_ov",
            )?
            .map(|v| v.into_int_value()))
    }

    pub(crate) fn adt_method_name(left: &Type, right: &Type) -> Option<String> {
        match (left, right) {
            (Type::Adt { name: a, .. }, Type::Adt { name: b, .. }) if a == b => Some(a.clone()),
            _ => None,
        }
    }

    /// Typed `==` for ADTs with Float fields and fallback to `lumia_eq`.
    pub(crate) fn emit_value_eq(
        &mut self,
        lt: &Type,
        rt: &Type,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>> {
        // Int/Bool/Unit: bit identity (matches `lumia_eq` scalar short-circuit).
        if Self::is_bit_identity_scalar(lt) && Self::is_bit_identity_scalar(rt) {
            let c = crate::error::llvm(self.llvm.builder.build_int_compare(
                inkwell::IntPredicate::EQ,
                l,
                r,
                "eq_bits",
            ))?;
            return crate::error::llvm(self.llvm.builder.build_int_z_extend(
                c,
                self.llvm.i64_ty,
                "eqz",
            ));
        }
        if let Some(name) = Self::adt_method_name(lt, rt) {
            // Hash ADTs use `lumia_eq` for Map/Set keys — keep `==` on the same path
            // so a custom `__Eq_*_eq` cannot diverge from containment.
            if !self.funs.hash_adts.contains(&name) {
                if let Some(eq) = self.emit_eq_override(&name, l, r)? {
                    return Ok(eq);
                }
            }
            if let (Type::Adt { name, params: lp }, Type::Adt { params: rp, .. }) = (lt, rt)
            {
                if lp.iter().any(|p| matches!(p, Type::Float))
                    || rp.iter().any(|p| matches!(p, Type::Float))
                    || lumia_hir::is_result(name)
                {
                    // Result always uses lumia_adt_eq (mask 0 → per-object `_pad`):
                    // type-param index ≠ constructor field index (Err field0 is
                    // params[1]), same as typed Show.
                    return self.emit_typed_adt_eq(name, l, r, lp, rp);
                }
            }
        }
        let f = self.runtime_fn(EQ_GENERIC)?;
        Ok(
            crate::error::llvm(self.llvm.builder.build_call(f, &[l.into(), r.into()], "eq"))?
                .try_as_basic_value()
                .basic()
                .context("call return value")?
                .into_int_value(),
        )
    }

    /// Bit `i` set ⇒ field `i` uses IEEE eq/show (union of both sides' params).
    ///
    /// Runtime stores the mask in a `u64` header word — ADTs with more than 64
    /// fields cannot be represented and are rejected at emit time.
    pub(crate) fn adt_float_field_mask(lp: &[Type], rp: &[Type]) -> Result<u64> {
        let n = lp.len().max(rp.len());
        if n > 32 {
            bail!(
                "ICE: ADT float field mask needs {n} bits; packed `_pad` supports at most 32 float fields"
            );
        }
        let mut mask = 0u64;
        for i in 0..n {
            let lf = matches!(lp.get(i), Some(Type::Float));
            let rf = matches!(rp.get(i), Some(Type::Float));
            if lf || rf {
                mask |= 1u64 << i;
            }
        }
        Ok(mask)
    }

    /// Bit `i` set ⇒ field `i` prints / compares as Bool.
    pub(crate) fn adt_bool_field_mask(lp: &[Type], rp: &[Type]) -> Result<u64> {
        let n = lp.len().max(rp.len());
        if n > 32 {
            bail!(
                "ICE: ADT bool field mask needs {n} bits; packed `_pad` supports at most 32 bool fields"
            );
        }
        let mut mask = 0u64;
        for i in 0..n {
            let lb = matches!(lp.get(i), Some(Type::Bool));
            let rb = matches!(rp.get(i), Some(Type::Bool));
            if lb || rb {
                mask |= 1u64 << i;
            }
        }
        Ok(mask)
    }

    /// Layout mask from concrete field SSA types at an `AllocAdt` site.
    pub(crate) fn adt_float_mask_from_fields(&self, fields: &[Local]) -> Result<u64> {
        if fields.len() > 32 {
            bail!(
                "ICE: AllocAdt has {} fields; packed float mask supports at most 32",
                fields.len()
            );
        }
        let mut mask = 0u64;
        for (i, f) in fields.iter().enumerate() {
            if matches!(self.frame.local_tys.get(&f.0), Some(Type::Float)) {
                mask |= 1u64 << i;
            }
        }
        Ok(mask)
    }

    /// Bool field mask from concrete SSA types at an `AllocAdt` site.
    pub(crate) fn adt_bool_mask_from_fields(&self, fields: &[Local]) -> Result<u64> {
        if fields.len() > 32 {
            bail!(
                "ICE: AllocAdt has {} fields; packed bool mask supports at most 32",
                fields.len()
            );
        }
        let mut mask = 0u64;
        for (i, f) in fields.iter().enumerate() {
            if matches!(self.frame.local_tys.get(&f.0), Some(Type::Bool)) {
                mask |= 1u64 << i;
            }
        }
        Ok(mask)
    }

    /// Call [`lumia_abi::ADT_SET_FLOAT_MASK`] when `mask != 0` (no-op otherwise).
    /// Sole emit site for ADT float masks (heap AllocAdt, stack LitAdt, Option).
    pub(crate) fn emit_adt_set_float_mask(
        &self,
        payload: inkwell::values::PointerValue<'ctx>,
        mask: u64,
    ) -> Result<()> {
        if mask == 0 {
            return Ok(());
        }
        let m = self.llvm.i64_ty.const_int(mask, false);
        self.call_rt_void(
            lumia_abi::ADT_SET_FLOAT_MASK,
            &[payload.into(), m.into()],
            "adt_fmask",
        )
    }

    /// Call [`lumia_abi::ADT_SET_BOOL_MASK`] when `mask != 0`.
    pub(crate) fn emit_adt_set_bool_mask(
        &self,
        payload: inkwell::values::PointerValue<'ctx>,
        mask: u64,
    ) -> Result<()> {
        if mask == 0 {
            return Ok(());
        }
        let m = self.llvm.i64_ty.const_int(mask, false);
        self.call_rt_void(
            lumia_abi::ADT_SET_BOOL_MASK,
            &[payload.into(), m.into()],
            "adt_bmask",
        )
    }

    /// Structural ADT `==` via runtime size (safe for sum None/Ok arity ≠ type params).
    pub(crate) fn emit_typed_adt_eq(
        &mut self,
        adt_name: &str,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
        lp: &[Type],
        rp: &[Type],
    ) -> Result<IntValue<'ctx>> {
        // Result: type-param index ≠ constructor field index — rely on `_pad`.
        let mask = if lumia_hir::is_result(adt_name) {
            0
        } else {
            Self::adt_float_field_mask(lp, rp)?
        };
        let f = self.runtime_fn(EQ_ADT)?;
        Ok(crate::error::llvm(self.llvm.builder.build_call(
            f,
            &[
                left.into(),
                right.into(),
                self.llvm.i64_ty.const_int(mask, false).into(),
            ],
            "adt_eq",
        ))?
        .try_as_basic_value()
        .basic()
        .context("call return value")?
        .into_int_value())
    }

    /// Structural ADT show; uses constructor / type names when known.
    pub(crate) fn emit_typed_adt_show(
        &mut self,
        adt_name: &str,
        arg: BasicValueEnum<'ctx>,
        params: &[Type],
    ) -> Result<PointerValue<'ctx>> {
        let i = self.coerce_i64(arg)?;
        // Result: type-param index ≠ constructor field index (Err field0 is
        // params[1]). Rely on per-object `_pad` from AllocAdt for Float.
        // Option: Some(x) field0 == params[0] — use param mask so RT-built
        // `map.get` Options still print Float when `_pad` was historically 0.
        let fmask = if lumia_hir::is_result(adt_name) {
            0
        } else {
            Self::adt_float_field_mask(params, &[])?
        };
        // Result: same index mismatch for Bool as Float — rely on `_pad`.
        let bmask = if lumia_hir::is_result(adt_name) {
            0
        } else {
            Self::adt_bool_field_mask(params, &[])?
        };
        let fmask_v = self.llvm.i64_ty.const_int(fmask, false);
        let bmask_v = self.llvm.i64_ty.const_int(bmask, false);
        if let Some(names) = self.funs.adt_variant_names.get(adt_name).cloned() {
            return self.emit_show_adt_named(i, fmask_v, bmask_v, &names);
        }
        let f = self.runtime_fn(SHOW_ADT)?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            f,
            &[i.into(), fmask_v.into(), bmask_v.into()],
            "show_adt",
        ))?;
        Ok(call
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_pointer_value())
    }

    /// `lumia_show_adt_named(obj, float_mask, bool_mask, names_ptr, n)`.
    pub(crate) fn emit_show_adt_named(
        &mut self,
        obj: IntValue<'ctx>,
        float_mask: IntValue<'ctx>,
        bool_mask: IntValue<'ctx>,
        names: &[String],
    ) -> Result<PointerValue<'ctx>> {
        use inkwell::AddressSpace;
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let mut name_ptrs = Vec::with_capacity(names.len());
        for (i, n) in names.iter().enumerate() {
            let label = if n.is_empty() { "?" } else { n.as_str() };
            let gv = self
                .llvm
                .builder
                .build_global_string_ptr(label, &format!(".adt_name.{i}"))
                .map_err(|e| anyhow::anyhow!("adt name lit: {e}"))?;
            name_ptrs.push(gv.as_pointer_value());
        }
        let arr_ty = ptr_ty.array_type(names.len() as u32);
        let arr = self.llvm.module.add_global(
            arr_ty,
            Some(AddressSpace::default()),
            &format!(".adt_names.{}", names.join("_")),
        );
        arr.set_linkage(inkwell::module::Linkage::Private);
        arr.set_constant(true);
        arr.set_initializer(&ptr_ty.const_array(&name_ptrs));
        let names_ptr = crate::error::llvm(self.llvm.builder.build_pointer_cast(
            arr.as_pointer_value(),
            ptr_ty,
            "adt_names_ptr",
        ))?;
        let n = self.llvm.i64_ty.const_int(names.len() as u64, false);
        let f = self.runtime_fn(SHOW_ADT_NAMED)?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            f,
            &[
                obj.into(),
                float_mask.into(),
                bool_mask.into(),
                names_ptr.into(),
                n.into(),
            ],
            "show_adt_named",
        ))?;
        Ok(call
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_pointer_value())
    }
}

#[cfg(test)]
mod mask_tests {
    use super::Codegen;
    use lumia_ty::Type;

    #[test]
    fn float_mask_rejects_more_than_32_fields() {
        let params: Vec<Type> = (0..33).map(|_| Type::Int).collect();
        let err = Codegen::adt_float_field_mask(&params, &[]).unwrap_err();
        assert!(
            err.to_string().contains("at most 32"),
            "got: {err}"
        );
    }

    #[test]
    fn float_mask_sets_bits_within_32() {
        let lp = [Type::Int, Type::Float, Type::Bool];
        let mask = Codegen::adt_float_field_mask(&lp, &[]).unwrap();
        assert_eq!(mask, 1u64 << 1);
    }

    #[test]
    fn bool_mask_sets_bits_within_32() {
        let lp = [Type::Int, Type::Float, Type::Bool];
        let mask = Codegen::adt_bool_field_mask(&lp, &[]).unwrap();
        assert_eq!(mask, 1u64 << 2);
    }
}

#[cfg(test)]
mod specialized_eq_rt_tests {
    use super::SPECIALIZED_EQ_RT;
    use crate::runtime_decls::runtime_decl_names_for_test;

    #[test]
    fn specialized_eq_rt_symbols_are_declared() {
        let decls = runtime_decl_names_for_test();
        for sym in SPECIALIZED_EQ_RT {
            assert!(decls.contains(sym), "{sym} missing from RUNTIME_DECLS");
        }
    }
}
