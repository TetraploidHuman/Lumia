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
use lumia_core::{Block, CoreFun, CoreModule, Local, Op, Value, MemoTf};
use lumia_hir::Builtin;
use lumia_syntax::{BinOp, UnOp};
use lumia_ty::Type;
use std::collections::{HashMap, HashSet};
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
    /// Auto-parallel pure list maps (DESIGN §11.1).
    pub parallel: bool,
    /// Extra linker args, e.g. `["-lm", "-L/opt/lib", "-lfoo"]`.
    pub link_args: Vec<String>,
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
        let name = if f.is_main {
            "lumia_user_main".to_string()
        } else if let Some(sym) = &f.external {
            sym.clone()
        } else {
            f.name.clone()
        };
        let fv = if f.external.is_some() {
            let fn_ty = c_abi_fn_type(&context, &f.param_tys, &f.ret_ty);
            let fv = cg.module.add_function(&name, fn_ty, None);
            fv.set_linkage(inkwell::module::Linkage::External);
            cg.external_funs.insert(f.name.clone());
            fv
        } else {
            let fn_ty = cg.i64_ty.fn_type(
                &vec![BasicMetadataTypeEnum::from(cg.i64_ty); f.params.len()],
                false,
            );
            cg.module.add_function(&name, fn_ty, None)
        };
        cg.functions.insert(f.name.clone(), fv);
        cg.fun_ret_tys.insert(f.name.clone(), f.ret_ty.clone());
        cg.fun_param_tys.insert(f.name.clone(), f.param_tys.clone());
    }

    for f in &core.functions {
        if f.external.is_some() {
            continue;
        }
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

    link_executable(&obj_path, &opts.runtime_lib, &opts.output, &opts.link_args)?;
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
        "lumia_string_cstr",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_cstr_to_string",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
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
        "lumia_list_set",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_set",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_map_remove",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_set_insert",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_remove",
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
    module.add_function(
        "lumia_str_trim",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_len",
        i64_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_to_lower",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_to_upper",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_split",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_substring",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_take",
        ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_reverse",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_sort",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_sort_by_keys",
        ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_par_map",
        ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_list_join",
        ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_read_stdin",
        ptr_ty.fn_type(&[], false),
        None,
    );
    module.add_function("lumia_match_fail", void_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_assert",
        void_ty.fn_type(
            &[
                i64_ty.into(),
                ptr_ty.into(),
                i64_ty.into(),
            ],
            false,
        ),
        None,
    );
    module.add_function("lumia_trap_div0", void_ty.fn_type(&[], false), None);
    module.add_function("lumia_trap_overflow", void_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_str_starts_with",
        i64_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_str_ends_with",
        i64_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_memo_l2_lookup",
        i64_ty.fn_type(
            &[
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                ptr_ty.into(),
            ],
            false,
        ),
        None,
    );
    module.add_function(
        "lumia_memo_l2_store",
        context.void_type().fn_type(
            &[
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
                i64_ty.into(),
            ],
            false,
        ),
        None,
    );
    module.add_function(
        "lumia_memo_idx_lookup",
        i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_memo_idx_store",
        context
            .void_type()
            .fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false),
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
    /// Mutable names whose payload is IEEE-754 bits (restore as f64 on load).
    float_slots: HashSet<String>,
    /// Nested loop targets: (continue_bb, break_bb, shadow-stack depth at loop entry)
    loop_stack: Vec<(BasicBlock<'ctx>, BasicBlock<'ctx>, u32)>,
    option_some_tag: i64,
    option_none_tag: i64,
    /// Stable arg snapshots for Memo L2 store (entry-time values).
    memo_arg_slots: Vec<PointerValue<'ctx>>,
    /// Entry-time Int key for dense index Memo.
    memo_idx_key: Option<PointerValue<'ctx>>,
    /// Callee return types for float ABI restore after call.
    fun_ret_tys: HashMap<String, Type>,
    /// Callee parameter types (C ABI for `foreign`).
    fun_param_tys: HashMap<String, Vec<Type>>,
    /// Lumia names of `foreign` imports.
    external_funs: HashSet<String>,
    /// Locals currently bound to `FunRef(name)` — for IndirectCall float ABI.
    funref_locals: HashMap<u32, String>,
    /// Shadow-stack pushes currently live in this function (LIFO).
    root_depth: u32,
    /// Mutable slots that have already been registered as GC roots.
    rooted_slots: HashSet<String>,
    /// Function entry block — all GC root allocas go here (avoid loop stack growth).
    entry_bb: Option<BasicBlock<'ctx>>,
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
            float_slots: HashSet::new(),
            loop_stack: Vec::new(),
            option_some_tag,
            option_none_tag,
            memo_arg_slots: Vec::new(),
            memo_idx_key: None,
            fun_ret_tys: HashMap::new(),
            fun_param_tys: HashMap::new(),
            external_funs: HashSet::new(),
            funref_locals: HashMap::new(),
            root_depth: 0,
            rooted_slots: HashSet::new(),
            entry_bb: None,
        }
    }

    fn type_may_heap(ty: &Type) -> bool {
        match ty {
            Type::String
            | Type::Char
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Set(_)
            | Type::Adt { .. }
            | Type::Fun(_, _, _) => true,
            Type::Tuple(ts) => ts.iter().any(Self::type_may_heap),
            _ => false,
        }
    }

    fn value_may_heap(&self, v: &Value) -> bool {
        match v {
            Value::String(_) | Value::Char(_) => true,
            Value::AllocList { .. }
            | Value::AllocSet { .. }
            | Value::AllocMap { .. }
            | Value::AllocAdt { .. }
            | Value::AllocClosure { .. } => true,
            Value::ClosureCap { .. } => true,
            Value::IndirectCall { .. } => true,
            // Only when an arm's result may be heap — parent `Let` re-roots after
            // scoped pop. Pure Int/Unit ifs must not allocate root slots.
            Value::If {
                then_block,
                else_block,
                ..
            } => {
                self.block_result_may_heap(then_block) || self.block_result_may_heap(else_block)
            }
            Value::Call { fun, .. } => self
                .fun_ret_tys
                .get(fun)
                .map(Self::type_may_heap)
                .unwrap_or(true),
            Value::Builtin { name, .. } => matches!(
                name,
                Builtin::ListGet
                    | Builtin::ListSlice
                    | Builtin::ListAppend
                    | Builtin::ListConcat
                    | Builtin::ListTake
                    | Builtin::ListReverse
                    | Builtin::ListSort
                    | Builtin::ListSortByKeys
                    | Builtin::ListParMap
                    | Builtin::ListJoin
                    | Builtin::MapSet
                    | Builtin::MapRemove
                    | Builtin::SetInsert
                    | Builtin::MapKeys
                    | Builtin::MapValues
                    | Builtin::MapItems
                    | Builtin::Range
                    | Builtin::RangeInclusive
                    | Builtin::Show
                    | Builtin::StrTrim
                    | Builtin::StrSplit
                    | Builtin::StrSubstring
                    | Builtin::StrToLower
                    | Builtin::StrToUpper
                    | Builtin::ReadStdin
                    | Builtin::AdtField
            ),
            _ => false,
        }
    }

    /// Whether a block's SSA result may be a heap pointer (for If re-rooting).
    fn block_result_may_heap(&self, block: &Block) -> bool {
        let Some(r) = block.result else {
            return false;
        };
        for op in block.ops.iter().rev() {
            if let Op::Let { local, value, .. } = op {
                if *local == r {
                    return self.value_may_heap(value);
                }
            }
        }
        // Result is an outer local — already rooted at its definition.
        false
    }

    fn root_push_i64(&mut self, bits: IntValue<'ctx>) -> Result<()> {
        let slot = self.alloca_in_entry(self.i64_ty, "gc_root")?;
        self.builder.build_store(slot, bits).unwrap();
        let push = self.module.get_function("lumia_root_push").unwrap();
        self.builder
            .build_call(push, &[slot.into()], "")
            .unwrap();
        self.root_depth += 1;
        Ok(())
    }

    /// `alloca` at function entry so loops do not grow the native stack.
    fn alloca_in_entry(
        &mut self,
        ty: IntType<'ctx>,
        name: &str,
    ) -> Result<PointerValue<'ctx>> {
        let entry = self
            .entry_bb
            .context("alloca_in_entry before emit_function")?;
        let cur = self
            .builder
            .get_insert_block()
            .context("no insert block")?;
        // Insert before the first non-alloca, or at end if entry is empty/only allocas.
        match entry.get_first_instruction() {
            Some(first) => self.builder.position_before(&first),
            None => self.builder.position_at_end(entry),
        }
        let slot = self.builder.build_alloca(ty, name).unwrap();
        self.builder.position_at_end(cur);
        Ok(slot)
    }

    fn root_register_slot(&mut self, slot: PointerValue<'ctx>, name: &str) {
        if self.rooted_slots.contains(name) {
            return;
        }
        let push = self.module.get_function("lumia_root_push").unwrap();
        self.builder
            .build_call(push, &[slot.into()], "")
            .unwrap();
        self.root_depth += 1;
        self.rooted_slots.insert(name.to_string());
    }

    /// Pop shadow-stack entries until `root_depth == depth` (scope exit).
    fn root_pop_to(&mut self, depth: u32) {
        debug_assert!(self.root_depth >= depth);
        let pop = self.module.get_function("lumia_root_pop").unwrap();
        while self.root_depth > depth {
            self.builder.build_call(pop, &[], "").unwrap();
            self.root_depth -= 1;
        }
    }

    fn emit_root_epilogue(&mut self) {
        // Emit pops for the current compile-time depth without clearing it:
        // early returns (memo hit) share the counter with the compute path.
        let pop = self.module.get_function("lumia_root_pop").unwrap();
        for _ in 0..self.root_depth {
            self.builder.build_call(pop, &[], "").unwrap();
        }
    }

    fn emit_return_i64(&mut self, ret: IntValue<'ctx>) {
        self.emit_root_epilogue();
        self.builder.build_return(Some(&ret)).unwrap();
    }

    fn emit_function(&mut self, fun: &CoreFun) -> Result<()> {
        let fv = *self
            .functions
            .get(&fun.name)
            .context("missing function decl")?;
        let entry = self.context.append_basic_block(fv, "entry");
        self.builder.position_at_end(entry);
        self.entry_bb = Some(entry);
        self.locals.clear();
        self.slots.clear();
        self.float_slots.clear();
        self.loop_stack.clear();
        self.memo_arg_slots.clear();
        self.memo_idx_key = None;
        self.root_depth = 0;
        self.rooted_slots.clear();
        self.funref_locals.clear();

        for (i, p) in fun.params.iter().enumerate() {
            let av = fv.get_nth_param(i as u32).unwrap();
            let ty = fun.param_tys.get(i).cloned().unwrap_or(Type::Int);
            if matches!(ty, Type::Float) {
                let bits = av.into_int_value();
                let f = self
                    .builder
                    .build_bit_cast(bits, self.context.f64_type(), "arg_f64")
                    .unwrap();
                self.locals.insert(p.0, f.into());
            } else {
                self.locals.insert(p.0, av);
                if Self::type_may_heap(&ty) {
                    let bits = self.coerce_i64(av)?;
                    self.root_push_i64(bits)?;
                }
            }
        }

        let compute_bb = match fun.memo {
            Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_prologue(fun, fv, id)?,
            Some(MemoTf::Slots { id }) => self.emit_memo_l2_prologue(fun, fv, id)?,
            None => entry,
        };
        if fun.memo.is_some() {
            self.builder.position_at_end(compute_bb);
        }

        let result = self.emit_block(&fun.body, fv)?;
        let ret = result.unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
        let ret_i = if matches!(fun.ret_ty, Type::Float) {
            match ret {
                BasicValueEnum::FloatValue(f) => self
                    .builder
                    .build_bit_cast(f, self.i64_ty, "ret_f64_bits")
                    .unwrap()
                    .into_int_value(),
                other => self.coerce_i64(other)?,
            }
        } else {
            self.coerce_i64(ret)?
        };

        match fun.memo {
            Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_store(id, ret_i)?,
            Some(MemoTf::Slots { id }) => self.emit_memo_l2_store(id, ret_i)?,
            None => {}
        }
        self.emit_return_i64(ret_i);
        Ok(())
    }

    fn memo_arg_values(&self) -> [IntValue<'ctx>; 4] {
        let z = self.i64_ty.const_int(0, false);
        let mut out = [z; 4];
        for (i, slot) in self.memo_arg_slots.iter().enumerate().take(4) {
            out[i] = self
                .builder
                .build_load(self.i64_ty, *slot, &format!("memo_a{i}"))
                .unwrap()
                .into_int_value();
        }
        out
    }

    /// On hit: branch to return cached. On miss: fall through to `compute` BB.
    /// Captures parameters into allocas so store uses entry-time keys.
    fn emit_memo_l2_prologue(
        &mut self,
        fun: &CoreFun,
        fv: FunctionValue<'ctx>,
        mid: u32,
    ) -> Result<BasicBlock<'ctx>> {
        let out_alloca = self
            .builder
            .build_alloca(self.i64_ty, "memo_out")
            .unwrap();
        self.memo_arg_slots.clear();
        for (i, p) in fun.params.iter().enumerate().take(4) {
            let slot = self
                .builder
                .build_alloca(self.i64_ty, &format!("memo_arg{i}"))
                .unwrap();
            let v = self.coerce_i64(self.local(*p)?)?;
            self.builder.build_store(slot, v).unwrap();
            self.memo_arg_slots.push(slot);
        }
        let nargs = self.i64_ty.const_int(fun.params.len().min(4) as u64, false);
        let args = self.memo_arg_values();
        let lookup = self.module.get_function("lumia_memo_l2_lookup").unwrap();
        let hit = self
            .builder
            .build_call(
                lookup,
                &[
                    self.i64_ty.const_int(mid as u64, false).into(),
                    nargs.into(),
                    args[0].into(),
                    args[1].into(),
                    args[2].into(),
                    args[3].into(),
                    out_alloca.into(),
                ],
                "memo_hit",
            )
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_int_value();
        let is_hit = self
            .builder
            .build_int_compare(
                IntPredicate::NE,
                hit,
                self.i64_ty.const_int(0, false),
                "memo_is_hit",
            )
            .unwrap();
        let hit_bb = self.context.append_basic_block(fv, "memo_hit_ret");
        let compute_bb = self.context.append_basic_block(fv, "memo_compute");
        self.builder
            .build_conditional_branch(is_hit, hit_bb, compute_bb)
            .unwrap();

        self.builder.position_at_end(hit_bb);
        let cached = self
            .builder
            .build_load(self.i64_ty, out_alloca, "memo_cached")
            .unwrap()
            .into_int_value();
        self.emit_return_i64(cached);
        Ok(compute_bb)
    }

    fn emit_memo_idx_prologue(
        &mut self,
        fun: &CoreFun,
        fv: FunctionValue<'ctx>,
        mid: u32,
    ) -> Result<BasicBlock<'ctx>> {
        let p0 = fun
            .params
            .first()
            .copied()
            .context("memo_index requires one param")?;
        let key = self.coerce_i64(self.local(p0)?)?;
        let key_slot = self
            .builder
            .build_alloca(self.i64_ty, "memo_idx_key")
            .unwrap();
        self.builder.build_store(key_slot, key).unwrap();
        self.memo_idx_key = Some(key_slot);

        let out_alloca = self
            .builder
            .build_alloca(self.i64_ty, "memo_idx_out")
            .unwrap();
        let lookup = self.module.get_function("lumia_memo_idx_lookup").unwrap();
        let hit = self
            .builder
            .build_call(
                lookup,
                &[
                    self.i64_ty.const_int(mid as u64, false).into(),
                    key.into(),
                    out_alloca.into(),
                ],
                "memo_idx_hit",
            )
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_int_value();
        let is_hit = self
            .builder
            .build_int_compare(
                IntPredicate::NE,
                hit,
                self.i64_ty.const_int(0, false),
                "memo_idx_is_hit",
            )
            .unwrap();
        let hit_bb = self.context.append_basic_block(fv, "memo_idx_hit_ret");
        let compute_bb = self.context.append_basic_block(fv, "memo_idx_compute");
        self.builder
            .build_conditional_branch(is_hit, hit_bb, compute_bb)
            .unwrap();

        self.builder.position_at_end(hit_bb);
        let cached = self
            .builder
            .build_load(self.i64_ty, out_alloca, "memo_idx_cached")
            .unwrap()
            .into_int_value();
        self.emit_return_i64(cached);
        Ok(compute_bb)
    }

    fn emit_memo_idx_store(&mut self, mid: u32, result: IntValue<'ctx>) -> Result<()> {
        let key_slot = self
            .memo_idx_key
            .context("memo_idx store without key slot")?;
        let key = self
            .builder
            .build_load(self.i64_ty, key_slot, "memo_idx_key_ld")
            .unwrap()
            .into_int_value();
        let store = self.module.get_function("lumia_memo_idx_store").unwrap();
        self.builder
            .build_call(
                store,
                &[
                    self.i64_ty.const_int(mid as u64, false).into(),
                    key.into(),
                    result.into(),
                ],
                "",
            )
            .unwrap();
        Ok(())
    }

    fn emit_memo_l2_store(&mut self, mid: u32, result: IntValue<'ctx>) -> Result<()> {
        let nargs = self
            .i64_ty
            .const_int(self.memo_arg_slots.len() as u64, false);
        let args = self.memo_arg_values();
        let store = self.module.get_function("lumia_memo_l2_store").unwrap();
        self.builder
            .build_call(
                store,
                &[
                    self.i64_ty.const_int(mid as u64, false).into(),
                    nargs.into(),
                    args[0].into(),
                    args[1].into(),
                    args[2].into(),
                    args[3].into(),
                    result.into(),
                ],
                "",
            )
            .unwrap();
        Ok(())
    }

    fn ensure_slot(&mut self, name: &str) -> PointerValue<'ctx> {
        if let Some(p) = self.slots.get(name) {
            return *p;
        }
        // Must be entry alloca — loop-body alloca grows the native stack each iteration.
        let alloca = self
            .alloca_in_entry(self.i64_ty, &format!("mut_{name}"))
            .expect("ensure_slot alloca");
        self.builder
            .build_store(alloca, self.i64_ty.const_int(0, false))
            .unwrap();
        self.root_register_slot(alloca, name);
        self.slots.insert(name.to_string(), alloca);
        alloca
    }

    fn store_slot(&mut self, name: &str, v: BasicValueEnum<'ctx>) -> Result<()> {
        if matches!(v, BasicValueEnum::FloatValue(_)) {
            // Float slots are not heap roots; create without rooting.
            if !self.slots.contains_key(name) {
                let alloca = self.alloca_in_entry(self.i64_ty, &format!("mut_{name}"))?;
                self.slots.insert(name.to_string(), alloca);
            }
            self.float_slots.insert(name.to_string());
        }
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
        let bits = self.builder.build_load(self.i64_ty, slot, name).unwrap();
        if self.float_slots.contains(name) {
            Ok(self
                .builder
                .build_bit_cast(bits.into_int_value(), self.context.f64_type(), "mut_f64")
                .unwrap()
                .into())
        } else {
            Ok(bits)
        }
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
                    if self.value_may_heap(value) {
                        if let Ok(bits) = self.coerce_i64(v) {
                            self.root_push_i64(bits)?;
                        }
                    }
                    self.locals.insert(local.0, v);
                    if let Value::FunRef(name) = value {
                        self.funref_locals.insert(local.0, name.clone());
                    } else if let Value::Local(Local(src)) = value {
                        if let Some(n) = self.funref_locals.get(src).cloned() {
                            self.funref_locals.insert(local.0, n);
                        } else {
                            self.funref_locals.remove(&local.0);
                        }
                    } else {
                        self.funref_locals.remove(&local.0);
                    }
                }
                Op::Effect { value } => {
                    let _ = self.emit_value(value, fv)?;
                }
                Op::Assign { name, value } => {
                    let v = self.local(*value)?;
                    self.store_slot(name, v)?;
                }
                Op::Break => {
                    let (_, break_bb, loop_depth) = self
                        .loop_stack
                        .last()
                        .copied()
                        .context("break outside loop")?;
                    self.root_pop_to(loop_depth);
                    self.builder.build_unconditional_branch(break_bb).unwrap();
                }
                Op::Continue => {
                    let (cont_bb, _, loop_depth) = self
                        .loop_stack
                        .last()
                        .copied()
                        .context("continue outside loop")?;
                    self.root_pop_to(loop_depth);
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

    /// Emit a nested block and drop roots pushed inside it (unless it terminated
    /// via break/continue, which already restored the loop entry depth).
    fn emit_scoped_block(
        &mut self,
        block: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let depth = self.root_depth;
        let result = self.emit_block(block, fv)?;
        let terminated = self
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some();
        if !terminated {
            self.root_pop_to(depth);
        }
        Ok(result)
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
            BasicValueEnum::FloatValue(f) => Ok(self
                .builder
                .build_bit_cast(f, self.i64_ty, "f64_bits")
                .unwrap()
                .into_int_value()),
            BasicValueEnum::PointerValue(p) => Ok(self
                .builder
                .build_ptr_to_int(p, self.i64_ty, "ptr_i64")
                .unwrap()),
            _ => bail!("expected i64 value"),
        }
    }

    fn coerce_i64(&self, v: BasicValueEnum<'ctx>) -> Result<IntValue<'ctx>> {
        self.as_i64(v)
    }

    /// Coerce a Lumia local to a C ABI argument for `foreign` calls.
    fn emit_c_abi_arg(
        &mut self,
        local: Local,
        ty: &Type,
    ) -> Result<BasicMetadataValueEnum<'ctx>> {
        match ty {
            Type::Float => Ok(self.promote_f64(self.local(local)?)?.into()),
            Type::Bool => {
                let i = self.coerce_i64(self.local(local)?)?;
                Ok(self
                    .builder
                    .build_int_truncate(i, self.context.i8_type(), "c_bool")
                    .unwrap()
                    .into())
            }
            Type::String => {
                let s_i = self.coerce_i64(self.local(local)?)?;
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let s = self
                    .builder
                    .build_int_to_ptr(s_i, ptr_ty, "cstr_in")
                    .unwrap();
                let f = self.module.get_function("lumia_string_cstr").unwrap();
                let call = self.builder.build_call(f, &[s.into()], "cstr").unwrap();
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value()
                    .into())
            }
            _ => Ok(self.coerce_i64(self.local(local)?)?.into()),
        }
    }

    fn restore_c_abi_ret(
        &self,
        fun: &str,
        call: inkwell::values::CallSiteValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>> {
        let ret = self.fun_ret_tys.get(fun).cloned().unwrap_or(Type::Int);
        match ret {
            Type::Unit => Ok(self.i64_ty.const_int(0, false).into()),
            Type::Float => {
                let f = call
                    .try_as_basic_value()
                    .basic()
                    .context("foreign float return")?
                    .into_float_value();
                Ok(f.into())
            }
            Type::Bool => {
                let b = call
                    .try_as_basic_value()
                    .basic()
                    .context("foreign bool return")?
                    .into_int_value();
                Ok(self
                    .builder
                    .build_int_z_extend(b, self.i64_ty, "bool_i64")
                    .unwrap()
                    .into())
            }
            Type::String => {
                let p = call
                    .try_as_basic_value()
                    .basic()
                    .context("foreign string return")?
                    .into_pointer_value();
                let f = self.module.get_function("lumia_cstr_to_string").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[p.into()], "cstr_to_str")
                    .unwrap();
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                Ok(self
                    .builder
                    .build_ptr_to_int(ptr, self.i64_ty, "str_ret")
                    .unwrap()
                    .into())
            }
            _ => Ok(call
                .try_as_basic_value()
                .basic()
                .unwrap_or_else(|| self.i64_ty.const_int(0, false).into())),
        }
    }

    fn promote_f64(
        &self,
        v: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::FloatValue<'ctx>> {
        let fty = self.context.f64_type();
        match v {
            BasicValueEnum::FloatValue(f) => Ok(f),
            // Float ABI: values travel as i64 bit patterns (not numeric conversion).
            BasicValueEnum::IntValue(i) => Ok(self
                .builder
                .build_bit_cast(i, fty, "i64_bits_f64")
                .unwrap()
                .into_float_value()),
            _ => bail!("expected numeric for float promote"),
        }
    }

    fn emit_checked_neg(
        &mut self,
        o: IntValue<'ctx>,
        fv: FunctionValue<'ctx>,
    ) -> Result<IntValue<'ctx>> {
        let min = self.i64_ty.const_int(i64::MIN as u64, true);
        let is_min = self
            .builder
            .build_int_compare(IntPredicate::EQ, o, min, "neg_min")
            .unwrap();
        let trap_bb = self.context.append_basic_block(fv, "neg_overflow_trap");
        let ok_bb = self.context.append_basic_block(fv, "neg_ok");
        self.builder
            .build_conditional_branch(is_min, trap_bb, ok_bb)
            .unwrap();
        self.builder.position_at_end(trap_bb);
        let trap = self.module.get_function("lumia_trap_overflow").unwrap();
        self.builder.build_call(trap, &[], "trap_neg").unwrap();
        self.builder.build_unreachable().unwrap();
        self.builder.position_at_end(ok_bb);
        Ok(self.builder.build_int_neg(o, "neg").unwrap())
    }

    fn emit_checked_binop(
        &mut self,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        fv: FunctionValue<'ctx>,
        kind: &str,
    ) -> Result<IntValue<'ctx>> {
        let name = format!("llvm.{kind}.with.overflow.i64");
        let intrinsic = inkwell::intrinsics::Intrinsic::find(&name)
            .with_context(|| format!("missing intrinsic {name}"))?;
        let id_tys = [self.i64_ty.into()];
        let fnty = intrinsic.get_declaration(&self.module, &id_tys).unwrap();
        let call = self
            .builder
            .build_call(fnty, &[l.into(), r.into()], "checked")
            .unwrap();
        let agg = call
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_struct_value();
        let result = self
            .builder
            .build_extract_value(agg, 0, "ov_res")
            .unwrap()
            .into_int_value();
        let overflow = self
            .builder
            .build_extract_value(agg, 1, "ov_flag")
            .unwrap()
            .into_int_value();
        let trap_bb = self.context.append_basic_block(fv, "overflow_trap");
        let ok_bb = self.context.append_basic_block(fv, "overflow_ok");
        self.builder
            .build_conditional_branch(overflow, trap_bb, ok_bb)
            .unwrap();
        self.builder.position_at_end(trap_bb);
        let trap = self.module.get_function("lumia_trap_overflow").unwrap();
        self.builder.build_call(trap, &[], "trap_ov").unwrap();
        self.builder.build_unreachable().unwrap();
        self.builder.position_at_end(ok_bb);
        Ok(result)
    }

    fn emit_checked_div_rem(
        &mut self,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        fv: FunctionValue<'ctx>,
        is_rem: bool,
    ) -> Result<IntValue<'ctx>> {
        let zero = self.i64_ty.const_int(0, false);
        let minus_one = self.i64_ty.const_int((-1i64) as u64, true);
        let i64_min = self.i64_ty.const_int(i64::MIN as u64, true);
        let is_zero = self
            .builder
            .build_int_compare(IntPredicate::EQ, r, zero, "div0")
            .unwrap();
        let is_m1 = self
            .builder
            .build_int_compare(IntPredicate::EQ, r, minus_one, "div_m1")
            .unwrap();
        let is_min = self
            .builder
            .build_int_compare(IntPredicate::EQ, l, i64_min, "div_min")
            .unwrap();
        let ov = self.builder.build_and(is_m1, is_min, "div_ov").unwrap();
        let bad = self.builder.build_or(is_zero, ov, "div_bad").unwrap();
        let trap_bb = self.context.append_basic_block(fv, "div_trap");
        let ok_bb = self.context.append_basic_block(fv, "div_ok");
        self.builder
            .build_conditional_branch(bad, trap_bb, ok_bb)
            .unwrap();
        self.builder.position_at_end(trap_bb);
        let div0_bb = self.context.append_basic_block(fv, "div0_trap");
        let ov_bb = self.context.append_basic_block(fv, "div_ov_trap");
        self.builder
            .build_conditional_branch(is_zero, div0_bb, ov_bb)
            .unwrap();
        self.builder.position_at_end(div0_bb);
        let t0 = self.module.get_function("lumia_trap_div0").unwrap();
        self.builder.build_call(t0, &[], "trap0").unwrap();
        self.builder.build_unreachable().unwrap();
        self.builder.position_at_end(ov_bb);
        let t1 = self.module.get_function("lumia_trap_overflow").unwrap();
        self.builder.build_call(t1, &[], "trap_ov").unwrap();
        self.builder.build_unreachable().unwrap();
        self.builder.position_at_end(ok_bb);
        Ok(if is_rem {
            self.builder.build_int_signed_rem(l, r, "rem").unwrap()
        } else {
            self.builder.build_int_signed_div(l, r, "div").unwrap()
        })
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
                let either_float = matches!(lv, BasicValueEnum::FloatValue(_))
                    || matches!(rv, BasicValueEnum::FloatValue(_));
                if either_float
                    && matches!(
                        op,
                        BinOp::Add
                            | BinOp::Sub
                            | BinOp::Mul
                            | BinOp::Div
                            | BinOp::Rem
                            | BinOp::Eq
                            | BinOp::Ne
                            | BinOp::Lt
                            | BinOp::Le
                            | BinOp::Gt
                            | BinOp::Ge
                    )
                {
                    let l = self.promote_f64(lv)?;
                    let r = self.promote_f64(rv)?;
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
                        _ => unreachable!(),
                    };
                    return Ok(v.into());
                }
                let l = self.as_i64(lv)?;
                let r = self.as_i64(rv)?;
                let v = match op {
                    BinOp::Add => self.emit_checked_binop(l, r, fv, "sadd")?,
                    BinOp::Sub => self.emit_checked_binop(l, r, fv, "ssub")?,
                    BinOp::Mul => self.emit_checked_binop(l, r, fv, "smul")?,
                    BinOp::Div => self.emit_checked_div_rem(l, r, fv, false)?,
                    BinOp::Rem => self.emit_checked_div_rem(l, r, fv, true)?,
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
                    UnOp::Neg => self.emit_checked_neg(o, fv)?,
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
                let is_ext = self.external_funs.contains(fun);
                let param_tys = self.fun_param_tys.get(fun).cloned().unwrap_or_default();
                let mut av: Vec<BasicMetadataValueEnum> = vec![];
                for (i, a) in args.iter().enumerate() {
                    let pty = param_tys.get(i).unwrap_or(&Type::Int);
                    if is_ext {
                        av.push(self.emit_c_abi_arg(*a, pty)?);
                    } else {
                        let v = self.coerce_i64(self.local(*a)?)?;
                        av.push(v.into());
                    }
                }
                let call = self.builder.build_call(callee, &av, "call").unwrap();
                if is_ext {
                    return self.restore_c_abi_ret(fun, call);
                }
                let raw = call
                    .try_as_basic_value()
                    .basic()
                    .unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
                if matches!(self.fun_ret_tys.get(fun), Some(Type::Float)) {
                    let bits = raw.into_int_value();
                    Ok(self
                        .builder
                        .build_bit_cast(bits, self.context.f64_type(), "call_f64")
                        .unwrap()
                        .into())
                } else {
                    Ok(raw)
                }
            }
            Value::IndirectCall { callee, args } => {
                let mut any_float_arg = false;
                for a in args {
                    if matches!(self.local(*a)?, BasicValueEnum::FloatValue(_)) {
                        any_float_arg = true;
                        break;
                    }
                }
                let float_ret = any_float_arg
                    || self
                        .funref_locals
                        .get(&callee.0)
                        .and_then(|name| self.fun_ret_tys.get(name))
                        .is_some_and(|ty| matches!(ty, Type::Float));
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
                let bits = phi.as_basic_value().into_int_value();
                if float_ret {
                    Ok(self
                        .builder
                        .build_bit_cast(bits, self.context.f64_type(), "icall_f64")
                        .unwrap()
                        .into())
                } else {
                    Ok(bits.into())
                }
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
                        .build_int_to_ptr(map_i, ptr_ty, "col_ptr")
                        .unwrap();
                    let f = self.module.get_function("lumia_set").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[map.into(), key.into(), val.into()], "col_set")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "set_i64")
                        .unwrap()
                        .into())
                }
                Builtin::MapRemove => {
                    let map_i = self.coerce_i64(self.local(args[0])?)?;
                    let key = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let map = self
                        .builder
                        .build_int_to_ptr(map_i, ptr_ty, "col_ptr")
                        .unwrap();
                    let f = self.module.get_function("lumia_remove").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[map.into(), key.into()], "col_rm")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "rm_i64")
                        .unwrap()
                        .into())
                }
                Builtin::SetInsert => {
                    let set_i = self.coerce_i64(self.local(args[0])?)?;
                    let elem = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let set = self
                        .builder
                        .build_int_to_ptr(set_i, ptr_ty, "set_ptr")
                        .unwrap();
                    let f = self.module.get_function("lumia_set_insert").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[set.into(), elem.into()], "set_ins")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "set_ins_i64")
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
                Builtin::StrTrim | Builtin::StrToLower | Builtin::StrToUpper => {
                    let s_i = self.coerce_i64(self.local(args[0])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let s = self.builder.build_int_to_ptr(s_i, ptr_ty, "str").unwrap();
                    let fname = match name {
                        Builtin::StrTrim => "lumia_str_trim",
                        Builtin::StrToLower => "lumia_str_to_lower",
                        _ => "lumia_str_to_upper",
                    };
                    let f = self.module.get_function(fname).unwrap();
                    let call = self.builder.build_call(f, &[s.into()], "str_op").unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "str_i64")
                        .unwrap()
                        .into())
                }
                Builtin::StrSplit => {
                    let s_i = self.coerce_i64(self.local(args[0])?)?;
                    let sep = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let s = self.builder.build_int_to_ptr(s_i, ptr_ty, "str").unwrap();
                    let f = self.module.get_function("lumia_str_split").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[s.into(), sep.into()], "split")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "split_i64")
                        .unwrap()
                        .into())
                }
                Builtin::StrSubstring => {
                    let s_i = self.coerce_i64(self.local(args[0])?)?;
                    let a = self.coerce_i64(self.local(args[1])?)?;
                    let b = self.coerce_i64(self.local(args[2])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let s = self.builder.build_int_to_ptr(s_i, ptr_ty, "str").unwrap();
                    let f = self.module.get_function("lumia_str_substring").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[s.into(), a.into(), b.into()], "substr")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "substr_i64")
                        .unwrap()
                        .into())
                }
                Builtin::ListTake => {
                    let list_i = self.coerce_i64(self.local(args[0])?)?;
                    let n = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let list = self
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "list")
                        .unwrap();
                    let f = self.module.get_function("lumia_list_take").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[list.into(), n.into()], "take")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "take_i64")
                        .unwrap()
                        .into())
                }
                Builtin::ListReverse | Builtin::ListSort => {
                    let list_i = self.coerce_i64(self.local(args[0])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let list = self
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "list")
                        .unwrap();
                    let fname = match name {
                        Builtin::ListReverse => "lumia_list_reverse",
                        _ => "lumia_list_sort",
                    };
                    let f = self.module.get_function(fname).unwrap();
                    let call = self.builder.build_call(f, &[list.into()], "list_op").unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "list_op_i64")
                        .unwrap()
                        .into())
                }
                Builtin::ListSortByKeys => {
                    let vals_i = self.coerce_i64(self.local(args[0])?)?;
                    let keys_i = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let vals = self
                        .builder
                        .build_int_to_ptr(vals_i, ptr_ty, "sby_vals")
                        .unwrap();
                    let keys = self
                        .builder
                        .build_int_to_ptr(keys_i, ptr_ty, "sby_keys")
                        .unwrap();
                    let f = self.module.get_function("lumia_list_sort_by_keys").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[vals.into(), keys.into()], "sort_by")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "sort_by_i64")
                        .unwrap()
                        .into())
                }
                Builtin::ListParMap => {
                    let list_i = self.coerce_i64(self.local(args[0])?)?;
                    let fun_i = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let list = self
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "pmap_list")
                        .unwrap();
                    // FunRef is tagged with low bit; refuse heap closures.
                    let one = self.i64_ty.const_int(1, false);
                    let tagged = self.builder.build_and(fun_i, one, "pmap_tag").unwrap();
                    let is_funref = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, tagged, one, "pmap_is_fr")
                        .unwrap();
                    let cur = self
                        .builder
                        .get_insert_block()
                        .context("par_map needs insert block")?;
                    let parent = cur.get_parent().context("bb parent")?;
                    let ok_bb = self.context.append_basic_block(parent, "pmap_ok");
                    let bad_bb = self.context.append_basic_block(parent, "pmap_bad");
                    self.builder
                        .build_conditional_branch(is_funref, ok_bb, bad_bb)
                        .unwrap();
                    self.builder.position_at_end(bad_bb);
                    let fail = self.module.get_function("lumia_match_fail").unwrap();
                    self.builder.build_call(fail, &[], "pmap_bad_fn").unwrap();
                    self.builder.build_unreachable().unwrap();
                    self.builder.position_at_end(ok_bb);
                    let cleared = self
                        .builder
                        .build_and(
                            fun_i,
                            self.builder.build_not(one, "not1").unwrap(),
                            "fun_clear",
                        )
                        .unwrap();
                    let fptr = self
                        .builder
                        .build_int_to_ptr(cleared, ptr_ty, "pmap_fn")
                        .unwrap();
                    let f = self.module.get_function("lumia_list_par_map").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[list.into(), fptr.into()], "par_map")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "par_map_i64")
                        .unwrap()
                        .into())
                }
                Builtin::ListJoin => {
                    let list_i = self.coerce_i64(self.local(args[0])?)?;
                    let sep_i = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let list = self
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "list")
                        .unwrap();
                    let sep = self
                        .builder
                        .build_int_to_ptr(sep_i, ptr_ty, "sep")
                        .unwrap();
                    let f = self.module.get_function("lumia_list_join").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[list.into(), sep.into()], "join")
                        .unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "join_i64")
                        .unwrap()
                        .into())
                }
                Builtin::ReadStdin => {
                    let f = self.module.get_function("lumia_read_stdin").unwrap();
                    let call = self.builder.build_call(f, &[], "stdin").unwrap();
                    let ptr = call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "stdin_i64")
                        .unwrap()
                        .into())
                }
                Builtin::MatchFail => {
                    let f = self.module.get_function("lumia_match_fail").unwrap();
                    self.builder.build_call(f, &[], "match_fail").unwrap();
                    // Unreachable in practice; keep SSA well-typed.
                    Ok(self.i64_ty.const_int(0, false).into())
                }
                Builtin::Assert => {
                    let cond = self.coerce_i64(self.local(args[0])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let (msg_ptr, msg_len) = if args.len() >= 2 {
                        let msg_i = self.coerce_i64(self.local(args[1])?)?;
                        let msg_ptr = self
                            .builder
                            .build_int_to_ptr(msg_i, ptr_ty, "assert_msg")
                            .unwrap();
                        let len_f = self.module.get_function("lumia_str_len").unwrap();
                        let len = self
                            .builder
                            .build_call(len_f, &[msg_ptr.into()], "assert_len")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap();
                        (msg_ptr, len)
                    } else {
                        (
                            ptr_ty.const_null(),
                            self.i64_ty.const_int(0, false).into(),
                        )
                    };
                    let f = self.module.get_function("lumia_assert").unwrap();
                    self.builder
                        .build_call(
                            f,
                            &[cond.into(), msg_ptr.into(), msg_len.into()],
                            "assert",
                        )
                        .unwrap();
                    Ok(self.i64_ty.const_int(0, false).into())
                }
                Builtin::StrStartsWith | Builtin::StrEndsWith => {
                    let a_i = self.coerce_i64(self.local(args[0])?)?;
                    let b_i = self.coerce_i64(self.local(args[1])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let a = self.builder.build_int_to_ptr(a_i, ptr_ty, "a").unwrap();
                    let b = self.builder.build_int_to_ptr(b_i, ptr_ty, "b").unwrap();
                    let fname = match name {
                        Builtin::StrStartsWith => "lumia_str_starts_with",
                        _ => "lumia_str_ends_with",
                    };
                    let f = self.module.get_function(fname).unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[a.into(), b.into()], "str_affix")
                        .unwrap();
                    Ok(call.try_as_basic_value().basic().unwrap())
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
                    .emit_scoped_block(then_block, fv)?
                    .unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
                let then_terminated = self
                    .builder
                    .get_insert_block()
                    .and_then(|bb| bb.get_terminator())
                    .is_some();
                let mut then_incoming_i = None;
                let mut then_incoming_f = None;
                if !then_terminated {
                    let then_bb_end = self.builder.get_insert_block().unwrap();
                    then_incoming_i = Some((self.coerce_i64(then_raw)?, then_bb_end));
                    then_incoming_f = Some((self.promote_f64(then_raw)?, then_bb_end));
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                }
                let then_is_float = matches!(then_raw, BasicValueEnum::FloatValue(_));

                self.builder.position_at_end(else_bb);
                let else_raw = self
                    .emit_scoped_block(else_block, fv)?
                    .unwrap_or_else(|| self.i64_ty.const_int(0, false).into());
                let else_terminated = self
                    .builder
                    .get_insert_block()
                    .and_then(|bb| bb.get_terminator())
                    .is_some();
                let mut else_incoming_i = None;
                let mut else_incoming_f = None;
                if !else_terminated {
                    let else_bb_end = self.builder.get_insert_block().unwrap();
                    else_incoming_i = Some((self.coerce_i64(else_raw)?, else_bb_end));
                    else_incoming_f = Some((self.promote_f64(else_raw)?, else_bb_end));
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                }
                let float_merge =
                    then_is_float || matches!(else_raw, BasicValueEnum::FloatValue(_));

                self.builder.position_at_end(merge_bb);
                if float_merge {
                    match (then_incoming_f, else_incoming_f) {
                        (Some((tv, tb)), Some((ev, eb))) => {
                            let phi = self
                                .builder
                                .build_phi(self.context.f64_type(), "iftmpf")
                                .unwrap();
                            phi.add_incoming(&[(&tv, tb), (&ev, eb)]);
                            Ok(phi.as_basic_value())
                        }
                        (Some((tv, _)), None) | (None, Some((tv, _))) => Ok(tv.into()),
                        (None, None) => Ok(self.context.f64_type().const_float(0.0).into()),
                    }
                } else {
                    match (then_incoming_i, else_incoming_i) {
                        (Some((tv, tb)), Some((ev, eb))) => {
                            let phi = self.builder.build_phi(self.i64_ty, "iftmp").unwrap();
                            phi.add_incoming(&[(&tv, tb), (&ev, eb)]);
                            Ok(phi.as_basic_value())
                        }
                        (Some((tv, _)), None) | (None, Some((tv, _))) => Ok(tv.into()),
                        (None, None) => Ok(self.i64_ty.const_int(0, false).into()),
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

                // continue → latch (runs step); break → exit; both restore loop roots
                let loop_depth = self.root_depth;
                self.loop_stack.push((latch_bb, exit_bb, loop_depth));

                self.builder.position_at_end(header_bb);
                let cond_raw = self
                    .emit_scoped_block(header, fv)?
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
                let _ = self.emit_scoped_block(body, fv)?;
                if self
                    .builder
                    .get_insert_block()
                    .and_then(|bb| bb.get_terminator())
                    .is_none()
                {
                    self.builder.build_unconditional_branch(latch_bb).unwrap();
                }

                self.builder.position_at_end(latch_bb);
                let _ = self.emit_scoped_block(latch, fv)?;
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

fn c_abi_fn_type<'ctx>(
    context: &'ctx Context,
    params: &[Type],
    ret: &Type,
) -> inkwell::types::FunctionType<'ctx> {
    let i64_ty = context.i64_type();
    let f64_ty = context.f64_type();
    let i8_ty = context.i8_type();
    let ptr_ty = context.ptr_type(AddressSpace::default());
    let meta: Vec<BasicMetadataTypeEnum> = params
        .iter()
        .map(|t| match t {
            Type::Float => f64_ty.into(),
            Type::Bool => i8_ty.into(),
            Type::String => ptr_ty.into(),
            _ => i64_ty.into(),
        })
        .collect();
    match ret {
        Type::Unit => context.void_type().fn_type(&meta, false),
        Type::Float => f64_ty.fn_type(&meta, false),
        Type::Bool => i8_ty.fn_type(&meta, false),
        Type::String => ptr_ty.fn_type(&meta, false),
        _ => i64_ty.fn_type(&meta, false),
    }
}

fn link_executable(
    obj: &Path,
    runtime: &Path,
    output: &Path,
    extra: &[String],
) -> Result<()> {
    let mut cmd = Command::new("clang");
    cmd.arg(obj).arg(runtime).arg("-o").arg(output);
    // Unix shared libs; skip on Windows MSVC/clang-cl style targets.
    if !cfg!(target_os = "windows") {
        cmd.arg("-lpthread").arg("-ldl").arg("-lm");
    }
    for a in extra {
        cmd.arg(a);
    }
    let status = cmd.status().context("invoke clang linker")?;
    if !status.success() {
        bail!("link failed with {status}");
    }
    Ok(())
}

/// Locate `liblumia_rt.a` / `lumia_rt.lib` in target dir.
pub fn find_runtime_lib(target_dir: &Path) -> Result<PathBuf> {
    find_runtime_lib_prefer(target_dir, false)
}

pub fn find_runtime_lib_prefer(target_dir: &Path, release: bool) -> Result<PathBuf> {
    let profiles = if release {
        ["release", "debug"]
    } else {
        ["debug", "release"]
    };
    let mut candidates = Vec::new();
    for p in profiles {
        candidates.push(target_dir.join(p).join("liblumia_rt.a"));
        candidates.push(target_dir.join(p).join("lumia_rt.lib"));
        candidates.push(target_dir.join(p).join("lumia_rt.dll.lib"));
    }
    candidates.push(target_dir.join("liblumia_rt.a"));
    candidates.push(target_dir.join("lumia_rt.lib"));
    for c in candidates {
        if c.exists() {
            return Ok(c);
        }
    }
    bail!(
        "liblumia_rt.a / lumia_rt.lib not found under {} — run `cargo build -p lumia_rt` first",
        target_dir.display()
    )
}
