//! Local lookup, numeric coercion, and runtime call helpers.

use super::super::Codegen;
use anyhow::{bail, Context as AnyhowContext, Result};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue};
use lumia_core::Local;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn local(&self, l: Local) -> Result<BasicValueEnum<'ctx>> {
        self.frame
            .locals
            .get(&l.0)
            .copied()
            .with_context(|| format!("undefined local %{}", l.0))
    }

    pub(crate) fn as_i64(&self, v: BasicValueEnum<'ctx>) -> Result<IntValue<'ctx>> {
        match v {
            BasicValueEnum::IntValue(i) => Ok(i),
            BasicValueEnum::FloatValue(f) => Ok(self
                .llvm
                .builder
                .build_bit_cast(f, self.llvm.i64_ty, "f64_bits")
                .map_err(|e| anyhow::anyhow!("bitcast f64_bits: {e}"))?
                .into_int_value()),
            BasicValueEnum::PointerValue(p) => Ok(self
                .llvm
                .builder
                .build_ptr_to_int(p, self.llvm.i64_ty, "ptr_i64")
                .map_err(|e| anyhow::anyhow!("ptr_to_int: {e}"))?),
            _ => bail!("expected i64 value"),
        }
    }

    pub(crate) fn coerce_i64(&self, v: BasicValueEnum<'ctx>) -> Result<IntValue<'ctx>> {
        self.as_i64(v)
    }

    pub(crate) fn promote_f64(
        &self,
        v: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::FloatValue<'ctx>> {
        let fty = self.llvm.context.f64_type();
        match v {
            BasicValueEnum::FloatValue(f) => Ok(f),
            // Float ABI: values travel as i64 bit patterns (not numeric conversion).
            BasicValueEnum::IntValue(i) => Ok(self
                .llvm
                .builder
                .build_bit_cast(i, fty, "i64_bits_f64")
                .map_err(|e| anyhow::anyhow!("bitcast i64_bits_f64: {e}"))?
                .into_float_value()),
            _ => bail!("expected numeric for float promote"),
        }
    }

    /// Convert an operand for float arithmetic: numeric Int → sitofp; Float bits → bitcast.
    pub(crate) fn arith_as_f64(
        &self,
        v: BasicValueEnum<'ctx>,
        ty: &Type,
    ) -> Result<inkwell::values::FloatValue<'ctx>> {
        let fty = self.llvm.context.f64_type();
        match v {
            BasicValueEnum::FloatValue(f) => Ok(f),
            BasicValueEnum::IntValue(i) if matches!(ty, Type::Float) => Ok(self
                .llvm
                .builder
                .build_bit_cast(i, fty, "fbits_arith")
                .map_err(|e| anyhow::anyhow!("bitcast fbits_arith: {e}"))?
                .into_float_value()),
            BasicValueEnum::IntValue(i) => Ok(self
                .llvm
                .builder
                .build_signed_int_to_float(i, fty, "sitofp")
                .map_err(|e| anyhow::anyhow!("sitofp: {e}"))?),
            _ => bail!("expected numeric for float arith"),
        }
    }

    pub(crate) fn build_call(
        &self,
        f: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> Result<inkwell::values::CallSiteValue<'ctx>> {
        self.llvm
            .builder
            .build_call(f, args, name)
            .map_err(|e| anyhow::anyhow!("LLVM build_call `{name}`: {e}"))
    }

    pub(crate) fn call_rt(
        &self,
        name: &'static str,
        args: &[BasicMetadataValueEnum<'ctx>],
        label: &str,
    ) -> Result<inkwell::values::CallSiteValue<'ctx>> {
        let f = self.runtime_fn(name)?;
        self.build_call(f, args, label)
    }

    pub(crate) fn call_rt_basic(
        &self,
        name: &'static str,
        args: &[BasicMetadataValueEnum<'ctx>],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>> {
        self.call_rt(name, args, label)?
            .try_as_basic_value()
            .basic()
            .with_context(|| format!("runtime `{name}` returned void, expected a value"))
    }

    pub(crate) fn call_rt_void(
        &self,
        name: &'static str,
        args: &[BasicMetadataValueEnum<'ctx>],
        label: &str,
    ) -> Result<()> {
        self.call_rt(name, args, label)?;
        Ok(())
    }
}
