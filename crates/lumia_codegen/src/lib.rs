//! LLVM codegen via inkwell (LLVM 21). Links against `lumia_rt`.

use anyhow::{bail, Context as AnyhowContext, Result};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::{BasicMetadataTypeEnum, IntType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PointerValue,
};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate, OptimizationLevel};
use lumia_core::{Block, CoreFun, CoreModule, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::{BinOp, UnOp};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct CodegenOptions {
    pub release: bool,
    pub output: PathBuf,
    pub emit_ir: bool,
    pub runtime_lib: PathBuf,
    /// Tags for `Option::{Some, None}` from the source module (defaults 0/1).
    pub option_some_tag: i64,
    pub option_none_tag: i64,
}

pub fn compile_module(core: &CoreModule, opts: &CodegenOptions) -> Result<()> {
    let context = Context::create();
    let mut cg = Codegen::new(
        &context,
        &core.name,
        opts.option_some_tag,
        opts.option_none_tag,
    );
    declare_runtime(&context, &cg.module);

    for f in &core.functions {
        let fn_ty = cg.i64_ty.fn_type(
            &vec![BasicMetadataTypeEnum::from(cg.i64_ty); f.params.len()],
            false,
        );
        let name = if f.is_main {
            "lumia_user_main".to_string()
        } else {
            f.name.clone()
        };
        let fv = cg.module.add_function(&name, fn_ty, None);
        cg.functions.insert(f.name.clone(), fv);
    }

    // Collect owned names to avoid borrow issues while emitting
    let funs: Vec<_> = core.functions.clone();
    for f in &funs {
        cg.emit_function(f)?;
    }

    emit_c_main(&context, &cg.module, &cg.builder);

    if opts.emit_ir {
        let ir_path = opts.output.with_extension("ll");
        cg.module
            .print_to_file(&ir_path)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    cg.module
        .verify()
        .map_err(|e| anyhow::anyhow!("LLVM verify: {e}"))?;

    Target::initialize_all(&InitializationConfig::default());
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| anyhow::anyhow!("{e}"))?;
    let cpu = TargetMachine::get_host_cpu_name().to_string();
    let features = TargetMachine::get_host_cpu_features().to_string();
    let opt = if opts.release {
        OptimizationLevel::Aggressive
    } else {
        OptimizationLevel::None
    };
    let tm = target
        .create_target_machine(
            &triple,
            &cpu,
            &features,
            opt,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .context("create target machine")?;

    let obj_path = opts.output.with_extension("o");
    tm.write_to_file(&cg.module, FileType::Object, &obj_path)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Drop LLVM module before linking (owns no further need)
    drop(cg);

    link_executable(&obj_path, &opts.runtime_lib, &opts.output)?;
    Ok(())
}

fn declare_runtime<'ctx>(context: &'ctx Context, module: &LlvmModule<'ctx>) {
    let i64_ty = context.i64_type();
    let i32_ty = context.i32_type();
    let i8_ty = context.i8_type();
    let ptr_ty = context.ptr_type(AddressSpace::default());
    let void_ty = context.void_type();

    module.add_function(
        "lumia_println_int",
        void_ty.fn_type(&[i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_println_auto",
        void_ty.fn_type(&[i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_println_float",
        void_ty.fn_type(&[context.f64_type().into()], false),
        None,
    );
    module.add_function(
        "lumia_eq",
        i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_println_str",
        void_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_println_cstr",
        void_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_println_bool",
        void_ty.fn_type(&[i8_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_alloc",
        ptr_ty.fn_type(&[i64_ty.into(), i32_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_alloc_string",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_alloc_char",
        ptr_ty.fn_type(&[i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_show",
        ptr_ty.fn_type(&[i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_show_float",
        ptr_ty.fn_type(&[context.f64_type().into()], false),
        None,
    );
    module.add_function("lumia_gc_collect", void_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_root_push",
        void_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function("lumia_root_pop", void_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_write_barrier",
        void_ty.fn_type(&[ptr_ty.into(), i32_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_len",
        i64_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_get",
        i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_slice",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_append",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_concat",
        ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_len",
        i64_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_concat",
        ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_get",
        i64_ty.fn_type(
            &[
                ptr_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
            ],
            false,
        ),
        None,
    );
    module.add_function(
        "lumia_contains",
        i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_map_set",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_map_remove",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_map_keys",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_map_values",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_map_items",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_adt_tag",
        i64_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_adt_field",
        i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_range",
        ptr_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_range_inclusive",
        ptr_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false),
        None,
    );
}

fn emit_c_main<'ctx>(context: &'ctx Context, module: &LlvmModule<'ctx>, builder: &Builder<'ctx>) {
    let i32_ty = context.i32_type();
    let main_ty = i32_ty.fn_type(&[], false);
    let main_fn = module.add_function("main", main_ty, None);
    let entry = context.append_basic_block(main_fn, "entry");
    builder.position_at_end(entry);
    if let Some(user) = module.get_function("lumia_user_main") {
        builder.build_call(user, &[], "call_main").unwrap();
    }
    builder
        .build_return(Some(&i32_ty.const_int(0, false)))
        .unwrap();
}

struct Codegen<'ctx> {
    context: &'ctx Context,
    module: LlvmModule<'ctx>,
    builder: Builder<'ctx>,
    i64_ty: IntType<'ctx>,
    functions: HashMap<String, FunctionValue<'ctx>>,
    locals: HashMap<u32, BasicValueEnum<'ctx>>,
    /// Mutable bindings: alloca slots (i64 payload, pointers stored as i64).
    slots: HashMap<String, PointerValue<'ctx>>,
    /// Nested loop targets: (continue_bb, break_bb)
    loop_stack: Vec<(BasicBlock<'ctx>, BasicBlock<'ctx>)>,
    option_some_tag: i64,
    option_none_tag: i64,
}

impl<'ctx> Codegen<'ctx> {
    fn new(context: &'ctx Context, name: &str, option_some_tag: i64, option_none_tag: i64) -> Self {
        Self {
            context,
            module: context.create_module(name),
            builder: context.create_builder(),
            i64_ty: context.i64_type(),
            functions: HashMap::new(),
            locals: HashMap::new(),
            slots: HashMap::new(),
            loop_stack: Vec::new(),
            option_some_tag,
            option_none_tag,
        }
    }

    fn emit_function(&mut self, fun: &CoreFun) -> Result<()> {
        let fv = *self
            .functions
            .get(&fun.name)
            .context("missing function decl")?;
        let entry = self.context.append_basic_block(fv, "entry");
        self.builder.position_at_end(entry);
        self.locals.clear();
        self.slots.clear();
        self.loop_stack.clear();

        for (i, p) in fun.params.iter().enumerate() {
            let av = fv.get_nth_param(i as u32).unwrap();
            self.locals.insert(p.0, av);
        }

        let result = self.emit_block(&fun.body, fv)?;
        let ret = result.unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
        match ret {
            BasicValueEnum::IntValue(i) => {
                self.builder.build_return(Some(&i)).unwrap();
            }
            other => {
                let as_i64 = self.coerce_i64(other)?;
                self.builder.build_return(Some(&as_i64)).unwrap();
            }
        }
        Ok(())
    }

    fn ensure_slot(&mut self, name: &str) -> PointerValue<'ctx> {
        if let Some(p) = self.slots.get(name) {
            return *p;
        }
        let alloca = self
            .builder
            .build_alloca(self.i64_ty, &format!("mut_{name}"))
            .unwrap();
        self.slots.insert(name.to_string(), alloca);
        alloca
    }

    fn store_slot(&mut self, name: &str, v: BasicValueEnum<'ctx>) -> Result<()> {
        let slot = self.ensure_slot(name);
        let i = self.coerce_i64(v)?;
        self.builder.build_store(slot, i).unwrap();
        Ok(())
    }

    fn load_slot(&self, name: &str) -> Result<BasicValueEnum<'ctx>> {
        let slot = self
            .slots
            .get(name)
            .copied()
            .with_context(|| format!("unbound mutable `{name}`"))?;
        Ok(self.builder.build_load(self.i64_ty, slot, name).unwrap())
    }

    fn emit_block(
        &mut self,
        block: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        for op in &block.ops {
            // If current block already terminated (after break/continue), skip.
            if self
                .builder
                .get_insert_block()
                .and_then(|bb| bb.get_terminator())
                .is_some()
            {
                break;
            }
            match op {
                Op::Let { local, value, .. } => {
                    let v = self.emit_value(value, fv)?;
                    self.locals.insert(local.0, v);
                }
                Op::Effect { value } => {
                    let _ = self.emit_value(value, fv)?;
                }
                Op::Assign { name, value } => {
                    let v = self.local(*value)?;
                    self.store_slot(name, v)?;
                }
                Op::Break => {
                    let (_, break_bb) = self
                        .loop_stack
                        .last()
                        .copied()
                        .context("break outside loop")?;
                    self.builder.build_unconditional_branch(break_bb).unwrap();
                }
                Op::Continue => {
                    let (cont_bb, _) = self
                        .loop_stack
                        .last()
                        .copied()
                        .context("continue outside loop")?;
                    self.builder.build_unconditional_branch(cont_bb).unwrap();
                }
            }
        }
        if self
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some()
        {
            return Ok(None);
        }
        if let Some(r) = block.result {
            Ok(Some(self.local(r)?))
        } else {
            Ok(None)
        }
    }

    fn local(&self, l: Local) -> Result<BasicValueEnum<'ctx>> {
        self.locals
            .get(&l.0)
            .copied()
            .with_context(|| format!("undefined local %{}", l.0))
    }

    fn as_i64(&self, v: BasicValueEnum<'ctx>) -> Result<IntValue<'ctx>> {
        match v {
            BasicValueEnum::IntValue(i) => Ok(i),
            _ => bail!("expected i64 value"),
        }
    }

    fn coerce_i64(&self, v: BasicValueEnum<'ctx>) -> Result<IntValue<'ctx>> {
        match v {
            BasicValueEnum::IntValue(i) => Ok(i),
            BasicValueEnum::PointerValue(p) => Ok(self
                .builder
                .build_ptr_to_int(p, self.i64_ty, "ptr_i64")
                .unwrap()),
            _ => Ok(self.i64_ty.const_int(0, false)),
        }
    }

    fn emit_value(
        &mut self,
        value: &Value,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        match value {
            Value::Int(n) => Ok(self.i64_ty.const_int(*n as u64, true).into()),
            Value::Float(n) => Ok(self.context.f64_type().const_float(*n).into()),
            Value::Bool(b) => Ok(self.i64_ty.const_int(if *b { 1 } else { 0 }, false).into()),
            Value::String(s) => {
                let gv = self.builder.build_global_string_ptr(s, "str").unwrap();
                let ptr = gv.as_pointer_value();
                let len = self.i64_ty.const_int(s.len() as u64, false);
                let f = self.module.get_function("lumia_alloc_string").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[ptr.into(), len.into()], "alloc_str")
                    .unwrap();
                let heap = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(heap, self.i64_ty, "str_i64")
                    .unwrap()
                    .into())
            }
            Value::Char(c) => {
                let cp = self.i64_ty.const_int(*c as u32 as u64, false);
                let f = self.module.get_function("lumia_alloc_char").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[cp.into()], "alloc_char")
                    .unwrap();
                let heap = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(heap, self.i64_ty, "char_i64")
                    .unwrap()
                    .into())
            }
            Value::Unit => Ok(self.i64_ty.const_int(0, false).into()),
            Value::Local(l) => self.local(*l),
            Value::Name(name) => self.load_slot(name),
            Value::Binary { op, left, right } => {
                let lv = self.local(*left)?;
                let rv = self.local(*right)?;
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lv, rv) {
                    let v = match op {
                        BinOp::Add => self.builder.build_float_add(l, r, "fadd").unwrap(),
                        BinOp::Sub => self.builder.build_float_sub(l, r, "fsub").unwrap(),
                        BinOp::Mul => self.builder.build_float_mul(l, r, "fmul").unwrap(),
                        BinOp::Div => self.builder.build_float_div(l, r, "fdiv").unwrap(),
                        BinOp::Rem => self.builder.build_float_rem(l, r, "frem").unwrap(),
                        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                            let pred = match op {
                                BinOp::Eq => FloatPredicate::OEQ,
                                BinOp::Ne => FloatPredicate::ONE,
                                BinOp::Lt => FloatPredicate::OLT,
                                BinOp::Le => FloatPredicate::OLE,
                                BinOp::Gt => FloatPredicate::OGT,
                                BinOp::Ge => FloatPredicate::OGE,
                                _ => unreachable!(),
                            };
                            let c = self
                                .builder
                                .build_float_compare(pred, l, r, "fcmp")
                                .unwrap();
                            return Ok(self
                                .builder
                                .build_int_z_extend(c, self.i64_ty, "fcmpz")
                                .unwrap()
                                .into());
                        }
                        BinOp::And | BinOp::Or => bail!("logical op on Float"),
                    };
                    return Ok(v.into());
                }
                let l = self.as_i64(lv)?;
                let r = self.as_i64(rv)?;
                let v = match op {
                    BinOp::Add => self.builder.build_int_add(l, r, "add").unwrap(),
                    BinOp::Sub => self.builder.build_int_sub(l, r, "sub").unwrap(),
                    BinOp::Mul => self.builder.build_int_mul(l, r, "mul").unwrap(),
                    BinOp::Div => self.builder.build_int_signed_div(l, r, "div").unwrap(),
                    BinOp::Rem => self.builder.build_int_signed_rem(l, r, "rem").unwrap(),
                    BinOp::Eq => {
                        let f = self.module.get_function("lumia_eq").unwrap();
                        let call = self
                            .builder
                            .build_call(f, &[l.into(), r.into()], "eq")
                            .unwrap();
                        call.try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_int_value()
                    }
                    BinOp::Ne => {
                        let f = self.module.get_function("lumia_eq").unwrap();
                        let call = self
                            .builder
                            .build_call(f, &[l.into(), r.into()], "eq")
                            .unwrap();
                        let eq = call
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_int_value();
                        let z = self.i64_ty.const_int(0, false);
                        let c = self
                            .builder
                            .build_int_compare(IntPredicate::EQ, eq, z, "ne")
                            .unwrap();
                        self.builder
                            .build_int_z_extend(c, self.i64_ty, "nez")
                            .unwrap()
                    }
                    BinOp::Lt => {
                        let c = self
                            .builder
                            .build_int_compare(IntPredicate::SLT, l, r, "lt")
                            .unwrap();
                        self.builder
                            .build_int_z_extend(c, self.i64_ty, "ltz")
                            .unwrap()
                    }
                    BinOp::Le => {
                        let c = self
                            .builder
                            .build_int_compare(IntPredicate::SLE, l, r, "le")
                            .unwrap();
                        self.builder
                            .build_int_z_extend(c, self.i64_ty, "lez")
                            .unwrap()
                    }
                    BinOp::Gt => {
                        let c = self
                            .builder
                            .build_int_compare(IntPredicate::SGT, l, r, "gt")
                            .unwrap();
                        self.builder
                            .build_int_z_extend(c, self.i64_ty, "gtz")
                            .unwrap()
                    }
                    BinOp::Ge => {
                        let c = self
                            .builder
                            .build_int_compare(IntPredicate::SGE, l, r, "ge")
                            .unwrap();
                        self.builder
                            .build_int_z_extend(c, self.i64_ty, "gez")
                            .unwrap()
                    }
                    BinOp::And => self.builder.build_and(l, r, "and").unwrap(),
                    BinOp::Or => self.builder.build_or(l, r, "or").unwrap(),
                };
                Ok(v.into())
            }
            Value::Unary { op, operand } => {
                let ov = self.local(*operand)?;
                if let BasicValueEnum::FloatValue(o) = ov {
                    let v = match op {
                        UnOp::Neg => self.builder.build_float_neg(o, "fneg").unwrap(),
                        UnOp::Not => bail!("not on Float"),
                    };
                    return Ok(v.into());
                }
                let o = self.as_i64(ov)?;
                let v = match op {
                    UnOp::Neg => self.builder.build_int_neg(o, "neg").unwrap(),
                    UnOp::Not => {
                        let z = self.i64_ty.const_int(0, false);
                        let c = self
                            .builder
                            .build_int_compare(IntPredicate::EQ, o, z, "not")
                            .unwrap();
                        self.builder
                            .build_int_z_extend(c, self.i64_ty, "notz")
                            .unwrap()
                    }
                };
                Ok(v.into())
            }
            Value::Call { fun, args } => {
                let callee = self
                    .functions
                    .get(fun)
                    .copied()
                    .with_context(|| format!("unknown function {fun}"))?;
                let mut av: Vec<BasicMetadataValueEnum> = vec![];
                for a in args {
                    let v = self.coerce_i64(self.local(*a)?)?;
                    av.push(v.into());
                }
                let call = self.builder.build_call(callee, &av, "call").unwrap();
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .unwrap_or_else(|| self.i64_ty.const_int(0, false).into()))
            }
            Value::IndirectCall { callee, args } => {
                let cal_i = self.coerce_i64(self.local(*callee)?)?;
                let one = self.i64_ty.const_int(1, false);
                let tagged = self.builder.build_and(cal_i, one, "ic_tag").unwrap();
                let is_funref = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, tagged, one, "is_funref")
                    .unwrap();

                let cur = self
                    .builder
                    .get_insert_block()
                    .context("indirect call needs insert block")?;
                let parent = cur.get_parent().context("bb parent")?;
                let funref_bb = self.context.append_basic_block(parent, "icall_funref");
                let clos_bb = self.context.append_basic_block(parent, "icall_clos");
                let merge_bb = self.context.append_basic_block(parent, "icall_merge");
                self.builder
                    .build_conditional_branch(is_funref, funref_bb, clos_bb)
                    .unwrap();

                // Bare FunRef (low bit set): call without env.
                self.builder.position_at_end(funref_bb);
                let not_one = self.builder.build_not(one, "not1").unwrap();
                let fn_i = self.builder.build_and(cal_i, not_one, "fn_clear").unwrap();
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let fn_ptr = self
                    .builder
                    .build_int_to_ptr(fn_i, ptr_ty, "fn_ptr")
                    .unwrap();
                let param_tys: Vec<BasicMetadataTypeEnum> =
                    args.iter().map(|_| self.i64_ty.into()).collect();
                let fn_ty = self.i64_ty.fn_type(&param_tys, false);
                let mut av: Vec<BasicMetadataValueEnum> = vec![];
                for a in args {
                    let v = self.coerce_i64(self.local(*a)?)?;
                    av.push(v.into());
                }
                let call_fr = self
                    .builder
                    .build_indirect_call(fn_ty, fn_ptr, &av, "icall_fr")
                    .unwrap();
                let fr_v = call_fr
                    .try_as_basic_value()
                    .basic()
                    .unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
                let fr_i = self.coerce_i64(fr_v)?;
                self.builder.build_unconditional_branch(merge_bb).unwrap();
                let funref_bb_end = self.builder.get_insert_block().unwrap();

                // Heap closure: load code ptr, pass env as first arg.
                self.builder.position_at_end(clos_bb);
                let env_ptr = self
                    .builder
                    .build_int_to_ptr(cal_i, ptr_ty, "clos_env")
                    .unwrap();
                let fn_slot = unsafe {
                    self.builder
                        .build_gep(
                            self.i64_ty,
                            env_ptr,
                            &[self.i64_ty.const_int(0, false)],
                            "clos_fn_slot",
                        )
                        .unwrap()
                };
                let fn_i2 = self
                    .builder
                    .build_load(self.i64_ty, fn_slot, "clos_fn")
                    .unwrap()
                    .into_int_value();
                let fn_ptr2 = self
                    .builder
                    .build_int_to_ptr(fn_i2, ptr_ty, "clos_fn_ptr")
                    .unwrap();
                let mut clos_param_tys: Vec<BasicMetadataTypeEnum> =
                    vec![self.i64_ty.into()];
                for _ in args.iter() {
                    clos_param_tys.push(self.i64_ty.into());
                }
                let clos_fn_ty = self.i64_ty.fn_type(&clos_param_tys, false);
                let mut cav: Vec<BasicMetadataValueEnum> = vec![cal_i.into()];
                for a in args {
                    let v = self.coerce_i64(self.local(*a)?)?;
                    cav.push(v.into());
                }
                let call_cl = self
                    .builder
                    .build_indirect_call(clos_fn_ty, fn_ptr2, &cav, "icall_cl")
                    .unwrap();
                let cl_v = call_cl
                    .try_as_basic_value()
                    .basic()
                    .unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
                let cl_i = self.coerce_i64(cl_v)?;
                self.builder.build_unconditional_branch(merge_bb).unwrap();
                let clos_bb_end = self.builder.get_insert_block().unwrap();

                self.builder.position_at_end(merge_bb);
                let phi = self.builder.build_phi(self.i64_ty, "icall_res").unwrap();
                phi.add_incoming(&[(&fr_i, funref_bb_end), (&cl_i, clos_bb_end)]);
                Ok(phi.as_basic_value())
            }
            Value::FunRef(name) => {
                let fv = self
                    .functions
                    .get(name)
                    .copied()
                    .with_context(|| format!("unknown funref {name}"))?;
                let ptr = fv.as_global_value().as_pointer_value();
                let as_i = self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "funref_i64")
                    .unwrap();
                // Tag low bit so IndirectCall can tell FunRef from heap closure.
                let tagged = self
                    .builder
                    .build_or(as_i, self.i64_ty.const_int(1, false), "funref_tag")
                    .unwrap();
                Ok(tagged.into())
            }
            Value::Builtin { name, args } => match name {
                Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr => {
                    let arg = self.local(args[0])?;
                    match arg {
                        BasicValueEnum::FloatValue(f) => {
                            let fun = self.module.get_function("lumia_println_float").unwrap();
                            self.builder
                                .build_call(fun, &[f.into()], "println_float")
                                .unwrap();
                        }
                        _ => {
                            let i = self.coerce_i64(arg)?;
                            let fun = self.module.get_function("lumia_println_auto").unwrap();
                            self.builder
                                .build_call(fun, &[i.into()], "println")
                                .unwrap();
                        }
                    }
                    Ok(self.i64_ty.const_int(0, false).into())
                }
                Builtin::ListLen => {
                    let list_i = self.coerce_i64(self.local(args[0])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let list = self
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "obj_ptr")
                        .unwrap();
                    let f = self.module.get_function("lumia_len").unwrap();
                    let call = self.builder.build_call(f, &[list.into()], "len").unwrap();
                    Ok(call.try_as_basic_value().basic().unwrap())
                }
                Builtin::ListGet => {
                    let list_i = self.coerce_i64(self.local(args[0])?)?;
                    let idx = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let list = self
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "col_ptr")
                        .unwrap();
                    let some = self.i64_ty.const_int(self.option_some_tag as u64, true);
                    let none = self.i64_ty.const_int(self.option_none_tag as u64, true);
                    let f = self.module.get_function("lumia_get").unwrap();
                    let call = self
                        .builder
                        .build_call(
                            f,
                            &[list.into(), idx.into(), some.into(), none.into()],
                            "get",
                        )
                        .unwrap();
                    Ok(call.try_as_basic_value().basic().unwrap())
                }
                Builtin::Contains => {
                    let obj_i = self.coerce_i64(self.local(args[0])?)?;
                    let key = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let obj = self
                        .builder
                        .build_int_to_ptr(obj_i, ptr_ty, "col_ptr")
                        .unwrap();
                    let f = self.module.get_function("lumia_contains").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[obj.into(), key.into()], "contains")
                        .unwrap();
                    Ok(call.try_as_basic_value().basic().unwrap())
                }
                Builtin::MapSet => {
                    let map_i = self.coerce_i64(self.local(args[0])?)?;
                    let key = self.coerce_i64(self.local(args[1])?)?;
                    let val = self.coerce_i64(self.local(args[2])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let map = self
                        .builder
                        .build_int_to_ptr(map_i, ptr_ty, "map_ptr")
                        .unwrap();
                    let f = self.module.get_function("lumia_map_set").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[map.into(), key.into(), val.into()], "map_set")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "map_set_i64")
                        .unwrap()
                        .into())
                }
                Builtin::MapRemove => {
                    let map_i = self.coerce_i64(self.local(args[0])?)?;
                    let key = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let map = self
                        .builder
                        .build_int_to_ptr(map_i, ptr_ty, "map_ptr")
                        .unwrap();
                    let f = self.module.get_function("lumia_map_remove").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[map.into(), key.into()], "map_rm")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "map_rm_i64")
                        .unwrap()
                        .into())
                }
                Builtin::MapKeys | Builtin::MapValues | Builtin::MapItems => {
                    let map_i = self.coerce_i64(self.local(args[0])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let map = self
                        .builder
                        .build_int_to_ptr(map_i, ptr_ty, "map_ptr")
                        .unwrap();
                    let fname = match name {
                        Builtin::MapKeys => "lumia_map_keys",
                        Builtin::MapValues => "lumia_map_values",
                        _ => "lumia_map_items",
                    };
                    let f = self.module.get_function(fname).unwrap();
                    let call = self.builder.build_call(f, &[map.into()], "map_kv").unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "map_kv_i64")
                        .unwrap()
                        .into())
                }
                Builtin::AdtTag => {
                    let obj_i = self.coerce_i64(self.local(args[0])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let obj = self
                        .builder
                        .build_int_to_ptr(obj_i, ptr_ty, "adt_ptr")
                        .unwrap();
                    let f = self.module.get_function("lumia_adt_tag").unwrap();
                    let call = self.builder.build_call(f, &[obj.into()], "adt_tag").unwrap();
                    Ok(call.try_as_basic_value().basic().unwrap())
                }
                Builtin::AdtField => {
                    let obj_i = self.coerce_i64(self.local(args[0])?)?;
                    let idx = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let obj = self
                        .builder
                        .build_int_to_ptr(obj_i, ptr_ty, "adt_ptr")
                        .unwrap();
                    let f = self.module.get_function("lumia_adt_field").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[obj.into(), idx.into()], "adt_field")
                        .unwrap();
                    Ok(call.try_as_basic_value().basic().unwrap())
                }
                Builtin::ListSlice => {
                    let list_i = self.coerce_i64(self.local(args[0])?)?;
                    let start = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let list = self
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "list_ptr")
                        .unwrap();
                    let f = self.module.get_function("lumia_list_slice").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[list.into(), start.into()], "slice")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "slice_i64")
                        .unwrap()
                        .into())
                }
                Builtin::ListAppend => {
                    let list_i = self.coerce_i64(self.local(args[0])?)?;
                    let elem = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let list = self
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "list_ptr")
                        .unwrap();
                    let f = self.module.get_function("lumia_list_append").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[list.into(), elem.into()], "append")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "append_i64")
                        .unwrap()
                        .into())
                }
                Builtin::ListConcat => {
                    let a_i = self.coerce_i64(self.local(args[0])?)?;
                    let b_i = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let a = self
                        .builder
                        .build_int_to_ptr(a_i, ptr_ty, "concat_a")
                        .unwrap();
                    let b = self
                        .builder
                        .build_int_to_ptr(b_i, ptr_ty, "concat_b")
                        .unwrap();
                    let f = self.module.get_function("lumia_concat").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[a.into(), b.into()], "concat")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "concat_i64")
                        .unwrap()
                        .into())
                }
                Builtin::Show => {
                    let arg = self.local(args[0])?;
                    match arg {
                        BasicValueEnum::FloatValue(f) => {
                            let fun = self.module.get_function("lumia_show_float").unwrap();
                            let call = self
                                .builder
                                .build_call(fun, &[f.into()], "show_float")
                                .unwrap();
                            let ptr = call
                                .try_as_basic_value()
                                .basic()
                                .unwrap()
                                .into_pointer_value();
                            Ok(self
                                .builder
                                .build_ptr_to_int(ptr, self.i64_ty, "show_i64")
                                .unwrap()
                                .into())
                        }
                        _ => {
                            let i = self.coerce_i64(arg)?;
                            let fun = self.module.get_function("lumia_show").unwrap();
                            let call =
                                self.builder.build_call(fun, &[i.into()], "show").unwrap();
                            let ptr = call
                                .try_as_basic_value()
                                .basic()
                                .unwrap()
                                .into_pointer_value();
                            Ok(self
                                .builder
                                .build_ptr_to_int(ptr, self.i64_ty, "show_i64")
                                .unwrap()
                                .into())
                        }
                    }
                }
                Builtin::Range => {
                    let a = self.coerce_i64(self.local(args[0])?)?;
                    let b = self.coerce_i64(self.local(args[1])?)?;
                    let f = self.module.get_function("lumia_range").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[a.into(), b.into()], "range")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "range_i64")
                        .unwrap()
                        .into())
                }
                Builtin::RangeInclusive => {
                    let a = self.coerce_i64(self.local(args[0])?)?;
                    let b = self.coerce_i64(self.local(args[1])?)?;
                    let f = self.module.get_function("lumia_range_inclusive").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[a.into(), b.into()], "range_inc")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "range_i64")
                        .unwrap()
                        .into())
                }
            },
            Value::If {
                cond,
                then_block,
                else_block,
            } => {
                let c = self.as_i64(self.local(*cond)?)?;
                let zero = self.i64_ty.const_int(0, false);
                let cond_i1 = self
                    .builder
                    .build_int_compare(IntPredicate::NE, c, zero, "ifcond")
                    .unwrap();
                let then_bb = self.context.append_basic_block(fv, "then");
                let else_bb = self.context.append_basic_block(fv, "else");
                let merge_bb = self.context.append_basic_block(fv, "merge");
                self.builder
                    .build_conditional_branch(cond_i1, then_bb, else_bb)
                    .unwrap();

                self.builder.position_at_end(then_bb);
                let then_raw = self
                    .emit_block(then_block, fv)?
                    .unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
                let then_terminated = self
                    .builder
                    .get_insert_block()
                    .and_then(|bb| bb.get_terminator())
                    .is_some();
                let mut then_incoming = None;
                if !then_terminated {
                    let then_v = self.coerce_i64(then_raw)?;
                    let then_bb_end = self.builder.get_insert_block().unwrap();
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                    then_incoming = Some((then_v, then_bb_end));
                }

                self.builder.position_at_end(else_bb);
                let else_raw = self
                    .emit_block(else_block, fv)?
                    .unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
                let else_terminated = self
                    .builder
                    .get_insert_block()
                    .and_then(|bb| bb.get_terminator())
                    .is_some();
                let mut else_incoming = None;
                if !else_terminated {
                    let else_v = self.coerce_i64(else_raw)?;
                    let else_bb_end = self.builder.get_insert_block().unwrap();
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                    else_incoming = Some((else_v, else_bb_end));
                }

                self.builder.position_at_end(merge_bb);
                match (then_incoming, else_incoming) {
                    (Some((tv, tb)), Some((ev, eb))) => {
                        let phi = self.builder.build_phi(self.i64_ty, "iftmp").unwrap();
                        phi.add_incoming(&[(&tv, tb), (&ev, eb)]);
                        Ok(phi.as_basic_value())
                    }
                    (Some((tv, _)), None) | (None, Some((tv, _))) => Ok(tv.into()),
                    (None, None) => {
                        // both sides break/continue — merge is unreachable; keep builder there
                        Ok(self.i64_ty.const_int(0, false).into())
                    }
                }
            }
            Value::Loop {
                header,
                body,
                latch,
            } => {
                let header_bb = self.context.append_basic_block(fv, "loop_header");
                let body_bb = self.context.append_basic_block(fv, "loop_body");
                let latch_bb = self.context.append_basic_block(fv, "loop_latch");
                let exit_bb = self.context.append_basic_block(fv, "loop_exit");
                self.builder.build_unconditional_branch(header_bb).unwrap();

                // continue → latch (runs step); break → exit
                self.loop_stack.push((latch_bb, exit_bb));

                self.builder.position_at_end(header_bb);
                let cond_raw = self
                    .emit_block(header, fv)?
                    .unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
                if self
                    .builder
                    .get_insert_block()
                    .and_then(|bb| bb.get_terminator())
                    .is_none()
                {
                    let c = self.coerce_i64(cond_raw)?;
                    let zero = self.i64_ty.const_int(0, false);
                    let cond_i1 = self
                        .builder
                        .build_int_compare(IntPredicate::NE, c, zero, "loopcond")
                        .unwrap();
                    self.builder
                        .build_conditional_branch(cond_i1, body_bb, exit_bb)
                        .unwrap();
                }

                self.builder.position_at_end(body_bb);
                let _ = self.emit_block(body, fv)?;
                if self
                    .builder
                    .get_insert_block()
                    .and_then(|bb| bb.get_terminator())
                    .is_none()
                {
                    self.builder.build_unconditional_branch(latch_bb).unwrap();
                }

                self.builder.position_at_end(latch_bb);
                let _ = self.emit_block(latch, fv)?;
                if self
                    .builder
                    .get_insert_block()
                    .and_then(|bb| bb.get_terminator())
                    .is_none()
                {
                    self.builder.build_unconditional_branch(header_bb).unwrap();
                }

                self.loop_stack.pop();
                self.builder.position_at_end(exit_bb);
                Ok(self.i64_ty.const_int(0, false).into())
            }
            Value::Lambda { .. } => bail!("lambda should have been lifted to FunRef/AllocClosure"),
            Value::AllocClosure { fun, captures } => {
                let n = captures.len() as u64;
                let nbytes = self.i64_ty.const_int((1 + n) * 8, false);
                let type_id = self.context.i32_type().const_int(8, false); // TYPE_CLOSURE
                let alloc = self.module.get_function("lumia_alloc").unwrap();
                let ptr = self
                    .builder
                    .build_call(alloc, &[nbytes.into(), type_id.into()], "clos_alloc")
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                let fv = self
                    .functions
                    .get(fun)
                    .copied()
                    .with_context(|| format!("unknown closure fun {fun}"))?;
                let fn_as_i = self
                    .builder
                    .build_ptr_to_int(
                        fv.as_global_value().as_pointer_value(),
                        self.i64_ty,
                        "clos_fn_i",
                    )
                    .unwrap();
                let fn_slot = unsafe {
                    self.builder
                        .build_gep(
                            self.i64_ty,
                            ptr,
                            &[self.i64_ty.const_int(0, false)],
                            "clos_fn_slot",
                        )
                        .unwrap()
                };
                self.builder.build_store(fn_slot, fn_as_i).unwrap();
                for (i, e) in captures.iter().enumerate() {
                    let v = self.coerce_i64(self.local(*e)?)?;
                    let slot = unsafe {
                        self.builder
                            .build_gep(
                                self.i64_ty,
                                ptr,
                                &[self.i64_ty.const_int((i + 1) as u64, false)],
                                "clos_cap",
                            )
                            .unwrap()
                    };
                    self.builder.build_store(slot, v).unwrap();
                }
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "clos_as_i64")
                    .unwrap()
                    .into())
            }
            Value::ClosureCap { env, index } => {
                let env_i = self.coerce_i64(self.local(*env)?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let env_ptr = self
                    .builder
                    .build_int_to_ptr(env_i, ptr_ty, "cap_env")
                    .unwrap();
                let slot = unsafe {
                    self.builder
                        .build_gep(
                            self.i64_ty,
                            env_ptr,
                            &[self.i64_ty.const_int((*index as u64) + 1, false)],
                            "cap_slot",
                        )
                        .unwrap()
                };
                Ok(self
                    .builder
                    .build_load(self.i64_ty, slot, "cap")
                    .unwrap())
            }
            Value::AllocList { elems, .. } => {
                self.emit_heap_array(elems, 3 /* TYPE_LIST */)
            }
            Value::AllocSet { elems } => self.emit_heap_array(elems, 5 /* TYPE_SET */),
            Value::AllocMap { flat_pairs, .. } => {
                // Layout: [len][k0][v0]... — len = pair count
                if flat_pairs.len() % 2 != 0 {
                    bail!("mapOf expects even number of key/value args");
                }
                let n_pairs = (flat_pairs.len() / 2) as u64;
                let nbytes = self.i64_ty.const_int((1 + flat_pairs.len() as u64) * 8, false);
                let type_id = self.context.i32_type().const_int(4, false); // TYPE_MAP
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
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "map_as_i64")
                    .unwrap()
                    .into())
            }
            Value::AllocAdt { tag, fields } => {
                let n = fields.len() as u64;
                let nbytes = self.i64_ty.const_int((1 + n) * 8, false);
                let type_id = self.context.i32_type().const_int(6, false); // TYPE_ADT
                let alloc = self.module.get_function("lumia_alloc").unwrap();
                let ptr = self
                    .builder
                    .build_call(alloc, &[nbytes.into(), type_id.into()], "adt_alloc")
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
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
                    .build_store(tag_slot, self.i64_ty.const_int(*tag as u64, false))
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
        }
    }

    fn emit_heap_array(
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

fn link_executable(obj: &Path, runtime: &Path, output: &Path) -> Result<()> {
    let status = Command::new("clang")
        .arg(obj)
        .arg(runtime)
        .arg("-o")
        .arg(output)
        .arg("-lpthread")
        .arg("-ldl")
        .arg("-lm")
        .status()
        .context("invoke clang linker")?;
    if !status.success() {
        bail!("link failed with {status}");
    }
    Ok(())
}

/// Locate `liblumia_rt.a` in target dir.
pub fn find_runtime_lib(target_dir: &Path) -> Result<PathBuf> {
    let candidates = [
        target_dir.join("debug/liblumia_rt.a"),
        target_dir.join("release/liblumia_rt.a"),
        target_dir.join("liblumia_rt.a"),
    ];
    for c in candidates {
        if c.exists() {
            return Ok(c);
        }
    }
    bail!(
        "liblumia_rt.a not found under {} — run `cargo build -p lumia_rt` first",
        target_dir.display()
    )
}
