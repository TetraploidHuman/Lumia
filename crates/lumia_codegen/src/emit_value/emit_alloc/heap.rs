//! Heap allocations for List / Set / Map / ADT.

use super::super::super::Codegen;
use anyhow::{bail, Context as AnyhowContext, Result};
use inkwell::values::BasicValueEnum;
use inkwell::AddressSpace;
use lumia_abi::{adt_type_id, list_type_id, map_type_id, set_type_id};
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
            .is_some_and(|t| matches!(t, Type::Float));
        let list_tid = list_type_id(float_elems);
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
        repr: lumia_core::SetRepr,
    ) -> Result<BasicValueEnum<'ctx>> {
        let elem_ty = elems
            .first()
            .and_then(|e| self.frame.local_tys.get(&e.0).cloned())
            .unwrap_or(Type::Int);
        let float_elems = matches!(elem_ty, Type::Float);
        let no_hash = !self.key_type_has_hash(&elem_ty);
        let tid = set_type_id(float_elems, no_hash);
        if !elems.is_empty() && matches!(repr, lumia_core::SetRepr::LitSet) {
            return self.emit_stack_array(elems, tid as u64);
        }
        let v = self.emit_heap_array(elems, tid as u64)?;
        if elems.len() > 8 && !no_hash {
            let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
            let bits = self.coerce_i64(v)?;
            let p =
                crate::error::llvm(self.llvm.builder.build_int_to_ptr(bits, ptr_ty, "set_lin"))?;
            let f = self.runtime_fn("lumia_set_finish")?;
            let __call4 =
                crate::error::llvm(self.llvm.builder.build_call(f, &[p.into()], "set_fin"))?;

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
        } else {
            Ok(v)
        }
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
        let key_ty = flat_pairs
            .first()
            .and_then(|k| self.frame.local_tys.get(&k.0).cloned())
            .unwrap_or(Type::Int);
        let val_ty = flat_pairs
            .get(1)
            .and_then(|v| self.frame.local_tys.get(&v.0).cloned())
            .unwrap_or(Type::Int);
        let float_keys = matches!(key_ty, Type::Float);
        let float_vals = matches!(val_ty, Type::Float);
        let no_hash =
            matches!(repr, lumia_core::MapRepr::AssocList) || !self.key_type_has_hash(&key_ty);
        // Float-value tags win over Assoc for IEEE value ==; Assoc is for
        // key Hash absence (linear forever) when values are not Float.
        // AssocList (+ Float tags) stays linear forever; Hash maps use 4/10/15/16.
        let tid = map_type_id(float_keys, float_vals, no_hash);
        if n_pairs > 0 && matches!(repr, lumia_core::MapRepr::LitMap) {
            return self.emit_stack_map(flat_pairs, tid as u64);
        }
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
            let skip = if i.is_multiple_of(2) {
                float_keys
            } else {
                float_vals
            };
            if !skip {
                self.emit_write_barrier(ptr, i as u32, v)?;
            }
        }
        let ptr = if !no_hash && (n_pairs > 8 || matches!(repr, lumia_core::MapRepr::HashOrdered)) {
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
        let float_mask = self.adt_float_mask_from_fields(fields);
        if float_mask != 0 {
            let setm = self
                .llvm
                .module
                .get_function("lumia_adt_set_float_mask")
                .context("module function")?;
            let m = self
                .llvm
                .context
                .i32_type()
                .const_int(float_mask as u64, false);
            crate::error::llvm(self.llvm.builder.build_call(
                setm,
                &[ptr.into(), m.into()],
                "adt_fmask",
            ))?;
        }
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
            self.emit_write_barrier(ptr, i as u32, v)?;
        }
        Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
            ptr,
            self.llvm.i64_ty,
            "adt_as_i64",
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
            // Unboxed Float elems are not GC pointers (see float_contract).
            if !lumia_abi::list_elem_is_float(type_id as u32)
                && !lumia_abi::set_elem_is_float(type_id as u32)
            {
                self.emit_write_barrier(ptr, i as u32, v)?;
            }
        }
        Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
            ptr,
            self.llvm.i64_ty,
            "arr_as_i64",
        ))?
        .into())
    }
}
