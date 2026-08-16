//! Stack (non-escaping) List / Map / ADT layouts.
//!
//! ObjectHeader is [`lumia_abi::OBJECT_HEADER_BYTES`] ⇒
//! [`lumia_abi::OBJECT_HEADER_WORDS`] `i64` words before payload.

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::BasicValueEnum;
use lumia_abi::adt_type_id;
use lumia_core::Local;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_stack_adt(
        &mut self,
        adt_name: &str,
        tag: i64,
        fields: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        let n = fields.len() as u64;
        let payload_bytes = (1 + n) * 8;
        let words = (lumia_abi::OBJECT_HEADER_WORDS as u64 + 1 + n) as u32; // header + tag + fields
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

        let kind = self.funs.adt_show_kinds.get(adt_name).copied().unwrap_or(0);
        let type_id = adt_type_id(kind) as u64;
        let hdr0 = self
            .llvm
            .i64_ty
            .const_int(type_id | (payload_bytes << 32), false);
        let hdr0_slot = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                storage,
                &[self.llvm.i64_ty.const_int(0, false)],
                "adt_hdr0",
            ))?
        };
        crate::error::llvm(self.llvm.builder.build_store(hdr0_slot, hdr0))?;
        // word1: marked=1 (stack); high 32 unused (align pad before `_pad`)
        let hdr1_slot = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                storage,
                &[self.llvm.i64_ty.const_int(1, false)],
                "adt_hdr1",
            ))?
        };
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(hdr1_slot, self.llvm.i64_ty.const_int(1, false)),
        )?;
        // word2: `_pad` filled after fields via `lumia_adt_set_float_mask` (sanitizes).
        let hdr2_slot = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                storage,
                &[self.llvm.i64_ty.const_int(2, false)],
                "adt_hdr2",
            ))?
        };
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(hdr2_slot, self.llvm.i64_ty.const_int(0, false)),
        )?;

        let payload = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                storage,
                &[self
                    .llvm
                    .i64_ty
                    .const_int(lumia_abi::OBJECT_HEADER_WORDS as u64, false)],
                "adt_payload",
            ))?
        };
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(payload, self.llvm.i64_ty.const_int(tag as u64, false)),
        )?;
        for (i, e) in fields.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self
                        .llvm
                        .i64_ty
                        .const_int((lumia_abi::OBJECT_HEADER_WORDS + 1 + i) as u64, false)],
                    "adt_f",
                ))?
            };
            crate::error::llvm(self.llvm.builder.build_store(slot, v))?;
            // Stack parent still aliases nested heap List/ADT for COW RC.
            if let Some(ty) = self.frame.local_tys.get(&e.0) {
                if Self::type_needs_cow_retain(ty) {
                    self.adt_retain_i64(v)?;
                }
            }
        }
        let float_mask = self.adt_float_mask_from_fields(fields);
        self.emit_adt_set_float_mask(payload, float_mask)?;
        Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
            payload,
            self.llvm.i64_ty,
            "adt_stack_i64",
        ))?
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
        let words = (lumia_abi::OBJECT_HEADER_WORDS as u64 + 1 + n) as u32; // header + len + elems
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
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                storage,
                &[self.llvm.i64_ty.const_int(0, false)],
                "sa_hdr0",
            ))?
        };
        crate::error::llvm(self.llvm.builder.build_store(hdr0_slot, hdr0))?;
        let hdr1_slot = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                storage,
                &[self.llvm.i64_ty.const_int(1, false)],
                "sa_hdr1",
            ))?
        };
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(hdr1_slot, self.llvm.i64_ty.const_int(1, false)),
        )?;
        let hdr2_slot = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                storage,
                &[self.llvm.i64_ty.const_int(2, false)],
                "sa_hdr2",
            ))?
        };
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(hdr2_slot, self.llvm.i64_ty.const_int(0, false)),
        )?;

        let payload = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                storage,
                &[self
                    .llvm
                    .i64_ty
                    .const_int(lumia_abi::OBJECT_HEADER_WORDS as u64, false)],
                "sa_payload",
            ))?
        };
        crate::error::llvm(
            self.llvm
                .builder
                .build_store(payload, self.llvm.i64_ty.const_int(n, false)),
        )?;
        for (i, e) in elems.iter().enumerate() {
            let v = self.coerce_i64(self.local(*e)?)?;
            let slot = unsafe {
                crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self
                        .llvm
                        .i64_ty
                        .const_int((lumia_abi::OBJECT_HEADER_WORDS + 1 + i) as u64, false)],
                    "sa_elem",
                ))?
            };
            crate::error::llvm(self.llvm.builder.build_store(slot, v))?;
            if let Some(ty) = self.frame.local_tys.get(&e.0) {
                if Self::type_needs_cow_retain(ty) {
                    self.adt_retain_i64(v)?;
                }
            }
        }
        Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
            payload,
            self.llvm.i64_ty,
            "sa_i64",
        ))?
        .into())
    }
}
