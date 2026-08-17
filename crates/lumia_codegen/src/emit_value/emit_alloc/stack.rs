//! Stack (non-escaping) List and ADT layouts.
//!
//! Map/Set never use this path (always heap / null empty). ObjectHeader is
//! [`lumia_abi::OBJECT_HEADER_BYTES`] ⇒ [`lumia_abi::OBJECT_HEADER_WORDS`] `i64`
//! words before payload.

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, PointerValue};
use lumia_abi::adt_type_id;
use lumia_core::Local;

impl<'ctx> Codegen<'ctx> {
    /// Entry-block alloca + three header words; returns `(storage, payload)`.
    ///
    /// Header layout: word0 = `type_id | (payload_bytes << 32)`, word1 = marked=1
    /// (stack), word2 = `_pad` (0 until ADT float-mask sanitize).
    fn emit_stack_header(
        &mut self,
        type_id: u64,
        payload_i64s: u64,
        alloca_name: &str,
    ) -> Result<(PointerValue<'ctx>, PointerValue<'ctx>)> {
        const _: () = assert!(lumia_abi::OBJECT_HEADER_WORDS == 3);
        let payload_bytes = payload_i64s * 8;
        let words = (lumia_abi::OBJECT_HEADER_WORDS as u64 + payload_i64s) as u32;
        let arr_ty = self.llvm.i64_ty.array_type(words);
        let entry = self
            .frame
            .entry_bb
            .context("emit_stack_header before emit_function")?;
        let cur = self
            .llvm
            .builder
            .get_insert_block()
            .context("no insert block")?;
        match entry.get_first_instruction() {
            Some(first) => self.llvm.builder.position_before(&first),
            None => self.llvm.builder.position_at_end(entry),
        }
        let storage = crate::error::llvm(self.llvm.builder.build_alloca(arr_ty, alloca_name))?;
        self.llvm.builder.position_at_end(cur);

        let hdr0 = self
            .llvm
            .i64_ty
            .const_int(type_id | (payload_bytes << 32), false);
        for (i, val) in [hdr0, self.llvm.i64_ty.const_int(1, false), self.llvm.i64_ty.const_int(0, false)]
            .into_iter()
            .enumerate()
        {
            let slot = unsafe {
                crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                    self.llvm.i64_ty,
                    storage,
                    &[self.llvm.i64_ty.const_int(i as u64, false)],
                    &format!("stk_hdr{i}"),
                ))?
            };
            crate::error::llvm(self.llvm.builder.build_store(slot, val))?;
        }
        let payload = unsafe {
            crate::error::llvm(self.llvm.builder.build_in_bounds_gep(
                self.llvm.i64_ty,
                storage,
                &[self
                    .llvm
                    .i64_ty
                    .const_int(lumia_abi::OBJECT_HEADER_WORDS as u64, false)],
                "stk_payload",
            ))?
        };
        Ok((storage, payload))
    }

    pub(crate) fn emit_stack_adt(
        &mut self,
        adt_name: &str,
        tag: i64,
        fields: &[Local],
    ) -> Result<BasicValueEnum<'ctx>> {
        let n = fields.len() as u64;
        let kind = self.funs.adt_show_kinds.get(adt_name).copied().unwrap_or(0);
        let type_id = adt_type_id(kind) as u64;
        let (storage, payload) = self.emit_stack_header(type_id, 1 + n, "stack_adt")?;
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
        let float_mask = self.adt_float_mask_from_fields(fields)?;
        self.emit_adt_set_float_mask(payload, float_mask)?;
        let bool_mask = self.adt_bool_mask_from_fields(fields)?;
        self.emit_adt_set_bool_mask(payload, bool_mask)?;
        Ok(crate::error::llvm(self.llvm.builder.build_ptr_to_int(
            payload,
            self.llvm.i64_ty,
            "adt_stack_i64",
        ))?
        .into())
    }

    /// Stack List-shaped array: ObjectHeader + `[len][elems…]` (also used for
    /// non-escaping Set payloads that share the same shape).
    pub(crate) fn emit_stack_array(
        &mut self,
        elems: &[Local],
        type_id: u64,
    ) -> Result<BasicValueEnum<'ctx>> {
        let n = elems.len() as u64;
        let (storage, payload) = self.emit_stack_header(type_id, 1 + n, "stack_arr")?;
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
