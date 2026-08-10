//! Stack (non-escaping) List / Map / ADT layouts.

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::BasicValueEnum;
use lumia_abi::TYPE_ADT;
use lumia_core::Local;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_stack_adt(
        &mut self,
        tag: i64,
        fields: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        let n = fields.len() as u64;
        let payload_bytes = (1 + n) * 8;
        let words = (2 + 1 + n) as u32; // 2 header + tag + fields
        let arr_ty = self.llvm.i64_ty.array_type(words);
        let entry = self
            .frame
            .entry_bb
            .context("emit_stack_adt before emit_function")?;
        let cur = self
            .llvm
            .builder
            .get_insert_block()
            .context("no insert block")?;
        match entry.get_first_instruction() {
            Some(first) => self.llvm.builder.position_before(&first),
            None => self.llvm.builder.position_at_end(entry),
        }
        let storage = crate::error::llvm(self.llvm.builder.build_alloca(arr_ty, "stack_adt"))?;
        self.llvm.builder.position_at_end(cur);

        let type_id = TYPE_ADT as u64;
        let float_mask = self.adt_float_mask_from_fields(fields) as u64;
        let hdr0 = self
            .llvm
            .i64_ty
            .const_int(type_id | (payload_bytes << 32), false);
        let hdr0_slot = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self.llvm.i64_ty.const_int(0, false)],
                    "adt_hdr0",
                )
                .unwrap()
        };
        crate::error::llvm(self.llvm.builder.build_store(hdr0_slot, hdr0))?;
        let hdr1_slot = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self.llvm.i64_ty.const_int(1, false)],
                    "adt_hdr1",
                )
                .unwrap()
        };
        // marked=1 (stack), `_pad` = float field mask
        self.llvm
            .builder
            .build_store(
                hdr1_slot,
                self.llvm.i64_ty.const_int(1 | (float_mask << 32), false),
            )
            .unwrap();

        let payload = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self.llvm.i64_ty.const_int(2, false)],
                    "adt_payload",
                )
                .unwrap()
        };
        self.llvm
            .builder
            .build_store(payload, self.llvm.i64_ty.const_int(tag as u64, false))
            .unwrap();
        for (i, e) in fields.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                self.llvm
                    .builder
                    .build_gep(
                        self.llvm.i64_ty,
                        storage,
                        &[self.llvm.i64_ty.const_int((3 + i) as u64, false)],
                        "adt_f",
                    )
                    .unwrap()
            };
            crate::error::llvm(self.llvm.builder.build_store(slot, v))?;
        }
        Ok(self
            .llvm
            .builder
            .build_ptr_to_int(payload, self.llvm.i64_ty, "adt_stack_i64")
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
        let arr_ty = self.llvm.i64_ty.array_type(words);
        let entry = self
            .frame
            .entry_bb
            .context("emit_stack_array before emit_function")?;
        let cur = self
            .llvm
            .builder
            .get_insert_block()
            .context("no insert block")?;
        match entry.get_first_instruction() {
            Some(first) => self.llvm.builder.position_before(&first),
            None => self.llvm.builder.position_at_end(entry),
        }
        let storage = crate::error::llvm(self.llvm.builder.build_alloca(arr_ty, "stack_arr"))?;
        self.llvm.builder.position_at_end(cur);

        let hdr0 = self
            .llvm
            .i64_ty
            .const_int(type_id | (payload_bytes << 32), false);
        let hdr0_slot = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self.llvm.i64_ty.const_int(0, false)],
                    "sa_hdr0",
                )
                .unwrap()
        };
        crate::error::llvm(self.llvm.builder.build_store(hdr0_slot, hdr0))?;
        let hdr1_slot = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self.llvm.i64_ty.const_int(1, false)],
                    "sa_hdr1",
                )
                .unwrap()
        };
        self.llvm
            .builder
            .build_store(hdr1_slot, self.llvm.i64_ty.const_int(1, false))
            .unwrap();

        let payload = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self.llvm.i64_ty.const_int(2, false)],
                    "sa_payload",
                )
                .unwrap()
        };
        self.llvm
            .builder
            .build_store(payload, self.llvm.i64_ty.const_int(n, false))
            .unwrap();
        for (i, e) in elems.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                self.llvm
                    .builder
                    .build_gep(
                        self.llvm.i64_ty,
                        storage,
                        &[self.llvm.i64_ty.const_int((3 + i) as u64, false)],
                        "sa_elem",
                    )
                    .unwrap()
            };
            crate::error::llvm(self.llvm.builder.build_store(slot, v))?;
        }
        Ok(self
            .llvm
            .builder
            .build_ptr_to_int(payload, self.llvm.i64_ty, "sa_i64")
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
        let arr_ty = self.llvm.i64_ty.array_type(words);
        let entry = self
            .frame
            .entry_bb
            .context("emit_stack_map before emit_function")?;
        let cur = self
            .llvm
            .builder
            .get_insert_block()
            .context("no insert block")?;
        match entry.get_first_instruction() {
            Some(first) => self.llvm.builder.position_before(&first),
            None => self.llvm.builder.position_at_end(entry),
        }
        let storage = crate::error::llvm(self.llvm.builder.build_alloca(arr_ty, "stack_map"))?;
        self.llvm.builder.position_at_end(cur);

        let hdr0 = self
            .llvm
            .i64_ty
            .const_int(type_id | (payload_bytes << 32), false);
        let hdr0_slot = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self.llvm.i64_ty.const_int(0, false)],
                    "sm_hdr0",
                )
                .unwrap()
        };
        crate::error::llvm(self.llvm.builder.build_store(hdr0_slot, hdr0))?;
        let hdr1_slot = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self.llvm.i64_ty.const_int(1, false)],
                    "sm_hdr1",
                )
                .unwrap()
        };
        self.llvm
            .builder
            .build_store(hdr1_slot, self.llvm.i64_ty.const_int(1, false))
            .unwrap();

        let payload = unsafe {
            self.llvm
                .builder
                .build_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self.llvm.i64_ty.const_int(2, false)],
                    "sm_payload",
                )
                .unwrap()
        };
        self.llvm
            .builder
            .build_store(payload, self.llvm.i64_ty.const_int(n_pairs, false))
            .unwrap();
        for (i, e) in flat_pairs.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                self.llvm
                    .builder
                    .build_gep(
                        self.llvm.i64_ty,
                        storage,
                        &[self.llvm.i64_ty.const_int((3 + i) as u64, false)],
                        "sm_kv",
                    )
                    .unwrap()
            };
            crate::error::llvm(self.llvm.builder.build_store(slot, v))?;
        }
        Ok(self
            .llvm
            .builder
            .build_ptr_to_int(payload, self.llvm.i64_ty, "sm_i64")
            .unwrap()
            .into())
    }
}
