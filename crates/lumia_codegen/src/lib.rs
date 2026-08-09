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
    cg.tco_sccs = compute_tco_sccs(core);
    cg.hash_adts = core.hash_adts.clone();

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
    // PIC is fine on ELF/Mach-O; MSVC COFF expects the default reloc model.
    let reloc = if cfg!(target_os = "windows") {
        RelocMode::Default
    } else {
        RelocMode::PIC
    };
    let tm = target
        .create_target_machine(
            &triple,
            &cpu,
            &features,
            opt,
            reloc,
            CodeModel::Default,
        )
        .context("create target machine")?;

    let obj_path = if cfg!(target_os = "windows") {
        opts.output.with_extension("obj")
    } else {
        opts.output.with_extension("o")
    };
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
        "lumia_cmp",
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
    module.add_function(
        "lumia_show_bool",
        ptr_ty.fn_type(&[i8_ty.into()], false),
        None,
    );
    module.add_function("lumia_gc_collect", void_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_root_push",
        void_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function("lumia_root_pop", void_ty.fn_type(&[], false), None);
    // `lumia_write_barrier` stays in `lumia_rt` ABI for future concurrent GC;
    // STW mark-sweep does not emit calls.
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
    module.add_function("lumia_list_empty", ptr_ty.fn_type(&[], false), None);
    module.add_function(
        "lumia_map_finish",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_set_finish",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
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
        "lumia_ensure_map_f64",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_ensure_map_vf64",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_ensure_set_f64",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
        None,
    );
    module.add_function(
        "lumia_ensure_list_f64",
        ptr_ty.fn_type(&[ptr_ty.into()], false),
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
        "lumia_elems",
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
        "lumia_list_par_fold",
        i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false),
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
    /// Best-effort SSA local types (for typed println/show dispatch).
    local_tys: HashMap<u32, Type>,
    /// Mutable slot types.
    slot_tys: HashMap<String, Type>,
    /// Shadow-stack pushes currently live in this function (LIFO).
    root_depth: u32,
    /// Mutable slots that have already been registered as GC roots.
    rooted_slots: HashSet<String>,
    /// Function entry block — all GC root allocas go here (avoid loop stack growth).
    entry_bb: Option<BasicBlock<'ctx>>,
    /// Current Core function name (for TCO).
    current_fun: String,
    /// Pure Int mutual/self-recursion peers in the same SCC (musttail when `root_depth == 0`).
    tco_peers: HashSet<String>,
    /// Precomputed TCO SCCs: function → peers (including self).
    tco_sccs: HashMap<String, HashSet<String>>,
    /// ADT names with `instance Hash` (may promote Map/Set to hash tables).
    hash_adts: HashSet<String>,
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
            local_tys: HashMap::new(),
            slot_tys: HashMap::new(),
            root_depth: 0,
            rooted_slots: HashSet::new(),
            entry_bb: None,
            current_fun: String::new(),
            tco_peers: HashSet::new(),
            tco_sccs: HashMap::new(),
            hash_adts: HashSet::new(),
        }
    }

    fn key_type_has_hash(&self, ty: &Type) -> bool {
        match ty {
            Type::Adt { name, .. } => self.hash_adts.contains(name),
            // Scalars / collections: structural hash always available.
            Type::Int
            | Type::Float
            | Type::Bool
            | Type::String
            | Type::Char
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Set(_) => true,
            _ => false,
        }
    }

    /// Call `__Show_{T}_show` when an instance provided a custom Show method.
    fn emit_show_override(
        &mut self,
        adt_name: &str,
        arg: BasicValueEnum<'ctx>,
    ) -> Result<Option<PointerValue<'ctx>>> {
        let mangled = format!("__Show_{adt_name}_show");
        let Some(fv) = self.functions.get(&mangled).copied() else {
            return Ok(None);
        };
        let i = self.coerce_i64(arg)?;
        let call = self
            .builder
            .build_call(fv, &[i.into()], "show_ov")
            .unwrap();
        let bits = call
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_int_value();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let ptr = self
            .builder
            .build_int_to_ptr(bits, ptr_ty, "show_ov_ptr")
            .unwrap();
        Ok(Some(ptr))
    }

    /// Call `__Eq_{T}_eq(a,b) -> Bool` when present.
    fn emit_eq_override(
        &mut self,
        adt_name: &str,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> Result<Option<IntValue<'ctx>>> {
        let mangled = format!("__Eq_{adt_name}_eq");
        let Some(fv) = self.functions.get(&mangled).copied() else {
            return Ok(None);
        };
        let call = self
            .builder
            .build_call(fv, &[left.into(), right.into()], "eq_ov")
            .unwrap();
        Ok(Some(
            call.try_as_basic_value()
                .basic()
                .unwrap()
                .into_int_value(),
        ))
    }

    /// Call `__Ord_{T}_less(a,b) -> Bool` when present.
    fn emit_less_override(
        &mut self,
        adt_name: &str,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> Result<Option<IntValue<'ctx>>> {
        let mangled = format!("__Ord_{adt_name}_less");
        let Some(fv) = self.functions.get(&mangled).copied() else {
            return Ok(None);
        };
        let call = self
            .builder
            .build_call(fv, &[left.into(), right.into()], "less_ov")
            .unwrap();
        Ok(Some(
            call.try_as_basic_value()
                .basic()
                .unwrap()
                .into_int_value(),
        ))
    }

    fn adt_method_name(left: &Type, right: &Type) -> Option<String> {
        match (left, right) {
            (Type::Adt { name: a, .. }, Type::Adt { name: b, .. }) if a == b => Some(a.clone()),
            _ => None,
        }
    }

    /// Typed `==` for ADTs with Float fields and fallback to `lumia_eq`.
    fn emit_value_eq(
        &mut self,
        lt: &Type,
        rt: &Type,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>> {
        if let Some(name) = Self::adt_method_name(lt, rt) {
            if let Some(eq) = self.emit_eq_override(&name, l, r)? {
                return Ok(eq);
            }
            if let (Type::Adt { params: lp, .. }, Type::Adt { params: rp, .. }) = (lt, rt) {
                if lp.iter().any(|p| matches!(p, Type::Float))
                    || rp.iter().any(|p| matches!(p, Type::Float))
                {
                    let params = if lp.len() >= rp.len() { lp } else { rp };
                    return self.emit_typed_adt_eq(l, r, params);
                }
            }
        }
        let f = self.module.get_function("lumia_eq").unwrap();
        Ok(self
            .builder
            .build_call(f, &[l.into(), r.into()], "eq")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_int_value())
    }

    /// Structural ADT `==` using `Type::Adt` field params (Float → IEEE OEQ).
    fn emit_typed_adt_eq(
        &mut self,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
        params: &[Type],
    ) -> Result<IntValue<'ctx>> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let la = self
            .builder
            .build_int_to_ptr(left, ptr_ty, "adt_eq_l")
            .unwrap();
        let ra = self
            .builder
            .build_int_to_ptr(right, ptr_ty, "adt_eq_r")
            .unwrap();
        let load_i64 = |cg: &Self, base: PointerValue<'ctx>, idx: u64, name: &str| {
            let slot = unsafe {
                cg.builder
                    .build_gep(
                        cg.i64_ty,
                        base,
                        &[cg.i64_ty.const_int(idx, false)],
                        name,
                    )
                    .unwrap()
            };
            cg.builder
                .build_load(cg.i64_ty, slot, &format!("{name}v"))
                .unwrap()
                .into_int_value()
        };
        let ltag = load_i64(self, la, 0, "ltag");
        let rtag = load_i64(self, ra, 0, "rtag");
        let tag_eq = self
            .builder
            .build_int_compare(IntPredicate::EQ, ltag, rtag, "tag_eq")
            .unwrap();
        let mut acc = self
            .builder
            .build_int_z_extend(tag_eq, self.i64_ty, "tag_eqz")
            .unwrap();
        let zero = self.i64_ty.const_int(0, false);
        for (fi, pty) in params.iter().enumerate() {
            let lb = load_i64(self, la, (fi + 1) as u64, &format!("lf{fi}"));
            let rb = load_i64(self, ra, (fi + 1) as u64, &format!("rf{fi}"));
            let field_eq = match pty {
                Type::Float => {
                    let lf = self
                        .builder
                        .build_bit_cast(lb, self.context.f64_type(), "lf_f")
                        .unwrap()
                        .into_float_value();
                    let rf = self
                        .builder
                        .build_bit_cast(rb, self.context.f64_type(), "rf_f")
                        .unwrap()
                        .into_float_value();
                    let c = self
                        .builder
                        .build_float_compare(FloatPredicate::OEQ, lf, rf, "fld_fcmp")
                        .unwrap();
                    self.builder
                        .build_int_z_extend(c, self.i64_ty, "fld_feqz")
                        .unwrap()
                }
                Type::Bool | Type::Int | Type::Char | Type::Unit => {
                    let c = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, lb, rb, "fld_icmp")
                        .unwrap();
                    self.builder
                        .build_int_z_extend(c, self.i64_ty, "fld_iqz")
                        .unwrap()
                }
                _ => {
                    let f = self.module.get_function("lumia_eq").unwrap();
                    self.builder
                        .build_call(f, &[lb.into(), rb.into()], "fld_eq")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_int_value()
                }
            };
            // acc = acc != 0 && field_eq != 0
            let a_ok = self
                .builder
                .build_int_compare(IntPredicate::NE, acc, zero, "a_ok")
                .unwrap();
            let f_ok = self
                .builder
                .build_int_compare(IntPredicate::NE, field_eq, zero, "f_ok")
                .unwrap();
            let both = self.builder.build_and(a_ok, f_ok, "both").unwrap();
            acc = self
                .builder
                .build_int_z_extend(both, self.i64_ty, "acc_eq")
                .unwrap();
        }
        Ok(acc)
    }

    /// Structural ADT show using `Type::Adt` field params (Float/Bool typed).
    fn emit_typed_adt_show(
        &mut self,
        arg: BasicValueEnum<'ctx>,
        params: &[Type],
    ) -> Result<PointerValue<'ctx>> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let i = self.coerce_i64(arg)?;
        let base = self
            .builder
            .build_int_to_ptr(i, ptr_ty, "adt_show_base")
            .unwrap();
        let tag_slot = unsafe {
            self.builder
                .build_gep(
                    self.i64_ty,
                    base,
                    &[self.i64_ty.const_int(0, false)],
                    "tag",
                )
                .unwrap()
        };
        let tag = self
            .builder
            .build_load(self.i64_ty, tag_slot, "tagv")
            .unwrap()
            .into_int_value();
        let show_i = self.module.get_function("lumia_show").unwrap();
        let show_f = self.module.get_function("lumia_show_float").unwrap();
        let show_b = self.module.get_function("lumia_show_bool").unwrap();
        let concat = self.module.get_function("lumia_concat").unwrap();
        let alloc = self.module.get_function("lumia_alloc_string").unwrap();

        let mk_lit = |cg: &Self, s: &str, name: &str| {
            let gv = cg.builder.build_global_string_ptr(s, name).unwrap();
            cg.builder
                .build_call(
                    alloc,
                    &[
                        gv.as_pointer_value().into(),
                        cg.i64_ty.const_int(s.len() as u64, false).into(),
                    ],
                    &format!("lit_{name}"),
                )
                .unwrap()
                .try_as_basic_value()
                .basic()
                .unwrap()
                .into_pointer_value()
        };

        let mut acc = mk_lit(self, "#", "hash");
        let tag_s = self
            .builder
            .build_call(show_i, &[tag.into()], "show_tag")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        acc = self
            .builder
            .build_call(concat, &[acc.into(), tag_s.into()], "cat_tag")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        let lpar_s = mk_lit(self, "(", "lpar");
        acc = self
            .builder
            .build_call(concat, &[acc.into(), lpar_s.into()], "cat_lpar")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        let comma_s = mk_lit(self, ", ", "comma");

        for (fi, pty) in params.iter().enumerate() {
            if fi > 0 {
                acc = self
                    .builder
                    .build_call(concat, &[acc.into(), comma_s.into()], "cat_comma")
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
            }
            let slot = unsafe {
                self.builder
                    .build_gep(
                        self.i64_ty,
                        base,
                        &[self.i64_ty.const_int((fi + 1) as u64, false)],
                        "fld",
                    )
                    .unwrap()
            };
            let bits = self
                .builder
                .build_load(self.i64_ty, slot, "fldv")
                .unwrap()
                .into_int_value();
            let field_s = match pty {
                Type::Float => {
                    let f = self
                        .builder
                        .build_bit_cast(bits, self.context.f64_type(), "fld_f")
                        .unwrap()
                        .into_float_value();
                    self.builder
                        .build_call(show_f, &[f.into()], "show_fld_f")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value()
                }
                Type::Bool => {
                    let b = self
                        .builder
                        .build_int_truncate(bits, self.context.i8_type(), "fld_b")
                        .unwrap();
                    self.builder
                        .build_call(show_b, &[b.into()], "show_fld_b")
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value()
                }
                _ => self
                    .builder
                    .build_call(show_i, &[bits.into()], "show_fld")
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value(),
            };
            acc = self
                .builder
                .build_call(concat, &[acc.into(), field_s.into()], "cat_fld")
                .unwrap()
                .try_as_basic_value()
                .basic()
                .unwrap()
                .into_pointer_value();
        }

        let rpar_s = mk_lit(self, ")", "rpar");
        acc = self
            .builder
            .build_call(concat, &[acc.into(), rpar_s.into()], "cat_rpar")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        Ok(acc)
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
            // Small non-escaping Lit* lives on the stack — not a GC root.
            Value::AllocList { elems, repr } => {
                !(matches!(repr, lumia_core::ListRepr::LitList) && !elems.is_empty())
            }
            Value::AllocSet { elems, repr } => {
                !(matches!(repr, lumia_core::SetRepr::LitSet) && !elems.is_empty())
            }
            Value::AllocMap { flat_pairs, repr } => {
                !(matches!(repr, lumia_core::MapRepr::LitMap) && !flat_pairs.is_empty())
            }
            Value::AllocAdt { repr, .. } => {
                !matches!(repr, lumia_core::AdtRepr::LitAdt)
            }
            Value::AllocClosure { .. } => true,
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
                    | Builtin::Elems
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

    /// Emit `musttail call` + `ret` for pure Int TCO (self or mutual; no GC roots live).
    /// Returns true if the call was emitted as a terminator.
    fn emit_musttail_call(&mut self, fun: &str, args: &[Local]) -> Result<bool> {
        let callee = match self.functions.get(fun).copied() {
            Some(f) => f,
            None => return Ok(false),
        };
        let mut av: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
        for a in args {
            av.push(self.coerce_i64(self.local(*a)?)?.into());
        }
        let call = self.builder.build_call(callee, &av, "tco").unwrap();
        call.set_tail_call_kind(inkwell::values::LLVMTailCallKind::LLVMTailCallKindMustTail);
        let ret = call
            .try_as_basic_value()
            .basic()
            .unwrap_or_else(|| self.i64_ty.const_int(0, false).into())
            .into_int_value();
        // No root epilogue: musttail requires call immediately followed by ret.
        debug_assert_eq!(self.root_depth, 0);
        self.builder.build_return(Some(&ret)).unwrap();
        Ok(true)
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
        self.local_tys.clear();
        self.slot_tys.clear();
        self.current_fun = fun.name.clone();
        self.tco_peers = self
            .tco_sccs
            .get(&fun.name)
            .cloned()
            .unwrap_or_default();

        for (i, p) in fun.params.iter().enumerate() {
            let av = fv.get_nth_param(i as u32).unwrap();
            let ty = fun.param_tys.get(i).cloned().unwrap_or(Type::Int);
            self.local_tys.insert(p.0, ty.clone());
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
        // Tail-call / break paths may already have terminated the block.
        if self
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some()
        {
            return Ok(());
        }
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
            self.slot_tys.insert(name.to_string(), Type::Float);
        }
        let slot = self.ensure_slot(name);
        let i = self.coerce_i64(v)?;
        self.builder.build_store(slot, i).unwrap();
        Ok(())
    }

    /// Preserve `List[T]` element type from the first list-typed argument.
    fn list_elem_preserved(&self, args: &[Local]) -> Type {
        if let Some(arg0) = args.first() {
            if let Some(Type::List(elem)) = self.local_tys.get(&arg0.0) {
                return Type::List(elem.clone());
            }
        }
        Type::List(Box::new(Type::Int))
    }

    fn infer_value_ty(&self, value: &Value) -> Type {
        match value {
            Value::Bool(_) => Type::Bool,
            Value::Int(_) => Type::Int,
            Value::Float(_) => Type::Float,
            Value::String(_) => Type::String,
            Value::Char(_) => Type::Char,
            Value::Unit => Type::Unit,
            Value::Local(Local(id)) => self
                .local_tys
                .get(id)
                .cloned()
                .unwrap_or(Type::Int),
            Value::Name(n) => self.slot_tys.get(n).cloned().unwrap_or(Type::Int),
            Value::Binary { op, left, right } => match op {
                lumia_syntax::BinOp::Eq
                | lumia_syntax::BinOp::Ne
                | lumia_syntax::BinOp::Lt
                | lumia_syntax::BinOp::Le
                | lumia_syntax::BinOp::Gt
                | lumia_syntax::BinOp::Ge
                | lumia_syntax::BinOp::And
                | lumia_syntax::BinOp::Or => Type::Bool,
                _ => {
                    let lt = self.local_tys.get(&left.0).cloned().unwrap_or(Type::Int);
                    let rt = self.local_tys.get(&right.0).cloned().unwrap_or(Type::Int);
                    if matches!(lt, Type::Float) || matches!(rt, Type::Float) {
                        Type::Float
                    } else {
                        Type::Int
                    }
                }
            },
            Value::Unary {
                op: lumia_syntax::UnOp::Not,
                ..
            } => Type::Bool,
            Value::Unary { operand, .. } => self
                .local_tys
                .get(&operand.0)
                .cloned()
                .unwrap_or(Type::Int),
            Value::Call { fun, args } => {
                let ret = self
                    .fun_ret_tys
                    .get(fun)
                    .cloned()
                    .unwrap_or(Type::Int);
                // Let-poly identity lambdas keep a conservative heap `ret_ty`
                // (`List[Int]`) and `Int` formals so String/ADT still root. Before
                // monomorphization, a Float argument that is returned as bits must
                // still type the Call result as Float (println / arithmetic).
                if matches!(&ret, Type::List(e) if matches!(e.as_ref(), Type::Int)) {
                    let ptys = self.fun_param_tys.get(fun).cloned().unwrap_or_default();
                    if args.len() == 1
                        && ptys.len() == 1
                        && matches!(ptys[0], Type::Int)
                        && matches!(self.local_tys.get(&args[0].0), Some(Type::Float))
                    {
                        return Type::Float;
                    }
                }
                ret
            }
            Value::Builtin { name, args } => match name {
                Builtin::StrStartsWith
                | Builtin::StrEndsWith
                | Builtin::Contains => Type::Bool,
                Builtin::ListLen | Builtin::AdtTag => Type::Int,
                Builtin::Show
                | Builtin::ReadStdin
                | Builtin::StrTrim
                | Builtin::StrSplit
                | Builtin::StrSubstring
                | Builtin::StrToLower
                | Builtin::StrToUpper
                | Builtin::ListJoin => Type::String,
                Builtin::Println
                | Builtin::PrintlnInt
                | Builtin::PrintlnStr
                | Builtin::MatchFail
                | Builtin::Assert => Type::Unit,
                Builtin::ListGet => {
                    // Element type when known; Map get → Option[V] (runtime ADT).
                    if let Some(arg0) = args.first() {
                        match self.local_tys.get(&arg0.0) {
                            Some(Type::List(elem)) => (**elem).clone(),
                            Some(Type::Set(elem)) => (**elem).clone(),
                            Some(Type::Map(_, v)) => Type::Adt {
                                name: "Option".into(),
                                params: vec![(**v).clone()],
                            },
                            _ => Type::Int,
                        }
                    } else {
                        Type::Int
                    }
                }
                // ADT/Option/Result field 0 when known (Ok/Some payload = params[0]);
                // Result[T,E] has two type params so must not require `len == 1`.
                Builtin::AdtField => {
                    if let Some(arg0) = args.first() {
                        match self.local_tys.get(&arg0.0) {
                            Some(Type::Adt { params, .. }) if !params.is_empty() => {
                                params[0].clone()
                            }
                            Some(Type::Tuple(ts)) if !ts.is_empty() => {
                                // Index is a separate local; default to first field.
                                ts[0].clone()
                            }
                            _ => Type::Int,
                        }
                    } else {
                        Type::Int
                    }
                }
                Builtin::ListSlice
                | Builtin::ListTake
                | Builtin::ListReverse
                | Builtin::ListSort
                | Builtin::ListSortByKeys
                | Builtin::ListParMap => self.list_elem_preserved(args),
                Builtin::ListParFold => {
                    // Acc / init type (scalar).
                    args.get(1)
                        .and_then(|a| self.local_tys.get(&a.0).cloned())
                        .unwrap_or(Type::Int)
                }
                Builtin::ListAppend => {
                    if let Some(arg0) = args.first() {
                        match self.local_tys.get(&arg0.0) {
                            Some(Type::List(elem)) => Type::List(elem.clone()),
                            _ => Type::List(Box::new(Type::Int)),
                        }
                    } else {
                        Type::List(Box::new(Type::Int))
                    }
                }
                Builtin::ListConcat => {
                    if let Some(arg0) = args.first() {
                        match self.local_tys.get(&arg0.0) {
                            Some(Type::List(elem)) => Type::List(elem.clone()),
                            _ => Type::List(Box::new(Type::Int)),
                        }
                    } else {
                        Type::List(Box::new(Type::Int))
                    }
                }
                Builtin::Elems => {
                    if let Some(arg0) = args.first() {
                        match self.local_tys.get(&arg0.0) {
                            Some(Type::List(e)) => Type::List(e.clone()),
                            Some(Type::Set(e)) => Type::List(e.clone()),
                            Some(Type::Map(k, _)) => Type::List(k.clone()),
                            _ => Type::List(Box::new(Type::Int)),
                        }
                    } else {
                        Type::List(Box::new(Type::Int))
                    }
                }
                Builtin::MapKeys => {
                    if let Some(arg0) = args.first() {
                        match self.local_tys.get(&arg0.0) {
                            Some(Type::Map(k, _)) => Type::List(k.clone()),
                            _ => Type::List(Box::new(Type::Int)),
                        }
                    } else {
                        Type::List(Box::new(Type::Int))
                    }
                }
                Builtin::MapValues => {
                    if let Some(arg0) = args.first() {
                        match self.local_tys.get(&arg0.0) {
                            Some(Type::Map(_, v)) => Type::List(v.clone()),
                            _ => Type::List(Box::new(Type::Int)),
                        }
                    } else {
                        Type::List(Box::new(Type::Int))
                    }
                }
                Builtin::Range | Builtin::RangeInclusive => Type::List(Box::new(Type::Int)),
                Builtin::MapItems => {
                    if let Some(arg0) = args.first() {
                        match self.local_tys.get(&arg0.0) {
                            Some(Type::Map(k, v)) => Type::List(Box::new(Type::Adt {
                                name: "__Tuple".into(),
                                params: vec![(**k).clone(), (**v).clone()],
                            })),
                            Some(Type::List(elem)) => Type::List(elem.clone()),
                            _ => Type::List(Box::new(Type::Adt {
                                name: "__Tuple".into(),
                                params: vec![Type::Int, Type::Int],
                            })),
                        }
                    } else {
                        Type::List(Box::new(Type::Adt {
                            name: "__Tuple".into(),
                            params: vec![Type::Int, Type::Int],
                        }))
                    }
                }
                Builtin::MapSet | Builtin::MapRemove => {
                    let key_ty = args
                        .get(1)
                        .and_then(|a| self.local_tys.get(&a.0).cloned())
                        .unwrap_or(Type::Int);
                    if let Some(arg0) = args.first() {
                        match self.local_tys.get(&arg0.0) {
                            Some(Type::Map(k, v)) => {
                                let k = if matches!(key_ty, Type::Float) {
                                    Box::new(Type::Float)
                                } else {
                                    k.clone()
                                };
                                Type::Map(k, v.clone())
                            }
                            _ => Type::Map(Box::new(key_ty), Box::new(Type::Int)),
                        }
                    } else {
                        Type::Map(Box::new(Type::Int), Box::new(Type::Int))
                    }
                }
                Builtin::SetInsert => {
                    let elem_ty = args
                        .get(1)
                        .and_then(|a| self.local_tys.get(&a.0).cloned())
                        .unwrap_or(Type::Int);
                    if let Some(arg0) = args.first() {
                        match self.local_tys.get(&arg0.0) {
                            Some(Type::Set(e)) => {
                                if matches!(elem_ty, Type::Float) {
                                    Type::Set(Box::new(Type::Float))
                                } else {
                                    Type::Set(e.clone())
                                }
                            }
                            _ => Type::Set(Box::new(elem_ty)),
                        }
                    } else {
                        Type::Set(Box::new(Type::Int))
                    }
                }
            },
            Value::AllocList { elems, .. } => {
                let elem = elems
                    .first()
                    .and_then(|Local(id)| self.local_tys.get(id).cloned())
                    .unwrap_or(Type::Int);
                Type::List(Box::new(elem))
            }
            Value::AllocSet { elems, .. } => {
                let elem = elems
                    .first()
                    .and_then(|Local(id)| self.local_tys.get(id).cloned())
                    .unwrap_or(Type::Int);
                Type::Set(Box::new(elem))
            }
            Value::AllocMap { flat_pairs, .. } => {
                let (k, v) = if flat_pairs.len() >= 2 {
                    (
                        self.local_tys
                            .get(&flat_pairs[0].0)
                            .cloned()
                            .unwrap_or(Type::Int),
                        self.local_tys
                            .get(&flat_pairs[1].0)
                            .cloned()
                            .unwrap_or(Type::Int),
                    )
                } else {
                    (Type::Int, Type::Int)
                };
                Type::Map(Box::new(k), Box::new(v))
            }
            Value::AllocAdt {
                adt_name,
                fields,
                ..
            } => {
                let params: Vec<Type> = fields
                    .iter()
                    .map(|Local(id)| self.local_tys.get(id).cloned().unwrap_or(Type::Int))
                    .collect();
                Type::Adt {
                    name: adt_name.clone(),
                    params,
                }
            }
            Value::AllocClosure { .. } | Value::FunRef(_) | Value::ClosureCap { .. } => {
                Type::Fun(vec![], Box::new(Type::Int), lumia_ty::Effect::pure())
            }
            Value::If {
                then_block,
                else_block,
                ..
            } => {
                let t = then_block
                    .result
                    .and_then(|Local(id)| self.local_tys.get(&id).cloned());
                let e = else_block
                    .result
                    .and_then(|Local(id)| self.local_tys.get(&id).cloned());
                t.or(e).unwrap_or(Type::Int)
            }
            Value::Loop { .. } | Value::Lambda { .. } => Type::Int,
            Value::IndirectCall { callee, args } => {
                let ret = match self.local_tys.get(&callee.0) {
                    Some(Type::Fun(_, ret, _)) => (**ret).clone(),
                    _ => self
                        .funref_locals
                        .get(&callee.0)
                        .and_then(|name| self.fun_ret_tys.get(name).cloned())
                        .unwrap_or(Type::Int),
                };
                if matches!(&ret, Type::List(e) if matches!(e.as_ref(), Type::Int)) {
                    if let Some(name) = self.funref_locals.get(&callee.0) {
                        let ptys = self.fun_param_tys.get(name).cloned().unwrap_or_default();
                        if args.len() == 1
                            && ptys.len() == 1
                            && matches!(ptys[0], Type::Int)
                            && matches!(self.local_tys.get(&args[0].0), Some(Type::Float))
                        {
                            return Type::Float;
                        }
                    }
                }
                ret
            }
        }
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
                    // Pure self/mutual recursion in tail position → musttail (DESIGN §4.4).
                    // Pop shadow-stack roots first so heap-param frames can musttail.
                    let is_block_tail = block.result == Some(*local)
                        && matches!(
                            block.ops.last(),
                            Some(Op::Let { local: last, .. }) if last == local
                        );
                    if !self.tco_peers.is_empty() && is_block_tail {
                        match value {
                            Value::Call { fun, args } => {
                                if self.tco_peers.contains(fun) {
                                    self.root_pop_to(0);
                                    if self.emit_musttail_call(fun, args)? {
                                        return Ok(None);
                                    }
                                }
                            }
                            Value::IndirectCall { callee, args } => {
                                if let Some(fun) = self.funref_locals.get(&callee.0).cloned() {
                                    if self.tco_peers.contains(&fun) {
                                        self.root_pop_to(0);
                                        if self.emit_musttail_call(&fun, args)? {
                                            return Ok(None);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    let v = self.emit_value(value, fv)?;
                    if self.value_may_heap(value) {
                        if let Ok(bits) = self.coerce_i64(v) {
                            self.root_push_i64(bits)?;
                        }
                    }
                    self.locals.insert(local.0, v);
                    self.local_tys
                        .insert(local.0, self.infer_value_ty(value));
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
                    if let Some(ty) = self.local_tys.get(&value.0).cloned() {
                        if !matches!(ty, Type::Float) {
                            self.slot_tys.insert(name.clone(), ty);
                        }
                    }
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

    /// Convert an operand for float arithmetic: numeric Int → sitofp; Float bits → bitcast.
    fn arith_as_f64(
        &self,
        v: BasicValueEnum<'ctx>,
        ty: &Type,
    ) -> Result<inkwell::values::FloatValue<'ctx>> {
        let fty = self.context.f64_type();
        match v {
            BasicValueEnum::FloatValue(f) => Ok(f),
            BasicValueEnum::IntValue(i) if matches!(ty, Type::Float) => Ok(self
                .builder
                .build_bit_cast(i, fty, "fbits_arith")
                .unwrap()
                .into_float_value()),
            BasicValueEnum::IntValue(i) => Ok(self
                .builder
                .build_signed_int_to_float(i, fty, "sitofp")
                .unwrap()),
            _ => bail!("expected numeric for float arith"),
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
                // Heap loads (`ListGet`, fields) keep Float as i64 bits; consult
                // `local_tys`, not only LLVM FloatValue, or we do Int ops on IEEE bits.
                let lt = self.local_tys.get(&left.0).cloned().unwrap_or(Type::Int);
                let rt = self.local_tys.get(&right.0).cloned().unwrap_or(Type::Int);
                let either_float = matches!(lt, Type::Float)
                    || matches!(rt, Type::Float)
                    || matches!(lv, BasicValueEnum::FloatValue(_))
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
                    // Float-typed locals are IEEE bits in i64; Int locals are numeric
                    // (sitofp) so `{ x -> x + 1 }` works after Float monomorphization.
                    let l = self.arith_as_f64(lv, &lt)?;
                    let r = self.arith_as_f64(rv, &rt)?;
                    let v = match op {
                        BinOp::Add => self.builder.build_float_add(l, r, "fadd").unwrap(),
                        BinOp::Sub => self.builder.build_float_sub(l, r, "fsub").unwrap(),
                        BinOp::Mul => self.builder.build_float_mul(l, r, "fmul").unwrap(),
                        BinOp::Div => self.builder.build_float_div(l, r, "fdiv").unwrap(),
                        BinOp::Rem => self.builder.build_float_rem(l, r, "frem").unwrap(),
                        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                            let pred = match op {
                                BinOp::Eq => FloatPredicate::OEQ,
                                // UNE: NaN != x is true (IEEE unordered-or-ne).
                                BinOp::Ne => FloatPredicate::UNE,
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
                // `instance Num for T`: `__Num_T_add` / `__Num_T_mul`.
                if matches!(op, BinOp::Add | BinOp::Mul) {
                    if let Some(name) = Self::adt_method_name(&lt, &rt) {
                        let method = if matches!(op, BinOp::Add) {
                            "add"
                        } else {
                            "mul"
                        };
                        let mangled = format!("__Num_{name}_{method}");
                        if let Some(callee) = self.functions.get(&mangled).copied() {
                            let call = self
                                .builder
                                .build_call(callee, &[l.into(), r.into()], "num_ov")
                                .unwrap();
                            return Ok(call
                                .try_as_basic_value()
                                .basic()
                                .unwrap_or_else(|| self.i64_ty.const_int(0, false).into()));
                        }
                    }
                }
                let v = match op {
                    BinOp::Add => self.emit_checked_binop(l, r, fv, "sadd")?,
                    BinOp::Sub => self.emit_checked_binop(l, r, fv, "ssub")?,
                    BinOp::Mul => self.emit_checked_binop(l, r, fv, "smul")?,
                    BinOp::Div => self.emit_checked_div_rem(l, r, fv, false)?,
                    BinOp::Rem => self.emit_checked_div_rem(l, r, fv, true)?,
                    BinOp::Eq => self.emit_value_eq(&lt, &rt, l, r)?,
                    BinOp::Ne => {
                        let eq = self.emit_value_eq(&lt, &rt, l, r)?;
                        let z = self.i64_ty.const_int(0, false);
                        let c = self
                            .builder
                            .build_int_compare(IntPredicate::EQ, eq, z, "ne")
                            .unwrap();
                        self.builder
                            .build_int_z_extend(c, self.i64_ty, "nez")
                            .unwrap()
                    }
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        if let Some(name) = Self::adt_method_name(&lt, &rt) {
                            if let Some(_) = self.functions.get(&format!("__Ord_{name}_less")) {
                                // DESIGN less(self, other): a < b
                                let (left, right) = match op {
                                    BinOp::Lt | BinOp::Ge => (l, r),
                                    BinOp::Gt | BinOp::Le => (r, l),
                                    _ => unreachable!(),
                                };
                                let less = self
                                    .emit_less_override(&name, left, right)?
                                    .expect("Ord.less");
                                let z = self.i64_ty.const_int(0, false);
                                return Ok(match op {
                                    BinOp::Lt | BinOp::Gt => less.into(),
                                    BinOp::Le | BinOp::Ge => {
                                        // a <= b  iff  !(b < a); a >= b iff !(a < b)
                                        let c = self
                                            .builder
                                            .build_int_compare(IntPredicate::EQ, less, z, "nless")
                                            .unwrap();
                                        self.builder
                                            .build_int_z_extend(c, self.i64_ty, "nlessz")
                                            .unwrap()
                                            .into()
                                    }
                                    _ => unreachable!(),
                                });
                            }
                        }
                        // Structural Ord via runtime (String/Char/ADT); never SLT pointers.
                        let f = self.module.get_function("lumia_cmp").unwrap();
                        let call = self
                            .builder
                            .build_call(f, &[l.into(), r.into()], "cmp")
                            .unwrap();
                        let cmp = call
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_int_value();
                        let z = self.i64_ty.const_int(0, false);
                        let pred = match op {
                            BinOp::Lt => IntPredicate::SLT,
                            BinOp::Le => IntPredicate::SLE,
                            BinOp::Gt => IntPredicate::SGT,
                            BinOp::Ge => IntPredicate::SGE,
                            _ => unreachable!(),
                        };
                        let c = self
                            .builder
                            .build_int_compare(pred, cmp, z, "ord")
                            .unwrap();
                        self.builder
                            .build_int_z_extend(c, self.i64_ty, "ordz")
                            .unwrap()
                    }
                    BinOp::And => self.builder.build_and(l, r, "and").unwrap(),
                    BinOp::Or => self.builder.build_or(l, r, "or").unwrap(),
                };
                Ok(v.into())
            }
            Value::Unary { op, operand } => {
                let ov = self.local(*operand)?;
                let ot = self
                    .local_tys
                    .get(&operand.0)
                    .cloned()
                    .unwrap_or(Type::Int);
                if matches!(ot, Type::Float) || matches!(ov, BasicValueEnum::FloatValue(_)) {
                    let o = self.promote_f64(ov)?;
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
                // Temporary `lumia_string_cstr` buffers are unmarked heap objects;
                // root them until after the foreign call so a later arg alloc / GC
                // cannot collect an earlier cstr (UAF).
                let cstr_root_depth = self.root_depth;
                let mut av: Vec<BasicMetadataValueEnum> = vec![];
                for (i, a) in args.iter().enumerate() {
                    let pty = param_tys.get(i).unwrap_or(&Type::Int);
                    if is_ext {
                        if matches!(pty, Type::String) {
                            let s_i = self.coerce_i64(self.local(*a)?)?;
                            let ptr_ty = self.context.ptr_type(AddressSpace::default());
                            let s = self
                                .builder
                                .build_int_to_ptr(s_i, ptr_ty, "cstr_in")
                                .unwrap();
                            let f = self.module.get_function("lumia_string_cstr").unwrap();
                            let call = self
                                .builder
                                .build_call(f, &[s.into()], "cstr")
                                .unwrap();
                            let cstr = call
                                .try_as_basic_value()
                                .basic()
                                .unwrap()
                                .into_pointer_value();
                            let bits = self
                                .builder
                                .build_ptr_to_int(cstr, self.i64_ty, "cstr_bits")
                                .unwrap();
                            self.root_push_i64(bits)?;
                            av.push(cstr.into());
                        } else {
                            av.push(self.emit_c_abi_arg(*a, pty)?);
                        }
                    } else {
                        let v = self.coerce_i64(self.local(*a)?)?;
                        av.push(v.into());
                    }
                }
                let call = self.builder.build_call(callee, &av, "call").unwrap();
                if is_ext {
                    self.root_pop_to(cstr_root_depth);
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
                let _ = args;
                // Float return ABI must come from the callee's Fun type — never
                // from "any arg is float" (that breaks Float→Int HOFs).
                let float_ret = match self.local_tys.get(&callee.0) {
                    Some(Type::Fun(_, ret, _)) => matches!(ret.as_ref(), Type::Float),
                    _ => self
                        .funref_locals
                        .get(&callee.0)
                        .and_then(|name| self.fun_ret_tys.get(name))
                        .is_some_and(|ty| matches!(ty, Type::Float)),
                };
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
                    let arg_ty = self
                        .local_tys
                        .get(&args[0].0)
                        .cloned()
                        .unwrap_or(Type::Int);
                    match arg_ty {
                        Type::Float => {
                            let f = match arg {
                                BasicValueEnum::FloatValue(f) => f,
                                other => self.promote_f64(other)?,
                            };
                            let fun = self.module.get_function("lumia_println_float").unwrap();
                            self.builder
                                .build_call(fun, &[f.into()], "println_float")
                                .unwrap();
                        }
                        Type::Bool => {
                            let i = self.coerce_i64(arg)?;
                            let b = self
                                .builder
                                .build_int_truncate(i, self.context.i8_type(), "bool8")
                                .unwrap();
                            let fun = self.module.get_function("lumia_println_bool").unwrap();
                            self.builder
                                .build_call(fun, &[b.into()], "println_bool")
                                .unwrap();
                        }
                        Type::Adt { name, params } => {
                            let ptr = if let Some(ptr) = self.emit_show_override(&name, arg)? {
                                Some(ptr)
                            } else if params
                                .iter()
                                .any(|p| matches!(p, Type::Float | Type::Bool))
                            {
                                Some(self.emit_typed_adt_show(arg, &params)?)
                            } else {
                                None
                            };
                            if let Some(ptr) = ptr {
                                let len_f = self.module.get_function("lumia_str_len").unwrap();
                                let len = self
                                    .builder
                                    .build_call(len_f, &[ptr.into()], "show_len")
                                    .unwrap()
                                    .try_as_basic_value()
                                    .basic()
                                    .unwrap()
                                    .into_int_value();
                                let fun = self.module.get_function("lumia_println_str").unwrap();
                                self.builder
                                    .build_call(fun, &[ptr.into(), len.into()], "println_show")
                                    .unwrap();
                            } else {
                                let i = self.coerce_i64(arg)?;
                                let fun = self.module.get_function("lumia_println_auto").unwrap();
                                self.builder
                                    .build_call(fun, &[i.into()], "println")
                                    .unwrap();
                            }
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
                    let mut map = self
                        .builder
                        .build_int_to_ptr(map_i, ptr_ty, "col_ptr")
                        .unwrap();
                    if matches!(self.local_tys.get(&args[1].0), Some(Type::Float)) {
                        let ens = self.module.get_function("lumia_ensure_map_f64").unwrap();
                        map = self
                            .builder
                            .build_call(ens, &[map.into()], "ens_mf64")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_pointer_value();
                    }
                    if matches!(self.local_tys.get(&args[2].0), Some(Type::Float)) {
                        let ens = self.module.get_function("lumia_ensure_map_vf64").unwrap();
                        map = self
                            .builder
                            .build_call(ens, &[map.into()], "ens_mvf64")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_pointer_value();
                    }
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
                    let mut set = self
                        .builder
                        .build_int_to_ptr(set_i, ptr_ty, "set_ptr")
                        .unwrap();
                    if matches!(self.local_tys.get(&args[1].0), Some(Type::Float)) {
                        let ens = self.module.get_function("lumia_ensure_set_f64").unwrap();
                        set = self
                            .builder
                            .build_call(ens, &[set.into()], "ens_sf64")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_pointer_value();
                    }
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
                Builtin::MapKeys | Builtin::MapValues | Builtin::MapItems | Builtin::Elems => {
                    let map_i = self.coerce_i64(self.local(args[0])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let map = self
                        .builder
                        .build_int_to_ptr(map_i, ptr_ty, "map_ptr")
                        .unwrap();
                    let fname = match name {
                        Builtin::MapKeys => "lumia_map_keys",
                        Builtin::MapValues => "lumia_map_values",
                        Builtin::Elems => "lumia_elems",
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
                    let mut list = self
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "list_ptr")
                        .unwrap();
                    if matches!(self.local_tys.get(&args[1].0), Some(Type::Float)) {
                        let ens = self.module.get_function("lumia_ensure_list_f64").unwrap();
                        list = self
                            .builder
                            .build_call(ens, &[list.into()], "ens_lf64")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap()
                            .into_pointer_value();
                    }
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
                    let arg_ty = self
                        .local_tys
                        .get(&args[0].0)
                        .cloned()
                        .unwrap_or(Type::Int);
                    let ptr = match arg_ty {
                        Type::Float => {
                            let f = match arg {
                                BasicValueEnum::FloatValue(f) => f,
                                other => self.promote_f64(other)?,
                            };
                            let fun = self.module.get_function("lumia_show_float").unwrap();
                            self.builder
                                .build_call(fun, &[f.into()], "show_float")
                                .unwrap()
                                .try_as_basic_value()
                                .basic()
                                .unwrap()
                                .into_pointer_value()
                        }
                        Type::Bool => {
                            let i = self.coerce_i64(arg)?;
                            let b = self
                                .builder
                                .build_int_truncate(i, self.context.i8_type(), "bool8")
                                .unwrap();
                            let fun = self.module.get_function("lumia_show_bool").unwrap();
                            self.builder
                                .build_call(fun, &[b.into()], "show_bool")
                                .unwrap()
                                .try_as_basic_value()
                                .basic()
                                .unwrap()
                                .into_pointer_value()
                        }
                        Type::Adt { name, params } => {
                            if let Some(ptr) = self.emit_show_override(&name, arg)? {
                                ptr
                            } else if params
                                .iter()
                                .any(|p| matches!(p, Type::Float | Type::Bool))
                            {
                                self.emit_typed_adt_show(arg, &params)?
                            } else {
                                let i = self.coerce_i64(arg)?;
                                let fun = self.module.get_function("lumia_show").unwrap();
                                self.builder
                                    .build_call(fun, &[i.into()], "show")
                                    .unwrap()
                                    .try_as_basic_value()
                                    .basic()
                                    .unwrap()
                                    .into_pointer_value()
                            }
                        }
                        _ => {
                            let i = self.coerce_i64(arg)?;
                            let fun = self.module.get_function("lumia_show").unwrap();
                            self.builder
                                .build_call(fun, &[i.into()], "show")
                                .unwrap()
                                .try_as_basic_value()
                                .basic()
                                .unwrap()
                                .into_pointer_value()
                        }
                    };
                    Ok(self
                        .builder
                        .build_ptr_to_int(ptr, self.i64_ty, "show_i64")
                        .unwrap()
                        .into())
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
                Builtin::ListParFold => {
                    let list_i = self.coerce_i64(self.local(args[0])?)?;
                    let init_i = self.coerce_i64(self.local(args[1])?)?;
                    let fun_i = self.coerce_i64(self.local(args[2])?)?;
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let list = self
                        .builder
                        .build_int_to_ptr(list_i, ptr_ty, "pfold_list")
                        .unwrap();
                    let one = self.i64_ty.const_int(1, false);
                    let tagged = self.builder.build_and(fun_i, one, "pfold_tag").unwrap();
                    let is_funref = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, tagged, one, "pfold_is_fr")
                        .unwrap();
                    let cur = self
                        .builder
                        .get_insert_block()
                        .context("par_fold needs insert block")?;
                    let parent = cur.get_parent().context("bb parent")?;
                    let ok_bb = self.context.append_basic_block(parent, "pfold_ok");
                    let bad_bb = self.context.append_basic_block(parent, "pfold_bad");
                    self.builder
                        .build_conditional_branch(is_funref, ok_bb, bad_bb)
                        .unwrap();
                    self.builder.position_at_end(bad_bb);
                    let fail = self.module.get_function("lumia_match_fail").unwrap();
                    self.builder.build_call(fail, &[], "pfold_bad_fn").unwrap();
                    self.builder.build_unreachable().unwrap();
                    self.builder.position_at_end(ok_bb);
                    let cleared = self
                        .builder
                        .build_and(
                            fun_i,
                            self.builder.build_not(one, "pfold_not1").unwrap(),
                            "pfold_clear",
                        )
                        .unwrap();
                    let fptr = self
                        .builder
                        .build_int_to_ptr(cleared, ptr_ty, "pfold_fn")
                        .unwrap();
                    let f = self.module.get_function("lumia_list_par_fold").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[list.into(), init_i.into(), fptr.into()], "par_fold")
                        .unwrap();
                    Ok(call
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_int_value()
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
            Value::ClosureCap {
                env,
                index,
                as_float,
            } => {
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
                let loaded = self
                    .builder
                    .build_load(self.i64_ty, slot, "cap")
                    .unwrap();
                if *as_float {
                    Ok(self
                        .builder
                        .build_bit_cast(
                            loaded.into_int_value(),
                            self.context.f64_type(),
                            "cap_f64",
                        )
                        .unwrap()
                        .into())
                } else {
                    Ok(loaded)
                }
            }
            Value::AllocList { elems, repr } => {
                // Empty → immortal singleton. Non-escaping LitList → stack header+payload
                // (same layout as heap so RT len/get work). Escaping → heap.
                let float_elems = elems
                    .first()
                    .and_then(|e| self.local_tys.get(&e.0).cloned())
                    .is_some_and(|t| matches!(t, Type::Float));
                let list_tid = if float_elems {
                    14 /* TYPE_LIST_F64 */
                } else {
                    3 /* TYPE_LIST */
                };
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
                    return self.emit_stack_array(elems, list_tid);
                }
                self.emit_heap_array(elems, list_tid)
            }
            Value::AllocSet { elems, repr } => {
                let elem_ty = elems
                    .first()
                    .and_then(|e| self.local_tys.get(&e.0).cloned())
                    .unwrap_or(Type::Int);
                let float_elems = matches!(elem_ty, Type::Float);
                let no_hash = !self.key_type_has_hash(&elem_ty);
                let tid = if float_elems {
                    11 /* TYPE_SET_F64 */
                } else if no_hash {
                    13 /* TYPE_SET_ASSOC */
                } else {
                    5 /* TYPE_SET */
                };
                if !elems.is_empty() && matches!(repr, lumia_core::SetRepr::LitSet) {
                    return self.emit_stack_array(elems, tid);
                }
                let v = self.emit_heap_array(elems, tid)?;
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
            Value::AllocMap { flat_pairs, repr } => {
                // Layout: [len][k0][v0]... — len = pair count
                if flat_pairs.len() % 2 != 0 {
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
                let no_hash = matches!(repr, lumia_core::MapRepr::AssocList)
                    || !self.key_type_has_hash(&key_ty);
                // Float-value tags win over Assoc for IEEE value ==; Assoc is for
                // key Hash absence (linear forever) when values are not Float.
                let tid = match (float_keys, float_vals, no_hash) {
                    (true, true, _) => 16,  // TYPE_MAP_F64V
                    (true, false, _) => 10, // TYPE_MAP_F64
                    (false, true, _) => 15, // TYPE_MAP_VF64
                    (false, false, true) => 12, // TYPE_MAP_ASSOC
                    (false, false, false) => 4, // TYPE_MAP
                };
                if n_pairs > 0 && matches!(repr, lumia_core::MapRepr::LitMap) {
                    return self.emit_stack_map(flat_pairs, tid);
                }
                let nbytes = self.i64_ty.const_int((1 + flat_pairs.len() as u64) * 8, false);
                let type_id = self.context.i32_type().const_int(tid, false);
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
                let ptr = if !no_hash
                    && (n_pairs > 8 || matches!(repr, lumia_core::MapRepr::HashOrdered))
                {
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
            Value::AllocAdt {
                tag,
                fields,
                repr,
                ..
            } => {
                if matches!(repr, lumia_core::AdtRepr::LitAdt) {
                    return self.emit_stack_adt(*tag, fields);
                }
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

    /// Stack ADT: ObjectHeader + `[tag][field0]…` payload (TYPE_ADT).
    /// Escape analysis must ensure the pointer never outlives the frame.
    fn emit_stack_adt(
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
        let cur = self
            .builder
            .get_insert_block()
            .context("no insert block")?;
        match entry.get_first_instruction() {
            Some(first) => self.builder.position_before(&first),
            None => self.builder.position_at_end(entry),
        }
        let storage = self.builder.build_alloca(arr_ty, "stack_adt").unwrap();
        self.builder.position_at_end(cur);

        let type_id = 6u64; // TYPE_ADT
        let hdr0 = self
            .i64_ty
            .const_int(type_id | ((payload_bytes as u64) << 32), false);
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
        self.builder
            .build_store(hdr1_slot, self.i64_ty.const_int(1, false))
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
    fn emit_stack_array(
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
        let cur = self
            .builder
            .get_insert_block()
            .context("no insert block")?;
        match entry.get_first_instruction() {
            Some(first) => self.builder.position_before(&first),
            None => self.builder.position_at_end(entry),
        }
        let storage = self.builder.build_alloca(arr_ty, "stack_arr").unwrap();
        self.builder.position_at_end(cur);

        let hdr0 = self
            .i64_ty
            .const_int(type_id | ((payload_bytes as u64) << 32), false);
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
    fn emit_stack_map(
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
        let cur = self
            .builder
            .get_insert_block()
            .context("no insert block")?;
        match entry.get_first_instruction() {
            Some(first) => self.builder.position_before(&first),
            None => self.builder.position_at_end(entry),
        }
        let storage = self.builder.build_alloca(arr_ty, "stack_map").unwrap();
        self.builder.position_at_end(cur);

        let hdr0 = self
            .i64_ty
            .const_int(type_id | ((payload_bytes as u64) << 32), false);
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

/// Types allowed on pure TCO SCCs (DESIGN §4.4). Heap params OK: entry re-roots;
/// callers `root_pop_to(0)` immediately before musttail. Closures stay out.
fn tco_eligible_ty(t: &Type) -> bool {
    match t {
        Type::Int | Type::Bool | Type::Float | Type::Var(_) => true,
        Type::String | Type::Char | Type::Unit => true,
        Type::List(_) | Type::Set(_) | Type::Map(_, _) | Type::Adt { .. } | Type::Tuple(_) => {
            true
        }
        Type::Fun(_, _, _) => false,
    }
}

fn compute_tco_sccs(core: &CoreModule) -> HashMap<String, HashSet<String>> {
    let eligible: HashSet<String> = core
        .functions
        .iter()
        .filter(|f| {
            // DESIGN §4.4: pure mutual recursion is guaranteed; IO is not required
            // to TCO, but eligible Int/heap-param SCCs still get musttail when the
            // recursive edge is a direct/FunRef call (IO on other arms is fine).
            f.memo.is_none()
                && f.external.is_none()
                && tco_eligible_ty(&f.ret_ty)
                && f.param_tys.iter().all(tco_eligible_ty)
        })
        .map(|f| f.name.clone())
        .collect();
    if eligible.is_empty() {
        return HashMap::new();
    }
    let mut graph: HashMap<String, HashSet<String>> = HashMap::new();
    for name in &eligible {
        graph.insert(name.clone(), HashSet::new());
    }
    for f in &core.functions {
        if !eligible.contains(&f.name) {
            continue;
        }
        let mut callees = HashSet::new();
        collect_direct_calls(&f.body, &mut callees);
        for c in callees {
            if eligible.contains(&c) {
                graph.get_mut(&f.name).unwrap().insert(c);
            }
        }
    }
    // Tarjan SCC
    let mut index = 0u32;
    let mut stack: Vec<String> = Vec::new();
    let mut on_stack: HashSet<String> = HashSet::new();
    let mut indices: HashMap<String, u32> = HashMap::new();
    let mut lowlink: HashMap<String, u32> = HashMap::new();
    let mut sccs: Vec<HashSet<String>> = Vec::new();

    fn strongconnect(
        v: &str,
        graph: &HashMap<String, HashSet<String>>,
        index: &mut u32,
        stack: &mut Vec<String>,
        on_stack: &mut HashSet<String>,
        indices: &mut HashMap<String, u32>,
        lowlink: &mut HashMap<String, u32>,
        sccs: &mut Vec<HashSet<String>>,
    ) {
        indices.insert(v.to_string(), *index);
        lowlink.insert(v.to_string(), *index);
        *index += 1;
        stack.push(v.to_string());
        on_stack.insert(v.to_string());
        if let Some(ns) = graph.get(v) {
            for w in ns {
                if !indices.contains_key(w) {
                    strongconnect(w, graph, index, stack, on_stack, indices, lowlink, sccs);
                    let lw = *lowlink.get(w).unwrap();
                    let lv = *lowlink.get(v).unwrap();
                    lowlink.insert(v.to_string(), lv.min(lw));
                } else if on_stack.contains(w) {
                    let iw = *indices.get(w).unwrap();
                    let lv = *lowlink.get(v).unwrap();
                    lowlink.insert(v.to_string(), lv.min(iw));
                }
            }
        }
        if lowlink.get(v) == indices.get(v) {
            let mut comp = HashSet::new();
            loop {
                let w = stack.pop().unwrap();
                on_stack.remove(&w);
                comp.insert(w.clone());
                if w == v {
                    break;
                }
            }
            // Keep SCCs that can recurse (size>1 or self-loop).
            let self_loop = graph.get(v).map(|s| s.contains(v)).unwrap_or(false);
            if comp.len() > 1 || self_loop {
                sccs.push(comp);
            }
        }
    }

    let nodes: Vec<String> = eligible.iter().cloned().collect();
    for n in nodes {
        if !indices.contains_key(&n) {
            strongconnect(
                &n,
                &graph,
                &mut index,
                &mut stack,
                &mut on_stack,
                &mut indices,
                &mut lowlink,
                &mut sccs,
            );
        }
    }

    let mut out = HashMap::new();
    for scc in sccs {
        for m in &scc {
            out.insert(m.clone(), scc.clone());
        }
    }
    out
}

fn collect_direct_calls(block: &Block, out: &mut HashSet<String>) {
    collect_calls_with_funrefs(block, &HashMap::new(), out);
}

/// Collect direct callees, resolving `FunRef` → `IndirectCall` (for TCO SCCs).
fn collect_calls_with_funrefs(
    block: &Block,
    parent_funrefs: &HashMap<u32, String>,
    out: &mut HashSet<String>,
) {
    let mut funref_of = parent_funrefs.clone();
    for op in &block.ops {
        let value = match op {
            Op::Let { local, value, .. } => {
                match value {
                    Value::Call { fun, .. } => {
                        out.insert(fun.clone());
                    }
                    Value::IndirectCall { callee, .. } => {
                        if let Some(fun) = funref_of.get(&callee.0) {
                            out.insert(fun.clone());
                        }
                    }
                    Value::If {
                        then_block,
                        else_block,
                        ..
                    } => {
                        collect_calls_with_funrefs(then_block, &funref_of, out);
                        collect_calls_with_funrefs(else_block, &funref_of, out);
                    }
                    Value::Loop {
                        header,
                        body,
                        latch,
                    } => {
                        collect_calls_with_funrefs(header, &funref_of, out);
                        collect_calls_with_funrefs(body, &funref_of, out);
                        collect_calls_with_funrefs(latch, &funref_of, out);
                    }
                    _ => {}
                }
                if let Value::FunRef(name) = value {
                    funref_of.insert(local.0, name.clone());
                } else if let Value::Local(Local(src)) = value {
                    if let Some(n) = funref_of.get(src).cloned() {
                        funref_of.insert(local.0, n);
                    } else {
                        funref_of.remove(&local.0);
                    }
                } else {
                    funref_of.remove(&local.0);
                }
                continue;
            }
            Op::Effect { value } => value,
            _ => continue,
        };
        match value {
            Value::Call { fun, .. } => {
                out.insert(fun.clone());
            }
            Value::IndirectCall { callee, .. } => {
                if let Some(fun) = funref_of.get(&callee.0) {
                    out.insert(fun.clone());
                }
            }
            Value::If {
                then_block,
                else_block,
                ..
            } => {
                collect_calls_with_funrefs(then_block, &funref_of, out);
                collect_calls_with_funrefs(else_block, &funref_of, out);
            }
            Value::Loop {
                header,
                body,
                latch,
            } => {
                collect_calls_with_funrefs(header, &funref_of, out);
                collect_calls_with_funrefs(body, &funref_of, out);
                collect_calls_with_funrefs(latch, &funref_of, out);
            }
            _ => {}
        }
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
    // `lumia_rt` is a Rust staticlib: pull in the host libs Rust std needs.
    // (Matches `cargo rustc -p lumia_rt -- --print=native-static-libs`.)
    if cfg!(target_os = "windows") {
        cmd.args([
            "-ladvapi32",
            "-lws2_32",
            "-luserenv",
            "-lbcrypt",
            "-lntdll",
            // Match the compiler binary stack (see .cargo/config.toml).
            "-Wl,/STACK:16777216",
        ]);
    } else {
        cmd.arg("-lpthread").arg("-ldl").arg("-lm").arg("-lrt").arg("-lutil");
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
    let preferred = if release { "release" } else { "debug" };
    let fallback = if release { "debug" } else { "release" };
    let profiles = [preferred, fallback];
    let mut found_preferred: Option<PathBuf> = None;
    let mut found_fallback: Option<PathBuf> = None;
    for p in profiles {
        for name in ["liblumia_rt.a", "lumia_rt.lib", "lumia_rt.dll.lib"] {
            let c = target_dir.join(p).join(name);
            if c.exists() {
                if p == preferred {
                    found_preferred = Some(c);
                } else if found_fallback.is_none() {
                    found_fallback = Some(c);
                }
                break;
            }
        }
    }
    if let Some(c) = found_preferred {
        return Ok(c);
    }
    for name in ["liblumia_rt.a", "lumia_rt.lib"] {
        let c = target_dir.join(name);
        if c.exists() {
            return Ok(c);
        }
    }
    if let Some(c) = found_fallback {
        eprintln!(
            "warning: linking {} lumia_rt into a {} build ({}); run `cargo build -p lumia_rt{}` for a matching runtime",
            fallback,
            preferred,
            c.display(),
            if release { " --release" } else { "" },
        );
        return Ok(c);
    }
    bail!(
        "liblumia_rt.a / lumia_rt.lib not found under {} — run `cargo build -p lumia_rt` first",
        target_dir.display()
    )
}
