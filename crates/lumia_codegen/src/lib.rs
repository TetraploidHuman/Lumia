//! LLVM codegen via inkwell (LLVM 21). Links against `lumia_rt`.

mod emit_value;
mod link;
mod runtime_decls;
mod tco;

pub use link::find_runtime_lib_prefer;
use link::link_executable;
use runtime_decls::declare_runtime;
use tco::compute_tco_sccs;

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
use inkwell::{AddressSpace, IntPredicate, OptimizationLevel};
use lumia_core::{Block, CoreFun, CoreModule, Local, MemoTf, Op, Value};
use lumia_hir::Builtin;
use lumia_ty::Type;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

fn core_fun_is_param0_identity(f: &CoreFun) -> bool {
    let Some(p0) = f.params.first().map(|p| p.0) else {
        return false;
    };
    let Some(Local(result)) = f.body.result else {
        return false;
    };
    let mut root: HashMap<u32, u32> = HashMap::new();
    root.insert(p0, p0);
    for op in &f.body.ops {
        match op {
            Op::Let {
                local,
                value: Value::Local(Local(src)),
                ..
            } => {
                if let Some(&r) = root.get(src) {
                    root.insert(local.0, r);
                } else {
                    return false;
                }
            }
            Op::Let { .. } | Op::Effect { .. } | Op::Assign { .. } => return false,
            _ => {}
        }
    }
    root.get(&result) == Some(&p0)
}

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

/// Emit LLVM IR for `core`, run `module.verify()`, and return the IR text.
/// Does not write object files or invoke the linker.
pub fn emit_verified_llvm_ir(core: &CoreModule, opts: &CodegenOptions) -> Result<String> {
    let context = Context::create();
    let cg = emit_llvm_module(&context, core, opts)?;
    let ir = cg.module.print_to_string().to_string();
    Ok(ir)
}

fn emit_llvm_module<'ctx>(
    context: &'ctx Context,
    core: &CoreModule,
    opts: &CodegenOptions,
) -> Result<Codegen<'ctx>> {
    let mut cg = Codegen::new(
        context,
        &core.name,
        opts.option_some_tag,
        opts.option_none_tag,
    );
    declare_runtime(context, &cg.module);
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
            let fn_ty = c_abi_fn_type(context, &f.param_tys, &f.ret_ty);
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
        if core_fun_is_param0_identity(f) {
            cg.fun_param0_identity.insert(f.name.clone());
        }
    }

    for f in &core.functions {
        if f.external.is_some() {
            continue;
        }
        cg.emit_function(f)?;
    }

    emit_c_main(context, &cg.module, &cg.builder, core);

    if opts.emit_ir {
        let ir_path = opts.output.with_extension("ll");
        cg.module
            .print_to_file(&ir_path)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    cg.module
        .verify()
        .map_err(|e| anyhow::anyhow!("LLVM verify: {e}"))?;
    Ok(cg)
}

pub fn compile_module(core: &CoreModule, opts: &CodegenOptions) -> Result<()> {
    let context = Context::create();
    let cg = emit_llvm_module(&context, core, opts)?;

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
        .create_target_machine(&triple, &cpu, &features, opt, reloc, CodeModel::Default)
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

fn emit_c_main<'ctx>(
    context: &'ctx Context,
    module: &LlvmModule<'ctx>,
    builder: &Builder<'ctx>,
    core: &CoreModule,
) {
    let i32_ty = context.i32_type();
    let main_ty = i32_ty.fn_type(&[], false);
    let main_fn = module.add_function("main", main_ty, None);
    let entry = context.append_basic_block(main_fn, "entry");
    builder.position_at_end(entry);
    emit_trait_dict_registration(context, module, builder, core);
    if let Some(user) = module.get_function("lumia_user_main") {
        builder.build_call(user, &[], "call_main").unwrap();
    }
    builder
        .build_return(Some(&i32_ty.const_int(0, false)))
        .unwrap();
}

/// Register mangled instance methods in the runtime trait dictionary.
fn emit_trait_dict_registration<'ctx>(
    context: &'ctx Context,
    module: &LlvmModule<'ctx>,
    builder: &Builder<'ctx>,
    core: &CoreModule,
) {
    let Some(reg) = module.get_function("lumia_dict_register") else {
        return;
    };
    // (prefix, trait_id) — ids match lumia_rt::TRAIT_*.
    let specs: &[(&str, i64)] = &[
        ("__Show_", 1),
        ("__Eq_", 2),
        ("__Ord_", 3),
        ("__Hash_", 4),
        ("__Num_", 5),
    ];
    for f in &core.functions {
        let name = &f.name;
        for &(prefix, trait_id) in specs {
            let Some(rest) = name.strip_prefix(prefix) else {
                continue;
            };
            // `__Show_Point_show` → type `Point`; `__Num_Vec2_add` → `Vec2`.
            let Some((ty_name, _)) = rest.rsplit_once('_') else {
                continue;
            };
            if ty_name.is_empty() {
                continue;
            }
            let Some(fv) = module.get_function(name) else {
                continue;
            };
            let ty_str = builder
                .build_global_string_ptr(ty_name, &format!(".dict.ty.{ty_name}"))
                .expect("dict type name");
            let tid = context.i32_type().const_int(trait_id as u64, false);
            builder
                .build_call(
                    reg,
                    &[
                        tid.into(),
                        ty_str.as_pointer_value().into(),
                        fv.as_global_value().as_pointer_value().into(),
                    ],
                    "",
                )
                .unwrap();
            break;
        }
    }
}

pub(crate) struct Codegen<'ctx> {
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
    /// Funs whose body is `return params[0]` (identity); ListParMap may keep Float tags.
    fun_param0_identity: HashSet<String>,
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
    /// Memo transform for the function being emitted (early `return` must store too).
    current_memo: Option<MemoTf>,
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
            fun_param0_identity: HashSet::new(),
            external_funs: HashSet::new(),
            funref_locals: HashMap::new(),
            local_tys: HashMap::new(),
            slot_tys: HashMap::new(),
            root_depth: 0,
            rooted_slots: HashSet::new(),
            entry_bb: None,
            current_fun: String::new(),
            current_memo: None,
            tco_peers: HashSet::new(),
            tco_sccs: HashMap::new(),
            hash_adts: HashSet::new(),
        }
    }

    pub(crate) fn key_type_has_hash(&self, ty: &Type) -> bool {
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
    pub(crate) fn emit_show_override(
        &mut self,
        adt_name: &str,
        arg: BasicValueEnum<'ctx>,
    ) -> Result<Option<PointerValue<'ctx>>> {
        let mangled = format!("__Show_{adt_name}_show");
        let Some(fv) = self.functions.get(&mangled).copied() else {
            return Ok(None);
        };
        let i = self.coerce_i64(arg)?;
        let call = self.builder.build_call(fv, &[i.into()], "show_ov").unwrap();
        let bits = call.try_as_basic_value().basic().unwrap().into_int_value();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let ptr = self
            .builder
            .build_int_to_ptr(bits, ptr_ty, "show_ov_ptr")
            .unwrap();
        Ok(Some(ptr))
    }

    /// Call `__Eq_{T}_eq(a,b) -> Bool` when present.
    pub(crate) fn emit_eq_override(
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
            call.try_as_basic_value().basic().unwrap().into_int_value(),
        ))
    }

    /// Call `__Ord_{T}_less(a,b) -> Bool` when present.
    pub(crate) fn emit_less_override(
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
            call.try_as_basic_value().basic().unwrap().into_int_value(),
        ))
    }

    fn adt_method_name(left: &Type, right: &Type) -> Option<String> {
        match (left, right) {
            (Type::Adt { name: a, .. }, Type::Adt { name: b, .. }) if a == b => Some(a.clone()),
            _ => None,
        }
    }

    /// Typed `==` for ADTs with Float fields and fallback to `lumia_eq`.
    pub(crate) fn emit_value_eq(
        &mut self,
        lt: &Type,
        rt: &Type,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>> {
        if let Some(name) = Self::adt_method_name(lt, rt) {
            // Hash ADTs use `lumia_eq` for Map/Set keys — keep `==` on the same path
            // so a custom `__Eq_*_eq` cannot diverge from containment.
            if !self.hash_adts.contains(&name) {
                if let Some(eq) = self.emit_eq_override(&name, l, r)? {
                    return Ok(eq);
                }
            }
            if let (Type::Adt { params: lp, .. }, Type::Adt { params: rp, .. }) = (lt, rt) {
                if lp.iter().any(|p| matches!(p, Type::Float))
                    || rp.iter().any(|p| matches!(p, Type::Float))
                {
                    return self.emit_typed_adt_eq(l, r, lp, rp);
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

    /// Bit `i` set ⇒ field `i` uses IEEE eq/show (union of both sides' params).
    pub(crate) fn adt_float_field_mask(lp: &[Type], rp: &[Type]) -> u64 {
        let n = lp.len().max(rp.len()).min(64);
        let mut mask = 0u64;
        for i in 0..n {
            let lf = matches!(lp.get(i), Some(Type::Float));
            let rf = matches!(rp.get(i), Some(Type::Float));
            if lf || rf {
                mask |= 1u64 << i;
            }
        }
        mask
    }

    /// Layout mask from concrete field SSA types at an `AllocAdt` site.
    pub(crate) fn adt_float_mask_from_fields(&self, fields: &[Local]) -> u32 {
        let mut mask = 0u32;
        for (i, f) in fields.iter().enumerate().take(32) {
            if matches!(self.local_tys.get(&f.0), Some(Type::Float)) {
                mask |= 1u32 << i;
            }
        }
        mask
    }

    /// Structural ADT `==` via runtime size (safe for sum None/Ok arity ≠ type params).
    pub(crate) fn emit_typed_adt_eq(
        &mut self,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
        lp: &[Type],
        rp: &[Type],
    ) -> Result<IntValue<'ctx>> {
        let mask = Self::adt_float_field_mask(lp, rp);
        let f = self.module.get_function("lumia_adt_eq").unwrap();
        Ok(self
            .builder
            .build_call(
                f,
                &[
                    left.into(),
                    right.into(),
                    self.i64_ty.const_int(mask, false).into(),
                ],
                "adt_eq",
            )
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_int_value())
    }

    /// Structural ADT show; float_mask selects IEEE formatting per field index.
    pub(crate) fn emit_typed_adt_show(
        &mut self,
        arg: BasicValueEnum<'ctx>,
        params: &[Type],
    ) -> Result<PointerValue<'ctx>> {
        let i = self.coerce_i64(arg)?;
        let mask = Self::adt_float_field_mask(params, &[]);
        let f = self.module.get_function("lumia_show_adt").unwrap();
        Ok(self
            .builder
            .build_call(
                f,
                &[i.into(), self.i64_ty.const_int(mask, false).into()],
                "show_adt",
            )
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value())
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
            Type::Tuple(ts) | Type::TuplePrefix(ts) => ts.iter().any(Self::type_may_heap),
            _ => false,
        }
    }

    pub(crate) fn value_may_heap(&self, v: &Value) -> bool {
        use lumia_core::{value_alloc_may_heap, HeapPolicy};
        if value_alloc_may_heap(v, HeapPolicy::StackLitOk) {
            return true;
        }
        match v {
            Value::IndirectCall { .. } => true,
            // Only when an arm's result may be heap — parent `Let` re-roots after
            // scoped pop. Pure Int/Unit ifs must not allocate root slots.
            Value::If {
                then_block,
                else_block,
                ..
            } => self.block_result_may_heap(then_block) || self.block_result_may_heap(else_block),
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

    pub(crate) fn root_push_i64(&mut self, bits: IntValue<'ctx>) -> Result<()> {
        let slot = self.alloca_in_entry(self.i64_ty, "gc_root")?;
        self.builder.build_store(slot, bits).unwrap();
        let push = self.module.get_function("lumia_root_push").unwrap();
        self.builder.build_call(push, &[slot.into()], "").unwrap();
        self.root_depth += 1;
        Ok(())
    }

    /// `alloca` at function entry so loops do not grow the native stack.
    fn alloca_in_entry(&mut self, ty: IntType<'ctx>, name: &str) -> Result<PointerValue<'ctx>> {
        let entry = self
            .entry_bb
            .context("alloca_in_entry before emit_function")?;
        let cur = self.builder.get_insert_block().context("no insert block")?;
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
        self.builder.build_call(push, &[slot.into()], "").unwrap();
        self.root_depth += 1;
        self.rooted_slots.insert(name.to_string());
    }

    /// Pop shadow-stack entries until `root_depth == depth` (scope exit).
    pub(crate) fn root_pop_to(&mut self, depth: u32) {
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

    fn emit_frame_push(&mut self, name: &str) {
        let push = self.module.get_function("lumia_frame_push").unwrap();
        let s = self
            .builder
            .build_global_string_ptr(name, &format!(".fname.{name}"))
            .expect("global string");
        self.builder
            .build_call(push, &[s.as_pointer_value().into()], "")
            .unwrap();
    }

    fn emit_frame_pop(&mut self) {
        let pop = self.module.get_function("lumia_frame_pop").unwrap();
        self.builder.build_call(pop, &[], "").unwrap();
    }

    fn emit_return_i64(&mut self, ret: IntValue<'ctx>) {
        self.emit_root_epilogue();
        self.emit_frame_pop();
        self.builder.build_return(Some(&ret)).unwrap();
    }

    pub(crate) fn emit_function(&mut self, fun: &CoreFun) -> Result<()> {
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
        self.current_memo = fun.memo;
        self.tco_peers = self.tco_sccs.get(&fun.name).cloned().unwrap_or_default();
        let frame_name = if fun.is_main {
            "main"
        } else {
            fun.name.as_str()
        };
        self.emit_frame_push(frame_name);

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
                self.locals.insert(p.0, f);
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
        let out_alloca = self.builder.build_alloca(self.i64_ty, "memo_out").unwrap();
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

    fn infer_value_ty(&self, value: &Value) -> Type {
        lumia_core::infer_value_ty_ctx(
            value,
            lumia_core::InferValueCtx {
                local_tys: &self.local_tys,
                slot_tys: Some(&self.slot_tys),
                fun_ret_tys: Some(&self.fun_ret_tys),
                fun_param_tys: Some(&self.fun_param_tys),
                fun_param0_identity: Some(&self.fun_param0_identity),
                funref_locals: Some(&self.funref_locals),
            },
            None,
        )
    }

    pub(crate) fn load_slot(&self, name: &str) -> Result<BasicValueEnum<'ctx>> {
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
                .unwrap())
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
                                    self.emit_frame_pop();
                                    if self.emit_musttail_call(fun, args)? {
                                        return Ok(None);
                                    }
                                    // musttail failed — restore frame for normal call path.
                                    self.emit_frame_push(&self.current_fun.clone());
                                }
                            }
                            Value::IndirectCall { callee, args } => {
                                if let Some(fun) = self.funref_locals.get(&callee.0).cloned() {
                                    if self.tco_peers.contains(&fun) {
                                        self.root_pop_to(0);
                                        self.emit_frame_pop();
                                        if self.emit_musttail_call(&fun, args)? {
                                            return Ok(None);
                                        }
                                        self.emit_frame_push(&self.current_fun.clone());
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
                    self.local_tys.insert(local.0, self.infer_value_ty(value));
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
                Op::Return { value } => {
                    let v = self.local(*value)?;
                    let ret_i = if matches!(self.local_tys.get(&value.0), Some(Type::Float)) {
                        match v {
                            BasicValueEnum::FloatValue(f) => self
                                .builder
                                .build_bit_cast(f, self.i64_ty, "ret_f64_bits")
                                .unwrap()
                                .into_int_value(),
                            other => self.coerce_i64(other)?,
                        }
                    } else {
                        self.coerce_i64(v)?
                    };
                    match self.current_memo {
                        Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_store(id, ret_i)?,
                        Some(MemoTf::Slots { id }) => self.emit_memo_l2_store(id, ret_i)?,
                        None => {}
                    }
                    self.emit_return_i64(ret_i);
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

    pub(crate) fn local(&self, l: Local) -> Result<BasicValueEnum<'ctx>> {
        self.locals
            .get(&l.0)
            .copied()
            .with_context(|| format!("undefined local %{}", l.0))
    }

    pub(crate) fn as_i64(&self, v: BasicValueEnum<'ctx>) -> Result<IntValue<'ctx>> {
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

    pub(crate) fn coerce_i64(&self, v: BasicValueEnum<'ctx>) -> Result<IntValue<'ctx>> {
        self.as_i64(v)
    }

    /// Coerce a Lumia local to a C ABI argument for `foreign` calls.
    pub(crate) fn emit_c_abi_arg(
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

    pub(crate) fn restore_c_abi_ret(
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

    pub(crate) fn promote_f64(
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
    pub(crate) fn arith_as_f64(
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

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_opt::{compile_file_to_optimized, OptOptions};
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root")
    }

    fn test_opts() -> CodegenOptions {
        CodegenOptions {
            release: false,
            output: PathBuf::from("/tmp/lumia_codegen_test"),
            emit_ir: false,
            runtime_lib: PathBuf::from("/tmp/unused_rt"),
            option_some_tag: 0,
            option_none_tag: 1,
            parallel: true,
            link_args: vec![],
        }
    }

    fn emit_example(rel: &str, release: bool) -> String {
        let path = workspace_root().join(rel);
        let opts = if release {
            OptOptions::for_build(true)
        } else {
            OptOptions::default()
        };
        let core = compile_file_to_optimized(&path, &opts).expect("optimize");
        emit_verified_llvm_ir(&core, &test_opts()).expect("emit+verify")
    }

    #[test]
    fn emit_tco_sum_has_musttail() {
        let ir = emit_example("examples/tco_sum.lm", false);
        assert!(
            ir.contains("musttail") || ir.contains("tailcc") || ir.contains("tail "),
            "expected musttail-related IR in tco_sum; ir snip:\n{}",
            &ir[..ir.len().min(2000)]
        );
    }

    #[test]
    fn emit_memo_l2_has_lookup() {
        let ir = emit_example("examples/memo_l2.lm", true);
        assert!(
            ir.contains("lumia_memo") || ir.contains("memo"),
            "expected memo runtime calls in memo_l2 IR"
        );
    }

    #[test]
    fn emit_trait_poly_show_has_show_symbol() {
        let ir = emit_example("examples/trait_poly_show.lm", false);
        assert!(
            ir.contains("show") || ir.contains("Show") || ir.contains("__Show"),
            "expected Show-related symbol in trait_poly_show IR"
        );
    }

    #[test]
    fn emit_hello_verifies() {
        let _ir = emit_example("examples/hello.lm", false);
    }
}
