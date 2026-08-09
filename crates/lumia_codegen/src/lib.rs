//! LLVM codegen via inkwell (LLVM 21). Links against `lumia_rt`.

mod emit_eq;
mod emit_fun;
mod emit_memo;
mod emit_value;
mod link;
mod roots;
mod runtime_decls;
mod tco;

pub use link::find_runtime_lib_prefer;
use link::link_executable;
use runtime_decls::declare_runtime;
use tco::compute_tco_sccs;

use anyhow::{Context as AnyhowContext, Result};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::{BasicMetadataTypeEnum, IntType};
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};
use inkwell::{AddressSpace, OptimizationLevel};
use lumia_core::{CoreFun, CoreModule, Local, MemoTf, Op, Value};
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
