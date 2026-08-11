//! Show / Eq / Ord overrides and typed ADT helpers.

use super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, IntValue, PointerValue};
use inkwell::AddressSpace;
use lumia_core::Local;
use lumia_ty::Type;

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

    /// Call `__Show_{T}_show` when an instance provided a custom Show method.
    pub(crate) fn emit_show_override(
        &mut self,
        adt_name: &str,
        arg: BasicValueEnum<'ctx>,
    ) -> Result<Option<PointerValue<'ctx>>> {
        let mangled = lumia_hir::mangle_trait_method("Show", adt_name, "show");
        let Some(fv) = self.funs.functions.get(&mangled).copied() else {
            return Ok(None);
        };
        let i = self.coerce_i64(arg)?;
        let call = crate::error::llvm(self.llvm.builder.build_call(fv, &[i.into()], "show_ov"))?;
        let bits = call
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_int_value();
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
        let mangled = lumia_hir::mangle_trait_method("Eq", adt_name, "eq");
        let Some(fv) = self.funs.functions.get(&mangled).copied() else {
            return Ok(None);
        };
        let call = crate::error::llvm(self.llvm.builder.build_call(
            fv,
            &[left.into(), right.into()],
            "eq_ov",
        ))?;
        Ok(Some(
            call.try_as_basic_value()
                .basic()
                .context("call return value")?
                .into_int_value(),
        ))
    }

    /// Call `__Ord_{T}_less(a,b) -> Bool` when present.
    pub(crate) fn emit_less_override(
        &mut self,
        adt_name: &str,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> Result<Option<IntValue<'ctx>>> {
        let mangled = lumia_hir::mangle_trait_method("Ord", adt_name, "less");
        let Some(fv) = self.funs.functions.get(&mangled).copied() else {
            return Ok(None);
        };
        let call = crate::error::llvm(self.llvm.builder.build_call(
            fv,
            &[left.into(), right.into()],
            "less_ov",
        ))?;
        Ok(Some(
            call.try_as_basic_value()
                .basic()
                .context("call return value")?
                .into_int_value(),
        ))
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
            if let (Type::Adt { params: lp, .. }, Type::Adt { params: rp, .. }) = (lt, rt) {
                if lp.iter().any(|p| matches!(p, Type::Float))
                    || rp.iter().any(|p| matches!(p, Type::Float))
                {
                    return self.emit_typed_adt_eq(l, r, lp, rp);
                }
            }
        }
        let f = self.runtime_fn("lumia_eq")?;
        Ok(
            crate::error::llvm(self.llvm.builder.build_call(f, &[l.into(), r.into()], "eq"))?
                .try_as_basic_value()
                .basic()
                .context("call return value")?
                .into_int_value(),
        )
    }

    /// Bit `i` set ⇒ field `i` uses IEEE eq/show (union of both sides' params).
    pub(crate) fn adt_float_field_mask(lp: &[Type], rp: &[Type]) -> u64 {
        let n = lp.len().max(rp.len()).min(64);
        let mut mask = 0u64;
        for i in 0..n {
            let lf = matches!(lp.get(i), Some(Type::Float));
            let rf = matches!(rp.get(i), Some(Type::Float));
            if lf || rf {
                mask |= 1u64 << i;
            }
        }
        mask
    }

    /// Layout mask from concrete field SSA types at an `AllocAdt` site.
    pub(crate) fn adt_float_mask_from_fields(&self, fields: &[Local]) -> u32 {
        let mut mask = 0u32;
        for (i, f) in fields.iter().enumerate().take(32) {
            if matches!(self.frame.local_tys.get(&f.0), Some(Type::Float)) {
                mask |= 1u32 << i;
            }
        }
        mask
    }

    /// Structural ADT `==` via runtime size (safe for sum None/Ok arity ≠ type params).
    pub(crate) fn emit_typed_adt_eq(
        &mut self,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
        lp: &[Type],
        rp: &[Type],
    ) -> Result<IntValue<'ctx>> {
        let mask = Self::adt_float_field_mask(lp, rp);
        let f = self.runtime_fn("lumia_adt_eq")?;
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
        let mask = Self::adt_float_field_mask(params, &[]);
        let mask_v = self.llvm.i64_ty.const_int(mask, false);
        if let Some(names) = self.funs.adt_variant_names.get(adt_name).cloned() {
            return self.emit_show_adt_named(i, mask_v, &names);
        }
        let f = self.runtime_fn("lumia_show_adt")?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            f,
            &[i.into(), mask_v.into()],
            "show_adt",
        ))?;
        Ok(call
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_pointer_value())
    }

    /// `lumia_show_adt_named(obj, mask, names_ptr, n)`.
    pub(crate) fn emit_show_adt_named(
        &mut self,
        obj: IntValue<'ctx>,
        mask: IntValue<'ctx>,
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
        let f = self.runtime_fn("lumia_show_adt_named")?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            f,
            &[obj.into(), mask.into(), names_ptr.into(), n.into()],
            "show_adt_named",
        ))?;
        Ok(call
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_pointer_value())
    }
}
