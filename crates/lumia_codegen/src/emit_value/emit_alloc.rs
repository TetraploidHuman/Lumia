//! Value emission — allocations and stack/heap helpers

use super::super::Codegen;
use anyhow::{bail, Context as AnyhowContext, Result};
use inkwell::values::BasicValueEnum;
use inkwell::AddressSpace;
use lumia_abi::{list_type_id, map_type_id, set_type_id, TYPE_ADT};
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
            .and_then(|e| self.local_tys.get(&e.0).cloned())
            .is_some_and(|t| matches!(t, Type::Float));
        let list_tid = list_type_id(float_elems);
        if elems.is_empty() {
            if float_elems {
                let ens = self.module.get_function("lumia_ensure_list_f64").unwrap();
                let f = self.module.get_function("lumia_list_empty").unwrap();
                let empty = self
                    .builder
                    .build_call(f, &[], "list_empty")
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                let ptr = self
                    .builder
                    .build_call(ens, &[empty.into()], "ens_lf64")
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                return Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "empty_f64_i64")
                    .unwrap()
                    .into());
            }
            let f = self.module.get_function("lumia_list_empty").unwrap();
            let ptr = self
                .builder
                .build_call(f, &[], "list_empty")
                .unwrap()
                .try_as_basic_value()
                .basic()
                .unwrap()
                .into_pointer_value();
            return Ok(self
                .builder
                .build_ptr_to_int(ptr, self.i64_ty, "empty_i64")
                .unwrap()
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
            .and_then(|e| self.local_tys.get(&e.0).cloned())
            .unwrap_or(Type::Int);
        let float_elems = matches!(elem_ty, Type::Float);
        let no_hash = !self.key_type_has_hash(&elem_ty);
        let tid = set_type_id(float_elems, no_hash);
        if !elems.is_empty() && matches!(repr, lumia_core::SetRepr::LitSet) {
            return self.emit_stack_array(elems, tid as u64);
        }
        let v = self.emit_heap_array(elems, tid as u64)?;
        if elems.len() > 8 && !no_hash {
            let ptr_ty = self.context.ptr_type(AddressSpace::default());
            let bits = self.coerce_i64(v)?;
            let p = self
                .builder
                .build_int_to_ptr(bits, ptr_ty, "set_lin")
                .unwrap();
            let f = self.module.get_function("lumia_set_finish").unwrap();
            let out = self
                .builder
                .build_call(f, &[p.into()], "set_fin")
                .unwrap()
                .try_as_basic_value()
                .basic()
                .unwrap()
                .into_pointer_value();
            Ok(self
                .builder
                .build_ptr_to_int(out, self.i64_ty, "set_i64")
                .unwrap()
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
            .and_then(|k| self.local_tys.get(&k.0).cloned())
            .unwrap_or(Type::Int);
        let val_ty = flat_pairs
            .get(1)
            .and_then(|v| self.local_tys.get(&v.0).cloned())
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
            .i64_ty
            .const_int((1 + flat_pairs.len() as u64) * 8, false);
        let type_id = self.context.i32_type().const_int(tid as u64, false);
        let alloc = self.module.get_function("lumia_alloc").unwrap();
        let ptr = self
            .builder
            .build_call(alloc, &[nbytes.into(), type_id.into()], "map_alloc")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        let len_slot = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    ptr,
                    &[self.i64_ty.const_int(0, false)],
                    "len_slot",
                )
                .unwrap()
        };
        self.builder
            .build_store(len_slot, self.i64_ty.const_int(n_pairs, false))
            .unwrap();
        for (i, e) in flat_pairs.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                self.builder
                    .build_gep(
                        self.i64_ty,
                        ptr,
                        &[self.i64_ty.const_int((i + 1) as u64, false)],
                        "kv",
                    )
                    .unwrap()
            };
            self.builder.build_store(slot, v).unwrap();
        }
        let ptr = if !no_hash && (n_pairs > 8 || matches!(repr, lumia_core::MapRepr::HashOrdered)) {
            let f = self.module.get_function("lumia_map_finish").unwrap();
            self.builder
                .build_call(f, &[ptr.into()], "map_fin")
                .unwrap()
                .try_as_basic_value()
                .basic()
                .unwrap()
                .into_pointer_value()
        } else {
            ptr
        };
        Ok(self
            .builder
            .build_ptr_to_int(ptr, self.i64_ty, "map_as_i64")
            .unwrap()
            .into())
    }

    pub(crate) fn emit_value_alloc_adt(
        &mut self,
        _adt_name: &str,
        tag: i64,
        fields: &[Local],
        repr: lumia_core::AdtRepr,
    ) -> Result<BasicValueEnum<'ctx>> {
        if matches!(repr, lumia_core::AdtRepr::LitAdt) {
            return self.emit_stack_adt(tag, fields);
        }
        let n = fields.len() as u64;
        let nbytes = self.i64_ty.const_int((1 + n) * 8, false);
        let type_id = self.context.i32_type().const_int(TYPE_ADT as u64, false);
        let alloc = self.module.get_function("lumia_alloc").unwrap();
        let ptr = self
            .builder
            .build_call(alloc, &[nbytes.into(), type_id.into()], "adt_alloc")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        let float_mask = self.adt_float_mask_from_fields(fields);
        if float_mask != 0 {
            let setm = self
                .module
                .get_function("lumia_adt_set_float_mask")
                .unwrap();
            let m = self.context.i32_type().const_int(float_mask as u64, false);
            self.builder
                .build_call(setm, &[ptr.into(), m.into()], "adt_fmask")
                .unwrap();
        }
        let tag_slot = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    ptr,
                    &[self.i64_ty.const_int(0, false)],
                    "tag_slot",
                )
                .unwrap()
        };
        self.builder
            .build_store(tag_slot, self.i64_ty.const_int(tag as u64, false))
            .unwrap();
        for (i, e) in fields.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                self.builder
                    .build_gep(
                        self.i64_ty,
                        ptr,
                        &[self.i64_ty.const_int((i + 1) as u64, false)],
                        "adt_f",
                    )
                    .unwrap()
            };
            self.builder.build_store(slot, v).unwrap();
        }
        Ok(self
            .builder
            .build_ptr_to_int(ptr, self.i64_ty, "adt_as_i64")
            .unwrap()
            .into())
    }

    pub(crate) fn emit_stack_adt(
        &mut self,
        tag: i64,
        fields: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        let n = fields.len() as u64;
        let payload_bytes = (1 + n) * 8;
        let words = (2 + 1 + n) as u32; // 2 header + tag + fields
        let arr_ty = self.i64_ty.array_type(words);
        let entry = self
            .entry_bb
            .context("emit_stack_adt before emit_function")?;
        let cur = self.builder.get_insert_block().context("no insert block")?;
        match entry.get_first_instruction() {
            Some(first) => self.builder.position_before(&first),
            None => self.builder.position_at_end(entry),
        }
        let storage = self.builder.build_alloca(arr_ty, "stack_adt").unwrap();
        self.builder.position_at_end(cur);

        let type_id = TYPE_ADT as u64;
        let float_mask = self.adt_float_mask_from_fields(fields) as u64;
        let hdr0 = self
            .i64_ty
            .const_int(type_id | (payload_bytes << 32), false);
        let hdr0_slot = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    storage,
                    &[self.i64_ty.const_int(0, false)],
                    "adt_hdr0",
                )
                .unwrap()
        };
        self.builder.build_store(hdr0_slot, hdr0).unwrap();
        let hdr1_slot = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    storage,
                    &[self.i64_ty.const_int(1, false)],
                    "adt_hdr1",
                )
                .unwrap()
        };
        // marked=1 (stack), `_pad` = float field mask
        self.builder
            .build_store(
                hdr1_slot,
                self.i64_ty.const_int(1 | (float_mask << 32), false),
            )
            .unwrap();

        let payload = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    storage,
                    &[self.i64_ty.const_int(2, false)],
                    "adt_payload",
                )
                .unwrap()
        };
        self.builder
            .build_store(payload, self.i64_ty.const_int(tag as u64, false))
            .unwrap();
        for (i, e) in fields.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                self.builder
                    .build_gep(
                        self.i64_ty,
                        storage,
                        &[self.i64_ty.const_int((3 + i) as u64, false)],
                        "adt_f",
                    )
                    .unwrap()
            };
            self.builder.build_store(slot, v).unwrap();
        }
        Ok(self
            .builder
            .build_ptr_to_int(payload, self.i64_ty, "adt_stack_i64")
            .unwrap()
            .into())
    }

    /// Stack Set/List-shaped array: ObjectHeader + `[len][elems…]`.
    pub(crate) fn emit_stack_array(
        &mut self,
        elems: &[Local],
        type_id: u64,
    ) -> Result<BasicValueEnum<'ctx>> {
        let n = elems.len() as u64;
        let payload_bytes = (1 + n) * 8;
        let words = (2 + 1 + n) as u32; // 2 header words + len + elems
        let arr_ty = self.i64_ty.array_type(words);
        let entry = self
            .entry_bb
            .context("emit_stack_array before emit_function")?;
        let cur = self.builder.get_insert_block().context("no insert block")?;
        match entry.get_first_instruction() {
            Some(first) => self.builder.position_before(&first),
            None => self.builder.position_at_end(entry),
        }
        let storage = self.builder.build_alloca(arr_ty, "stack_arr").unwrap();
        self.builder.position_at_end(cur);

        let hdr0 = self
            .i64_ty
            .const_int(type_id | (payload_bytes << 32), false);
        let hdr0_slot = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    storage,
                    &[self.i64_ty.const_int(0, false)],
                    "sa_hdr0",
                )
                .unwrap()
        };
        self.builder.build_store(hdr0_slot, hdr0).unwrap();
        let hdr1_slot = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    storage,
                    &[self.i64_ty.const_int(1, false)],
                    "sa_hdr1",
                )
                .unwrap()
        };
        self.builder
            .build_store(hdr1_slot, self.i64_ty.const_int(1, false))
            .unwrap();

        let payload = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    storage,
                    &[self.i64_ty.const_int(2, false)],
                    "sa_payload",
                )
                .unwrap()
        };
        self.builder
            .build_store(payload, self.i64_ty.const_int(n, false))
            .unwrap();
        for (i, e) in elems.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                self.builder
                    .build_gep(
                        self.i64_ty,
                        storage,
                        &[self.i64_ty.const_int((3 + i) as u64, false)],
                        "sa_elem",
                    )
                    .unwrap()
            };
            self.builder.build_store(slot, v).unwrap();
        }
        Ok(self
            .builder
            .build_ptr_to_int(payload, self.i64_ty, "sa_i64")
            .unwrap()
            .into())
    }

    /// Stack Map: ObjectHeader + `[n_pairs][k0][v0]…`.
    pub(crate) fn emit_stack_map(
        &mut self,
        flat_pairs: &[Local],
        type_id: u64,
    ) -> Result<BasicValueEnum<'ctx>> {
        let n_words = flat_pairs.len() as u64;
        let n_pairs = n_words / 2;
        let payload_bytes = (1 + n_words) * 8;
        let words = (2 + 1 + n_words) as u32;
        let arr_ty = self.i64_ty.array_type(words);
        let entry = self
            .entry_bb
            .context("emit_stack_map before emit_function")?;
        let cur = self.builder.get_insert_block().context("no insert block")?;
        match entry.get_first_instruction() {
            Some(first) => self.builder.position_before(&first),
            None => self.builder.position_at_end(entry),
        }
        let storage = self.builder.build_alloca(arr_ty, "stack_map").unwrap();
        self.builder.position_at_end(cur);

        let hdr0 = self
            .i64_ty
            .const_int(type_id | (payload_bytes << 32), false);
        let hdr0_slot = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    storage,
                    &[self.i64_ty.const_int(0, false)],
                    "sm_hdr0",
                )
                .unwrap()
        };
        self.builder.build_store(hdr0_slot, hdr0).unwrap();
        let hdr1_slot = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    storage,
                    &[self.i64_ty.const_int(1, false)],
                    "sm_hdr1",
                )
                .unwrap()
        };
        self.builder
            .build_store(hdr1_slot, self.i64_ty.const_int(1, false))
            .unwrap();

        let payload = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    storage,
                    &[self.i64_ty.const_int(2, false)],
                    "sm_payload",
                )
                .unwrap()
        };
        self.builder
            .build_store(payload, self.i64_ty.const_int(n_pairs, false))
            .unwrap();
        for (i, e) in flat_pairs.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                self.builder
                    .build_gep(
                        self.i64_ty,
                        storage,
                        &[self.i64_ty.const_int((3 + i) as u64, false)],
                        "sm_kv",
                    )
                    .unwrap()
            };
            self.builder.build_store(slot, v).unwrap();
        }
        Ok(self
            .builder
            .build_ptr_to_int(payload, self.i64_ty, "sm_i64")
            .unwrap()
            .into())
    }

    pub(crate) fn emit_heap_array(
        &mut self,
        elems: &[Local],
        type_id: u64,
    ) -> Result<BasicValueEnum<'ctx>> {
        let n = elems.len() as u64;
        let nbytes = self.i64_ty.const_int((1 + n) * 8, false);
        let type_id = self.context.i32_type().const_int(type_id, false);
        let alloc = self.module.get_function("lumia_alloc").unwrap();
        let ptr = self
            .builder
            .build_call(alloc, &[nbytes.into(), type_id.into()], "arr_alloc")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        let len_slot = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    ptr,
                    &[self.i64_ty.const_int(0, false)],
                    "len_slot",
                )
                .unwrap()
        };
        self.builder
            .build_store(len_slot, self.i64_ty.const_int(n, false))
            .unwrap();
        for (i, e) in elems.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                self.builder
                    .build_gep(
                        self.i64_ty,
                        ptr,
                        &[self.i64_ty.const_int((i + 1) as u64, false)],
                        "elem",
                    )
                    .unwrap()
            };
            self.builder.build_store(slot, v).unwrap();
        }
        Ok(self
            .builder
            .build_ptr_to_int(ptr, self.i64_ty, "arr_as_i64")
            .unwrap()
            .into())
    }
}
