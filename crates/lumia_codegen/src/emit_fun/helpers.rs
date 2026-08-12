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

    /// Codegen tables for [`lumia_core::infer_value_ty_ctx`] / ParMap elem typing.
    pub(crate) fn infer_ctx(&self) -> lumia_core::InferValueCtx<'_> {
        lumia_core::InferValueCtx::full(
            &self.frame.local_tys,
            lumia_core::CodegenTypeTables {
                slot_tys: &self.frame.slot_tys,
                fun_ret_tys: &self.funs.fun_ret_tys,
                fun_param_tys: &self.funs.fun_param_tys,
                fun_param0_identity: &self.funs.fun_param0_identity,
                funref_locals: &self.funs.funref_locals,
            },
        )
    }

    /// FunRef values are tagged with the low bit; refuse heap closures for par_* workers.
    pub(crate) fn ensure_funref_ptr(
        &mut self,
        fun_i: inkwell::values::IntValue<'ctx>,
        prefix: &str,
    ) -> Result<inkwell::values::PointerValue<'ctx>> {
        use inkwell::{AddressSpace, IntPredicate};
        let ptr_ty = self.llvm.context.ptr_type(AddressSpace::default());
        let one = self.llvm.i64_ty.const_int(1, false);
        let tagged = crate::error::llvm(self.llvm.builder.build_and(
            fun_i,
            one,
            &format!("{prefix}_tag"),
        ))?;
        let is_funref = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::EQ,
            tagged,
            one,
            &format!("{prefix}_is_fr"),
        ))?;
        let cur = self
            .llvm
            .builder
            .get_insert_block()
            .with_context(|| format!("{prefix} needs insert block"))?;
        let parent = cur
            .get_parent()
            .with_context(|| format!("{prefix} bb parent"))?;
        let ok_bb = self
            .llvm
            .context
            .append_basic_block(parent, &format!("{prefix}_ok"));
        let bad_bb = self
            .llvm
            .context
            .append_basic_block(parent, &format!("{prefix}_bad"));
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(is_funref, ok_bb, bad_bb),
        )?;
        self.llvm.builder.position_at_end(bad_bb);
        let fail = self.runtime_fn("lumia_match_fail")?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_call(fail, &[], &format!("{prefix}_bad_fn")),
        )?;
        crate::error::llvm(self.llvm.builder.build_unreachable())?;
        self.llvm.builder.position_at_end(ok_bb);
        let not1 = crate::error::llvm(self.llvm.builder.build_not(one, &format!("{prefix}_not1")))?;
        let cleared = crate::error::llvm(self.llvm.builder.build_and(
            fun_i,
            not1,
            &format!("{prefix}_clear"),
        ))?;
        crate::error::llvm(self.llvm.builder.build_int_to_ptr(
            cleared,
            ptr_ty,
            &format!("{prefix}_fn"),
        ))
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

    /// Record a possible old→young edge after storing a pointer-sized field.
    /// No-op at runtime when `obj` is young / `new` is not a young heap ptr.
    /// Remembered-set barrier for old→young pointer stores.
    ///
    /// List/Map/Set mutations emit barriers inside the RT. Direct field stores
    /// from codegen (if added) should call this; alloc-init stores skip it
    /// because `lumia_alloc` returns a young object.
    #[allow(dead_code)]
    pub(crate) fn emit_write_barrier(
        &self,
        obj: inkwell::values::PointerValue<'ctx>,
        field: u32,
        new_i64: inkwell::values::IntValue<'ctx>,
    ) -> Result<()> {
        let field_v = self.llvm.context.i32_type().const_int(field as u64, false);
        let ptr_ty = self.llvm.context.ptr_type(inkwell::AddressSpace::default());
        let new_ptr = crate::error::llvm(
            self.llvm
                .builder
                .build_int_to_ptr(new_i64, ptr_ty, "wb_new"),
        )?;
        self.call_rt_void(
            "lumia_write_barrier",
            &[obj.into(), field_v.into(), new_ptr.into()],
            "wb",
        )
    }
}
