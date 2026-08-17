//! Println / Show value formatting for IO builtins.
//!
//! Typed shape classification is shared; sinks differ (direct println_* vs
//! heap String for `show`). Specialized container RT symbols live in one table
//! ([`SPECIALIZED_SHOW_RT`]) so show wiring and `runtime_decls` stay aligned.

use super::super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FloatValue, IntValue, PointerValue};
use lumia_ty::Type;

/// Typed-container Show helpers (Bool / Float masks). Keep in sync with
/// `runtime_decls` + `lumia_rt` `#[no_mangle]` (CI diffs no_mangle ↔ decls).
pub(crate) const SHOW_LIST_BOOL: &str = "lumia_show_list_bool";
pub(crate) const SHOW_SET_BOOL: &str = "lumia_show_set_bool";
pub(crate) const SHOW_MAP_BOOL: &str = "lumia_show_map_bool";
pub(crate) const SHOW_LIST_ADT: &str = "lumia_show_list_adt";
pub(crate) const SHOW_FLOAT: &str = "lumia_show_float";
pub(crate) const SHOW_BOOL: &str = "lumia_show_bool";
pub(crate) const SHOW_GENERIC: &str = "lumia_show";
pub(crate) const SHOW_ADT: &str = "lumia_show_adt";
pub(crate) const SHOW_ADT_NAMED: &str = "lumia_show_adt_named";

/// All Show RT symbols referenced from codegen (specialized + scalar/ADT).
/// Lock-in table for `runtime_decls` tests (emit uses `SHOW_*` consts).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const SPECIALIZED_SHOW_RT: &[&str] = &[
    SHOW_LIST_BOOL,
    SHOW_SET_BOOL,
    SHOW_MAP_BOOL,
    SHOW_LIST_ADT,
    SHOW_FLOAT,
    SHOW_BOOL,
    SHOW_GENERIC,
    SHOW_ADT,
    SHOW_ADT_NAMED,
];

/// Typed println sinks (keep ⊆ `runtime_decls` + `lumia_rt`).
pub(crate) const PRINTLN_UNIT: &str = "lumia_println_unit";
pub(crate) const PRINTLN_FLOAT: &str = "lumia_println_float";
pub(crate) const PRINTLN_BOOL: &str = "lumia_println_bool";
pub(crate) const PRINTLN_STR: &str = "lumia_println_str";
pub(crate) const PRINTLN_AUTO: &str = "lumia_println_auto";

/// Lock-in table for `runtime_decls` tests (emit uses `PRINTLN_*` consts).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const SPECIALIZED_PRINTLN_RT: &[&str] = &[
    PRINTLN_UNIT,
    PRINTLN_FLOAT,
    PRINTLN_BOOL,
    PRINTLN_STR,
    PRINTLN_AUTO,
];

/// Result of typed Show/Println classification (Todo: Show 双臂近拷贝).
enum ShowForm<'ctx> {
    Unit,
    Float(FloatValue<'ctx>),
    Bool8(IntValue<'ctx>),
    /// Already a heap String pointer (`lumia_show_*` helpers).
    StrPtr(PointerValue<'ctx>),
    /// Fall back to polymorphic `lumia_show` / `lumia_println_auto`.
    AutoI64(IntValue<'ctx>),
}

impl<'ctx> Codegen<'ctx> {
    fn emit_println_show_ptr(&mut self, ptr: PointerValue<'ctx>) -> Result<()> {
        let len = self
            .call_rt_basic("lumia_str_byte_len", &[ptr.into()], "show_len")?
            .into_int_value();
        self.call_rt_void(
            PRINTLN_STR,
            &[ptr.into(), len.into()],
            "println_show",
        )
    }

    /// Call a Show RT that returns a heap String pointer.
    fn call_show_rt_ptr(
        &mut self,
        symbol: &str,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> Result<PointerValue<'ctx>> {
        let fun = self.runtime_fn(symbol)?;
        Ok(crate::error::llvm(self.llvm.builder.build_call(fun, args, name))?
            .try_as_basic_value()
            .basic()
            .context("call return value")?
            .into_pointer_value())
    }

    /// Classify one value for Show/Println (shared Type arms).
    fn classify_show_form(
        &mut self,
        arg: BasicValueEnum<'ctx>,
        arg_ty: &Type,
    ) -> Result<ShowForm<'ctx>> {
        Ok(match arg_ty {
            Type::Unit => {
                let _ = arg;
                ShowForm::Unit
            }
            Type::Float => {
                let f = match arg {
                    BasicValueEnum::FloatValue(f) => f,
                    other => self.promote_f64(other)?,
                };
                ShowForm::Float(f)
            }
            Type::Bool => {
                let i = self.coerce_i64(arg)?;
                let b = crate::error::llvm(self.llvm.builder.build_int_truncate(
                    i,
                    self.llvm.context.i8_type(),
                    "bool8",
                ))?;
                ShowForm::Bool8(b)
            }
            Type::List(elem) if matches!(elem.as_ref(), Type::Bool) => {
                ShowForm::StrPtr(self.emit_show_list_bool(arg)?)
            }
            Type::List(elem) => {
                let meta = match elem.as_ref() {
                    Type::Adt { name, params } => Some((Some(name.as_str()), params.as_slice())),
                    Type::Tuple(ts) => Some((None, ts.as_slice())),
                    _ => None,
                };
                if let Some((adt_name, params)) =
                    meta.filter(|(_, p)| p.iter().any(|t| matches!(t, Type::Float | Type::Bool)))
                {
                    ShowForm::StrPtr(self.emit_show_list_adt(arg, adt_name, params)?)
                } else {
                    ShowForm::AutoI64(self.coerce_i64(arg)?)
                }
            }
            Type::Set(elem) => {
                ShowForm::StrPtr(self.emit_show_set(arg, matches!(elem.as_ref(), Type::Bool))?)
            }
            Type::Map(k, v) => ShowForm::StrPtr(self.emit_show_map_bool(arg, k, v)?),
            Type::Adt { name, params } => {
                if let Some(ptr) = self.emit_show_override(name, arg)? {
                    ShowForm::StrPtr(ptr)
                } else if self.funs.adt_variant_names.contains_key(name)
                    || params.iter().any(|p| matches!(p, Type::Float | Type::Bool))
                {
                    ShowForm::StrPtr(self.emit_typed_adt_show(name, arg, params)?)
                } else {
                    ShowForm::AutoI64(self.coerce_i64(arg)?)
                }
            }
            _ => ShowForm::AutoI64(self.coerce_i64(arg)?),
        })
    }

    /// Side-effecting print of one value (typed dispatch).
    pub(crate) fn emit_println_value(
        &mut self,
        arg: BasicValueEnum<'ctx>,
        arg_ty: &Type,
    ) -> Result<()> {
        match self.classify_show_form(arg, arg_ty)? {
            ShowForm::Unit => {
                self.call_rt_void(PRINTLN_UNIT, &[], "println_unit")?;
            }
            ShowForm::Float(f) => {
                self.call_rt_void(PRINTLN_FLOAT, &[f.into()], "println_float")?;
            }
            ShowForm::Bool8(b) => {
                self.call_rt_void(PRINTLN_BOOL, &[b.into()], "println_bool")?;
            }
            ShowForm::StrPtr(ptr) => self.emit_println_show_ptr(ptr)?,
            ShowForm::AutoI64(i) => {
                self.call_rt_void(PRINTLN_AUTO, &[i.into()], "println")?;
            }
        }
        Ok(())
    }

    fn emit_show_list_bool(&mut self, arg: BasicValueEnum<'ctx>) -> Result<PointerValue<'ctx>> {
        let i = self.coerce_i64(arg)?;
        self.call_show_rt_ptr(SHOW_LIST_BOOL, &[i.into()], "show_list_bool")
    }

    fn emit_show_set(
        &mut self,
        arg: BasicValueEnum<'ctx>,
        as_bool: bool,
    ) -> Result<PointerValue<'ctx>> {
        let i = self.coerce_i64(arg)?;
        let b = self
            .llvm
            .context
            .i32_type()
            .const_int(as_bool as u64, false);
        self.call_show_rt_ptr(SHOW_SET_BOOL, &[i.into(), b.into()], "show_set")
    }

    fn emit_show_map_bool(
        &mut self,
        arg: BasicValueEnum<'ctx>,
        k: &Type,
        v: &Type,
    ) -> Result<PointerValue<'ctx>> {
        let i = self.coerce_i64(arg)?;
        let kb = self
            .llvm
            .context
            .i32_type()
            .const_int(matches!(k, Type::Bool) as u64, false);
        let vb = self
            .llvm
            .context
            .i32_type()
            .const_int(matches!(v, Type::Bool) as u64, false);
        self.call_show_rt_ptr(
            SHOW_MAP_BOOL,
            &[i.into(), kb.into(), vb.into()],
            "show_map_bool",
        )
    }

    fn emit_show_list_adt(
        &mut self,
        arg: BasicValueEnum<'ctx>,
        adt_name: Option<&str>,
        params: &[Type],
    ) -> Result<PointerValue<'ctx>> {
        let i = self.coerce_i64(arg)?;
        // Result: type-param index ≠ constructor field index (Err field0 is
        // params[1]). Same as emit_typed_adt_show — rely on per-object `_pad`.
        let fmask = if adt_name.is_some_and(lumia_hir::is_result) {
            0
        } else {
            Self::adt_float_field_mask(params, &[])?
        };
        let bmask = if adt_name.is_some_and(lumia_hir::is_result) {
            0
        } else {
            Self::adt_bool_field_mask(params, &[])?
        };
        self.call_show_rt_ptr(
            SHOW_LIST_ADT,
            &[
                i.into(),
                self.llvm.i64_ty.const_int(fmask, false).into(),
                self.llvm.i64_ty.const_int(bmask, false).into(),
            ],
            "show_list_adt",
        )
    }

    /// Format one value as a heap String pointer.
    pub(crate) fn emit_show_ptr(
        &mut self,
        arg: BasicValueEnum<'ctx>,
        arg_ty: &Type,
    ) -> Result<PointerValue<'ctx>> {
        match self.classify_show_form(arg, arg_ty)? {
            ShowForm::Unit => {
                let ptr = self
                    .llvm
                    .builder
                    .build_global_string_ptr("Unit", "unit_show")
                    .map_err(|e| anyhow::anyhow!("unit show: {e}"))?
                    .as_pointer_value();
                self.call_show_rt_ptr(
                    "lumia_alloc_string",
                    &[ptr.into(), self.llvm.i64_ty.const_int(4, false).into()],
                    "show_unit",
                )
            }
            ShowForm::Float(f) => {
                self.call_show_rt_ptr(SHOW_FLOAT, &[f.into()], "show_float")
            }
            ShowForm::Bool8(b) => {
                self.call_show_rt_ptr(SHOW_BOOL, &[b.into()], "show_bool")
            }
            ShowForm::StrPtr(ptr) => Ok(ptr),
            ShowForm::AutoI64(i) => self.emit_show_auto_i64(i),
        }
    }

    fn emit_show_auto_i64(&mut self, i: IntValue<'ctx>) -> Result<PointerValue<'ctx>> {
        self.call_show_rt_ptr(SHOW_GENERIC, &[i.into()], "show")
    }
}

#[cfg(test)]
mod specialized_show_rt_tests {
    use super::{SPECIALIZED_PRINTLN_RT, SPECIALIZED_SHOW_RT};
    use crate::runtime_decls::runtime_decl_names_for_test;

    #[test]
    fn specialized_show_rt_symbols_are_declared() {
        let decls = runtime_decl_names_for_test();
        for sym in SPECIALIZED_SHOW_RT {
            assert!(decls.contains(sym), "{sym} missing from RUNTIME_DECLS");
        }
    }

    #[test]
    fn specialized_println_rt_symbols_are_declared() {
        let decls = runtime_decl_names_for_test();
        for sym in SPECIALIZED_PRINTLN_RT {
            assert!(decls.contains(sym), "{sym} missing from RUNTIME_DECLS");
        }
    }
}
