//! Heap allocations for List / Set / Map / ADT.

use super::super::super::Codegen;
use anyhow::{bail, Context as AnyhowContext, Result};
use inkwell::values::BasicValueEnum;
use inkwell::AddressSpace;
use lumia_abi::{adt_type_id, list_type_id_flags, map_type_id_flags, set_type_id_flags};
use lumia_core::Local;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_value_alloc_list(
        &mut self,
        elems: &[Local],
        repr: lumia_core::ListRepr,
    ) -> Result<BasicValueEnum<'ctx>> {
        // Empty → immortal singleton. Non-escaping LitList → stack header+payload
        // (same layout as heap so RT len/get work). Escaping → heap.
        let float_elems = elems
            .first()
            .and_then(|e| self.frame.local_tys.get(&e.0).cloned())
            .map(|t| matches!(t, Type::Float))
            .unwrap_or_else(|| {
                matches!(
                    &self.frame.expect_alloc_ty,
                    Some(Type::List(e)) if matches!(e.as_ref(), Type::Float)
                )
            });
        let bool_elems = !float_elems
            && elems
                .first()
                .and_then(|e| self.frame.local_tys.get(&e.0).cloned())
                .map(|t| matches!(t, Type::Bool))
                .unwrap_or_else(|| {
                    matches!(
                        &self.frame.expect_alloc_ty,
                        Some(Type::List(e)) if matches!(e.as_ref(), Type::Bool)
                    )
                });
        let list_tid = list_type_id_flags(float_elems, bool_elems);
        if elems.is_empty() {
            if float_elems {
                let ens = self.runtime_fn(lumia_abi::ENSURE_LIST_F64)?;
                let f = self.runtime_fn("lumia_list_empty")?;
                let __call1 =
                    crate::error::llvm(self.llvm.builder.build_call(f, &[], "list_empty"))?;

                let empty = __call1
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                let __call2 = crate::error::llvm(self.llvm.builder.build_call(
                    ens,
                    &[empty.into()],
                    "ens_lf64",
                ))?;

                let ptr = __call2
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                return Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "empty_f64_i64",
                ))?
                .into());
            }
            if bool_elems {
                let ens = self.runtime_fn(lumia_abi::ENSURE_LIST_BOOL)?;
                let f = self.runtime_fn("lumia_list_empty")?;
                let empty_call =
                    crate::error::llvm(self.llvm.builder.build_call(f, &[], "list_empty"))?;
                let empty = empty_call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                let ens_call = crate::error::llvm(self.llvm.builder.build_call(
                    ens,
                    &[empty.into()],
                    "ens_lbool",
                ))?;
                let ptr = ens_call
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
                return Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                    ptr,
                    self.llvm.i64_ty,
                    "empty_bool_i64",
                ))?
                .into());
            }
            let f = self.runtime_fn("lumia_list_empty")?;
            let __call3 = crate::error::llvm(self.llvm.builder.build_call(f, &[], "list_empty"))?;

            let ptr = __call3
                .try_as_basic_value()
                .basic()
                .context("call return value")?
                .into_pointer_value();
            return Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                ptr,
                self.llvm.i64_ty,
                "empty_i64",
            ))?
            .into());
        }
        if matches!(repr, lumia_core::ListRepr::LitList) {
            return self.emit_stack_array(elems, list_tid as u64);
        }
        self.emit_heap_array(elems, list_tid as u64)
    }

    pub(crate) fn emit_value_alloc_set(
        &mut self,
        elems: &[Local],
        _repr: lumia_core::SetRepr,
    ) -> Result<BasicValueEnum<'ctx>> {
        // Empty Set → immortal singleton (like `listOf()`); null still accepted by RT.
        if elems.is_empty() {
            let f = self.runtime_fn("lumia_set_empty")?;
            let empty_call =
                crate::error::llvm(self.llvm.builder.build_call(f, &[], "set_empty"))?;
            let mut ptr = empty_call
                .try_as_basic_value()
                .basic()
                .context("call return value")?
                .into_pointer_value();
            let elem_ty = match &self.frame.expect_alloc_ty {
                Some(Type::Set(e)) => e.as_ref().clone(),
                _ => Type::Int,
            };
            if matches!(elem_ty, Type::Float) {
                let ens = self.runtime_fn(lumia_abi::ENSURE_SET_F64)?;
                let c = crate::error::llvm(self.llvm.builder.build_call(
                    ens,
                    &[ptr.into()],
                    "ens_sf64",
                ))?;
                ptr = c
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
            } else if matches!(elem_ty, Type::Bool) {
                let ens = self.runtime_fn(lumia_abi::ENSURE_SET_BOOL)?;
                let c = crate::error::llvm(self.llvm.builder.build_call(
                    ens,
                    &[ptr.into()],
                    "ens_sbool",
                ))?;
                ptr = c
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
            }
            return Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                ptr,
                self.llvm.i64_ty,
                "empty_set_i64",
            ))?
            .into());
        }
        let elem_ty = elems
            .first()
            .and_then(|e| self.frame.local_tys.get(&e.0).cloned())
            .or_else(|| match &self.frame.expect_alloc_ty {
                Some(Type::Set(e)) => Some(e.as_ref().clone()),
                _ => None,
            })
            .unwrap_or(Type::Int);
        let float_elems = matches!(elem_ty, Type::Float);
        let bool_elems = matches!(elem_ty, Type::Bool);
        let no_hash = !self.key_type_has_hash(&elem_ty);
        let tid = set_type_id_flags(float_elems, bool_elems, no_hash);
        // `SetRepr::LitSet` is a PE/hint tag only — never a stack layout. Always
        // heap+finish so `lumia_set_finish` can compact via `key_eq`.
        let v = self.emit_heap_array(elems, tid as u64)?;
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let bits = self.coerce_i64(v)?;
        let p = crate::error::llvm(self.llvm.builder.build_int_to_ptr(bits, ptr_ty, "set_lin"))?;
        let f = self.runtime_fn("lumia_set_finish")?;
        let __call4 = crate::error::llvm(self.llvm.builder.build_call(f, &[p.into()], "set_fin"))?;

        let out = __call4
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_pointer_value();
        Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
            out,
            self.llvm.i64_ty,
            "set_i64",
        ))?
        .into())
    }

    pub(crate) fn emit_value_alloc_map(
        &mut self,
        flat_pairs: &[Local],
        repr: lumia_core::MapRepr,
    ) -> Result<BasicValueEnum<'ctx>> {
        // Layout: [len][k0][v0]... — len = pair count
        if !flat_pairs.len().is_multiple_of(2) {
            bail!("mapOf expects even number of key/value args");
        }
        let n_pairs = (flat_pairs.len() / 2) as u64;
        let (key_ty, val_ty) = if flat_pairs.len() >= 2 {
            (
                flat_pairs
                    .first()
                    .and_then(|k| self.frame.local_tys.get(&k.0).cloned())
                    .unwrap_or(Type::Int),
                flat_pairs
                    .get(1)
                    .and_then(|v| self.frame.local_tys.get(&v.0).cloned())
                    .unwrap_or(Type::Int),
            )
        } else {
            match &self.frame.expect_alloc_ty {
                Some(Type::Map(k, v)) => (k.as_ref().clone(), v.as_ref().clone()),
                _ => (Type::Int, Type::Int),
            }
        };
        let float_keys = matches!(key_ty, Type::Float);
        let float_vals = matches!(val_ty, Type::Float);
        let bool_keys = matches!(key_ty, Type::Bool);
        let bool_vals = matches!(val_ty, Type::Bool);
        let no_hash =
            matches!(repr, lumia_core::MapRepr::AssocList) || !self.key_type_has_hash(&key_ty);
        // Empty Map → immortal singleton (like `listOf()`); null still accepted by RT.
        if flat_pairs.is_empty() {
            let f = self.runtime_fn("lumia_map_empty")?;
            let empty_call =
                crate::error::llvm(self.llvm.builder.build_call(f, &[], "map_empty"))?;
            let mut ptr = empty_call
                .try_as_basic_value()
                .basic()
                .context("call return value")?
                .into_pointer_value();
            if float_keys {
                let ens = self.runtime_fn(lumia_abi::ENSURE_MAP_F64)?;
                let c = crate::error::llvm(self.llvm.builder.build_call(
                    ens,
                    &[ptr.into()],
                    "ens_mf64",
                ))?;
                ptr = c
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
            }
            if float_vals {
                let ens = self.runtime_fn(lumia_abi::ENSURE_MAP_VF64)?;
                let c = crate::error::llvm(self.llvm.builder.build_call(
                    ens,
                    &[ptr.into()],
                    "ens_mvf64",
                ))?;
                ptr = c
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
            }
            if bool_keys {
                let ens = self.runtime_fn(lumia_abi::ENSURE_MAP_BOOL)?;
                let c = crate::error::llvm(self.llvm.builder.build_call(
                    ens,
                    &[ptr.into()],
                    "ens_mbool",
                ))?;
                ptr = c
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
            }
            if bool_vals {
                let ens = self.runtime_fn(lumia_abi::ENSURE_MAP_VBOOL)?;
                let c = crate::error::llvm(self.llvm.builder.build_call(
                    ens,
                    &[ptr.into()],
                    "ens_mvbool",
                ))?;
                ptr = c
                    .try_as_basic_value()
                    .basic()
                    .context("call return value")?
                    .into_pointer_value();
            }
            return Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
                ptr,
                self.llvm.i64_ty,
                "empty_map_i64",
            ))?
            .into());
        }
        // Float-value tags win over Assoc for IEEE value ==; Assoc is for
        // key Hash absence (linear forever) when values are not Float.
        // AssocList (+ Float tags) stays linear forever; Hash maps use 4/10/15/16.
        let tid = map_type_id_flags(float_keys, float_vals, bool_keys, bool_vals, no_hash);
        // `MapRepr::LitMap` is a PE/hint tag only — never a stack layout. Always
        // heap+finish so `lumia_map_finish` can compact (Float ±0 included).
        let nbytes = self
            .llvm
            .i64_ty
            .const_int((1 + flat_pairs.len() as u64) * 8, false);
        let type_id = self.llvm.context.i32_type().const_int(tid as u64, false);
        let alloc = self.runtime_fn("lumia_alloc")?;
        let __call5 = crate::error::llvm(self.llvm.builder.build_call(
            alloc,
            &[nbytes.into(), type_id.into()],
            "map_alloc",
        ))?;

        let ptr = __call5
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_pointer_value();
        let len_slot = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                ptr,
                &[self.llvm.i64_ty.const_int(0, false)],
                "len_slot",
            ))?
        };
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(len_slot, self.llvm.i64_ty.const_int(n_pairs, false)),
        )?;
        for (i, e) in flat_pairs.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                    self.llvm.i64_ty,
                    ptr,
                    &[self.llvm.i64_ty.const_int((i + 1) as u64, false)],
                    "kv",
                ))?
            };
            crate::error::llvm(self.llvm.builder.build_store(slot, v))?;
            // Map holds a COW alias of nested List/Map/Set/ADT keys/values.
            if let Some(ty) = self.frame.local_tys.get(&e.0) {
                if Self::type_needs_cow_retain(ty) {
                    self.adt_retain_i64(v)?;
                }
            }
            // Young alloc: init stores need no write barrier.
        }
        let ptr = if n_pairs > 0 {
            let f = self.runtime_fn("lumia_map_finish")?;
            crate::error::llvm(self.llvm.builder.build_call(f, &[ptr.into()], "map_fin"))?
                .try_as_basic_value()
                .basic()
                .context("call return value")?
                .into_pointer_value()
        } else {
            ptr
        };
        Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
            ptr,
            self.llvm.i64_ty,
            "map_as_i64",
        ))?
        .into())
    }

    pub(crate) fn emit_value_alloc_adt(
        &mut self,
        adt_name: &str,
        tag: i64,
        fields: &[Local],
        repr: lumia_core::AdtRepr,
    ) -> Result<BasicValueEnum<'ctx>> {
        if matches!(repr, lumia_core::AdtRepr::LitAdt) {
            return self.emit_stack_adt(adt_name, tag, fields);
        }
        // `slot = slot with { f = … }` on a heap product: consume alias + field stores.
        if let Some((slot, updates)) = self.frame.adt_with_inplace.take() {
            return self.emit_adt_with_inplace(&slot, &updates);
        }
        let n = fields.len() as u64;
        let nbytes = self.llvm.i64_ty.const_int((1 + n) * 8, false);
        let kind = self.funs.adt_show_kinds.get(adt_name).copied().unwrap_or(0);
        let type_id = self
            .llvm
            .context
            .i32_type()
            .const_int(adt_type_id(kind) as u64, false);
        let alloc = self.runtime_fn("lumia_alloc")?;
        let __call6 = crate::error::llvm(self.llvm.builder.build_call(
            alloc,
            &[nbytes.into(), type_id.into()],
            "adt_alloc",
        ))?;

        let ptr = __call6
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_pointer_value();
        let tag_slot = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                ptr,
                &[self.llvm.i64_ty.const_int(0, false)],
                "tag_slot",
            ))?
        };
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(tag_slot, self.llvm.i64_ty.const_int(tag as u64, false)),
        )?;
        for (i, e) in fields.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                    self.llvm.i64_ty,
                    ptr,
                    &[self.llvm.i64_ty.const_int((i + 1) as u64, false)],
                    "adt_f",
                ))?
            };
            crate::error::llvm(self.llvm.builder.build_store(slot, v))?;
            // Parent holds a COW alias of nested List/ADT fields.
            if let Some(ty) = self.frame.local_tys.get(&e.0) {
                if Self::type_needs_cow_retain(ty) {
                    self.adt_retain_i64(v)?;
                }
            }
            // Young alloc: init stores need no write barrier.
        }
        // After fields are live: set masks, clearing bits that actually hold heap ptrs.
        let float_mask = self.adt_float_mask_from_fields(fields)?;
        self.emit_adt_set_float_mask(ptr, float_mask)?;
        let bool_mask = self.adt_bool_mask_from_fields(fields)?;
        self.emit_adt_set_bool_mask(ptr, bool_mask)?;
        Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
            ptr,
            self.llvm.i64_ty,
            "adt_as_i64",
        ))?
        .into())
    }

    /// Unique / COW path for `slot = slot with { … }` (heap products only).
    fn emit_adt_with_inplace(
        &mut self,
        slot: &str,
        updates: &[(u32, Local)],
    ) -> Result<BasicValueEnum<'ctx>> {
        let loaded = self.load_slot(slot)?;
        let raw = self.coerce_i64(loaded)?;
        let ptr0 = self.i64_as_ptr(raw, "adt_with_base")?;
        // Drop the with-temp `Name(slot)` retain from bind_let, then unique-check.
        // Extra aliases (`val snap = p`) keep RC ≥ 2 → clone.
        // Overwrite mask: skip nested retain on fields we rewrite (brother buffers).
        let mut overwrite_mask = 0u64;
        for &(idx, _) in updates {
            if idx >= 64 {
                bail!(
                    "ICE: ADT field index {idx} exceeds 64-bit overwrite mask (with-update)"
                );
            }
            overwrite_mask |= 1u64 << idx;
        }
        let ensure = self.runtime_fn("lumia_adt_ensure_unique_consume_mask")?;
        let mask_v = self.llvm.i64_ty.const_int(overwrite_mask, false);
        let ptr = crate::error::llvm(self.llvm.builder.build_call(
            ensure,
            &[ptr0.into(), mask_v.into()],
            "adt_uniq",
        ))?
        .try_as_basic_value()
        .basic()
        .context("ensure_unique_consume_mask return")?
        .into_pointer_value();
        let setf = self.runtime_fn("lumia_adt_set_field")?;
        for &(idx, loc) in updates {
            let v = self.coerce_i64(self.local(loc)?)?;
            let i = self.llvm.i64_ty.const_int(idx as u64, false);
            crate::error::llvm(self.llvm.builder.build_call(
                setf,
                &[ptr.into(), i.into(), v.into()],
                "adt_set_f",
            ))?;
        }
        Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
            ptr,
            self.llvm.i64_ty,
            "adt_with_i64",
        ))?
        .into())
    }

    pub(crate) fn emit_heap_array(
        &mut self,
        elems: &[Local],
        type_id: u64,
    ) -> Result<BasicValueEnum<'ctx>> {
        let n = elems.len() as u64;
        let nbytes = self.llvm.i64_ty.const_int((1 + n) * 8, false);
        let tid_const = self.llvm.context.i32_type().const_int(type_id, false);
        let alloc = self.runtime_fn("lumia_alloc")?;
        let __call7 = crate::error::llvm(self.llvm.builder.build_call(
            alloc,
            &[nbytes.into(), tid_const.into()],
            "arr_alloc",
        ))?;

        let ptr = __call7
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_pointer_value();
        let len_slot = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                ptr,
                &[self.llvm.i64_ty.const_int(0, false)],
                "len_slot",
            ))?
        };
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(len_slot, self.llvm.i64_ty.const_int(n, false)),
        )?;
        for (i, e) in elems.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                    self.llvm.i64_ty,
                    ptr,
                    &[self.llvm.i64_ty.const_int((i + 1) as u64, false)],
                    "elem",
                ))?
            };
            crate::error::llvm(self.llvm.builder.build_store(slot, v))?;
            // List/Set holds a COW alias of nested List/Map/Set/ADT elems.
            if let Some(ty) = self.frame.local_tys.get(&e.0) {
                if Self::type_needs_cow_retain(ty) {
                    self.adt_retain_i64(v)?;
                }
            }
            // Young alloc: init stores need no write barrier (Float elems are
            // non-pointers anyway; see float_contract).
        }
        Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
            ptr,
            self.llvm.i64_ty,
            "arr_as_i64",
        ))?
        .into())
    }
}
