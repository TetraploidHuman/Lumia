//! Show / Eq / Ord overrides and typed ADT helpers.

use super::Codegen;
use anyhow::Result;
use inkwell::values::{BasicValueEnum, IntValue, PointerValue};
use inkwell::AddressSpace;
use lumia_core::Local;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn key_type_has_hash(&self, ty: &Type) -> bool {
        match ty {
            Type::Adt { name, .. } => self.hash_adts.contains(name),
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
        let mangled = format!("__Show_{adt_name}_show");
        let Some(fv) = self.functions.get(&mangled).copied() else {
            return Ok(None);
        };
        let i = self.coerce_i64(arg)?;
        let call = self.builder.build_call(fv, &[i.into()], "show_ov").unwrap();
        let bits = call.try_as_basic_value().basic().unwrap().into_int_value();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let ptr = self
            .builder
            .build_int_to_ptr(bits, ptr_ty, "show_ov_ptr")
            .unwrap();
        Ok(Some(ptr))
    }

    /// Call `__Eq_{T}_eq(a,b) -> Bool` when present.
    pub(crate) fn emit_eq_override(
        &mut self,
        adt_name: &str,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> Result<Option<IntValue<'ctx>>> {
        let mangled = format!("__Eq_{adt_name}_eq");
        let Some(fv) = self.functions.get(&mangled).copied() else {
            return Ok(None);
        };
        let call = self
            .builder
            .build_call(fv, &[left.into(), right.into()], "eq_ov")
            .unwrap();
        Ok(Some(
            call.try_as_basic_value().basic().unwrap().into_int_value(),
        ))
    }

    /// Call `__Ord_{T}_less(a,b) -> Bool` when present.
    pub(crate) fn emit_less_override(
        &mut self,
        adt_name: &str,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> Result<Option<IntValue<'ctx>>> {
        let mangled = format!("__Ord_{adt_name}_less");
        let Some(fv) = self.functions.get(&mangled).copied() else {
            return Ok(None);
        };
        let call = self
            .builder
            .build_call(fv, &[left.into(), right.into()], "less_ov")
            .unwrap();
        Ok(Some(
            call.try_as_basic_value().basic().unwrap().into_int_value(),
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
        if let Some(name) = Self::adt_method_name(lt, rt) {
            // Hash ADTs use `lumia_eq` for Map/Set keys — keep `==` on the same path
            // so a custom `__Eq_*_eq` cannot diverge from containment.
            if !self.hash_adts.contains(&name) {
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
        let f = self.module.get_function("lumia_eq").unwrap();
        Ok(self
            .builder
            .build_call(f, &[l.into(), r.into()], "eq")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_int_value())
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
            if matches!(self.local_tys.get(&f.0), Some(Type::Float)) {
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
        let f = self.module.get_function("lumia_adt_eq").unwrap();
        Ok(self
            .builder
            .build_call(
                f,
                &[
                    left.into(),
                    right.into(),
                    self.i64_ty.const_int(mask, false).into(),
                ],
                "adt_eq",
            )
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_int_value())
    }

    /// Structural ADT show; float_mask selects IEEE formatting per field index.
    pub(crate) fn emit_typed_adt_show(
        &mut self,
        arg: BasicValueEnum<'ctx>,
        params: &[Type],
    ) -> Result<PointerValue<'ctx>> {
        let i = self.coerce_i64(arg)?;
        let mask = Self::adt_float_field_mask(params, &[]);
        let f = self.module.get_function("lumia_show_adt").unwrap();
        Ok(self
            .builder
            .build_call(
                f,
                &[i.into(), self.i64_ty.const_int(mask, false).into()],
                "show_adt",
            )
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value())
    }
}
