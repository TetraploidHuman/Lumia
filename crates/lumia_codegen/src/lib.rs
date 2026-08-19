//! LLVM codegen via inkwell (LLVM 21). Links against `lumia_rt`.
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]
#![allow(clippy::collapsible_match)]

mod attrs;
mod closure_cap_tys;
mod emit_eq;
mod emit_fun;
mod emit_memo;
mod emit_value;
mod error;
mod funref;
mod link;
mod opt_level;
mod roots;
mod runtime_decls;
mod state;
mod tco;

pub use error::CodegenError;
pub use link::find_runtime_lib_prefer;
use link::link_executable;
pub use opt_level::LlvmOptLevel;
use runtime_decls::declare_runtime;
use state::{FrameState, FunTables, LlvmTypes, MemoEmit};
use tco::compute_tco_sccs;

use anyhow::{Context as AnyhowContext, Result};
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::BasicMetadataTypeEnum;
use inkwell::values::FunctionValue;
use inkwell::AddressSpace;
use lumia_core::CoreModule;
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;
use std::path::PathBuf;

pub struct CodegenOptions {
    /// Product mode: strip link, omit trap backtrace frames, pick `lumia_rt` profile.
    /// Independent of [`Self::llvm_opt`] (Debug can still run `default<O1>`).
    pub release: bool,
    /// LLVM new-PM + instruction selection. See [`LlvmOptLevel::from_release`].
    pub llvm_opt: LlvmOptLevel,
    pub output: PathBuf,
    pub emit_ir: bool,
    pub runtime_lib: PathBuf,
    /// Emit `lumia_f64_*` for recognized dense float nests (default on).
    pub dense_f64_sr: bool,
    /// Extra linker args, e.g. `["-lm", "-L/opt/lib", "-lfoo"]`.
    pub link_args: Vec<String>,
}

/// Emit LLVM IR for `core`, run `module.verify()`, and return the IR text.
/// Does not write object files, invoke the linker, or run [`CodegenOptions::llvm_opt`].
pub fn emit_verified_llvm_ir(core: &CoreModule, opts: &CodegenOptions) -> Result<String> {
    let context = Context::create();
    let cg = emit_llvm_module(&context, core, opts)?;
    let ir = cg.llvm.module.print_to_string().to_string();
    Ok(ir)
}

fn emit_llvm_module<'ctx>(
    context: &'ctx Context,
    core: &CoreModule,
    opts: &CodegenOptions,
) -> Result<Codegen<'ctx>> {
    let mut cg = Codegen::new(context, &core.name, opts.release, opts.dense_f64_sr);
    declare_runtime(context, &cg.llvm.module);
    cg.funs.tco_sccs = compute_tco_sccs(core);
    cg.funs.closure_cap_tys = closure_cap_tys::collect_closure_cap_tys(core);

    // Core ABI + analysis blackboard: one ModuleTables seed; FunTables only
    // adds LLVM handles / emit-only maps below.
    let tables = lumia_core::ModuleTables::from_module(core);
    cg.funs.seed_abi_from(tables);
    cg.funs.adt_show_kinds = assign_adt_show_kinds(&cg.funs.adt_variant_names);

    for f in &core.functions {
        let name = if f.is_main {
            "lumia_user_main".to_string()
        } else if let Some(sym) = &f.external {
            sym.clone()
        } else {
            f.name.to_string()
        };
        let fv = if let Some(sym) = &f.external {
            let mut runtime_abi = matches!(f.foreign_abi, lumia_core::ForeignAbi::Runtime);
            // Prefer the declaration from `declare_runtime` when present so
            // LLVM types match `lumia_rt` (e.g. Bool as i64, List as ptr).
            // Surface `foreign "C"` for those symbols still needs Runtime object
            // marshalling (heap ptr, not cstr) — see `std/string.lm`.
            let fv = if let Some(existing) = cg.llvm.module.get_function(sym) {
                runtime_abi = true;
                existing
            } else {
                let fn_ty = if runtime_abi {
                    runtime_abi_fn_type(context, &f.param_tys, &f.ret_ty)
                } else {
                    c_abi_fn_type(context, &f.param_tys, &f.ret_ty)
                };
                let fv = cg.llvm.module.add_function(sym, fn_ty, None);
                fv.set_linkage(inkwell::module::Linkage::External);
                fv
            };
            cg.funs.external_funs.insert(f.name.to_string());
            if runtime_abi {
                cg.funs.runtime_external_funs.insert(f.name.to_string());
            }
            fv
        } else {
            let fn_ty = cg.llvm.i64_ty.fn_type(
                &vec![BasicMetadataTypeEnum::from(cg.llvm.i64_ty); f.params.len()],
                false,
            );
            cg.llvm.module.add_function(&name, fn_ty, None)
        };
        cg.funs.functions.insert(f.name.to_string(), fv);
        attrs::add_nounwind(context, fv);
    }

    for f in &core.functions {
        if f.external.is_some() {
            continue;
        }
        cg.emit_function(f)?;
    }

    emit_c_main(
        context,
        &cg.llvm.module,
        &cg.llvm.builder,
        core,
        &cg.funs.adt_show_kinds,
        &cg.funs.adt_variant_names,
    );

    cg.llvm
        .module
        .verify()
        .map_err(|e| anyhow::anyhow!("LLVM verify: {e}"))?;
    Ok(cg)
}

fn apply_llvm_opt(module: &LlvmModule, tm: &TargetMachine, level: LlvmOptLevel) -> Result<()> {
    let Some(pipeline) = level.pass_pipeline() else {
        return Ok(());
    };
    let pb = PassBuilderOptions::create();
    let loops = level.aggressive_loop_opts();
    pb.set_loop_vectorization(loops);
    pb.set_loop_slp_vectorization(loops);
    pb.set_loop_unrolling(loops);
    module
        .run_passes(pipeline, tm, pb)
        .map_err(|e| anyhow::anyhow!("LLVM run_passes ({pipeline}): {e}"))?;
    // Pre-pipeline verify always runs in `emit_llvm_module`. Post-pipeline is
    // optional: set `LUMIA_VERIFY=1` when hunting LLVM/pass bugs.
    if std::env::var_os("LUMIA_VERIFY").is_some() {
        module
            .verify()
            .map_err(|e| anyhow::anyhow!("LLVM verify after {pipeline}: {e}"))?;
    }
    Ok(())
}

pub fn compile_module(core: &CoreModule, opts: &CodegenOptions) -> Result<()> {
    let context = Context::create();
    let cg = emit_llvm_module(&context, core, opts)?;

    Target::initialize_all(&InitializationConfig::default());
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| anyhow::anyhow!("{e}"))?;
    let cpu = TargetMachine::get_host_cpu_name().to_string();
    let features = TargetMachine::get_host_cpu_features().to_string();
    let opt = opts.llvm_opt.inkwell();
    // PIC is fine on ELF/Mach-O; MSVC COFF expects the default reloc model.
    let reloc = if cfg!(target_os = "windows") {
        RelocMode::Default
    } else {
        RelocMode::PIC
    };
    let tm = target
        .create_target_machine(&triple, &cpu, &features, opt, reloc, CodeModel::Default)
        .context("create target machine")?;

    // Run the LLVM new-PM pipeline before object emit so mem2reg / loop opts
    // see mut-slot allocas. Debug defaults to O1; Release to O3.
    apply_llvm_opt(&cg.llvm.module, &tm, opts.llvm_opt)?;

    if opts.emit_ir {
        let ir_path = opts.output.with_extension("ll");
        cg.llvm
            .module
            .print_to_file(&ir_path)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    let obj_path = if cfg!(target_os = "windows") {
        opts.output.with_extension("obj")
    } else {
        opts.output.with_extension("o")
    };
    tm.write_to_file(&cg.llvm.module, FileType::Object, &obj_path)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Drop LLVM module before linking (owns no further need)
    drop(cg);

    link_executable(
        &obj_path,
        &opts.runtime_lib,
        &opts.output,
        &opts.link_args,
        opts.release,
    )?;
    // Object is only an intermediate; leave it when debugging the link step.
    link::remove_link_object_unless_kept(&obj_path);
    Ok(())
}

fn emit_c_main<'ctx>(
    context: &'ctx Context,
    module: &LlvmModule<'ctx>,
    builder: &Builder<'ctx>,
    core: &CoreModule,
    adt_show_kinds: &HashMap<String, u16>,
    adt_variant_names: &HashMap<String, Vec<String>>,
) {
    let i32_ty = context.i32_type();
    let main_ty = i32_ty.fn_type(&[], false);
    let main_fn = module.add_function("main", main_ty, None);
    let entry = context.append_basic_block(main_fn, "entry");
    builder.position_at_end(entry);
    let _ = emit_trait_dict_registration(context, module, builder, core);
    let _ = emit_adt_show_registration(context, module, builder, adt_show_kinds, adt_variant_names);
    if let Some(user) = module.get_function("lumia_user_main") {
        let _ = builder.build_call(user, &[], "call_main");
    }
    let _ = builder.build_return(Some(&i32_ty.const_int(0, false)));
}

/// Assign stable Show-kind ids (`1..`) for ADTs with known variant labels.
fn assign_adt_show_kinds(names: &HashMap<String, Vec<String>>) -> HashMap<String, u16> {
    let mut keys: Vec<&String> = names.keys().collect();
    keys.sort();
    let mut out = HashMap::default();
    for (i, k) in keys.into_iter().enumerate() {
        // Kind 0 is reserved for anonymous `#tag` fallback.
        let kind = (i + 1) as u16;
        out.insert(k.clone(), kind);
    }
    out
}

/// Register ADT constructor-name tables used by recursive `lumia_show`.
fn emit_adt_show_registration<'ctx>(
    context: &'ctx Context,
    module: &LlvmModule<'ctx>,
    builder: &Builder<'ctx>,
    kinds: &HashMap<String, u16>,
    names: &HashMap<String, Vec<String>>,
) -> Result<()> {
    let Some(reg) = module.get_function("lumia_adt_register_show") else {
        return Ok(());
    };
    let i32_ty = context.i32_type();
    let i64_ty = context.i64_type();
    let ptr_ty = context.ptr_type(AddressSpace::default());
    let mut entries: Vec<(&String, u16)> = kinds.iter().map(|(k, &v)| (k, v)).collect();
    entries.sort_by_key(|(_, kind)| *kind);
    for (adt_name, kind) in entries {
        let Some(labels) = names.get(adt_name) else {
            continue;
        };
        if labels.is_empty() {
            continue;
        }
        let mut label_ptrs = Vec::with_capacity(labels.len());
        for (i, label) in labels.iter().enumerate() {
            let g = builder
                .build_global_string_ptr(label, &format!(".adt.show.{adt_name}.{i}"))
                .context("adt show label")?;
            label_ptrs.push(g.as_pointer_value());
        }
        let arr_ty = ptr_ty.array_type(label_ptrs.len() as u32);
        let global = module.add_global(
            arr_ty,
            Some(AddressSpace::default()),
            &format!(".adt.show.names.{adt_name}"),
        );
        global.set_linkage(inkwell::module::Linkage::Private);
        global.set_constant(true);
        global.set_initializer(&ptr_ty.const_array(&label_ptrs));
        let names_ptr = crate::error::llvm(builder.build_pointer_cast(
            global.as_pointer_value(),
            ptr_ty,
            "adt_show_names_ptr",
        ))?;
        let kind_v = i32_ty.const_int(kind as u64, false);
        let n = i64_ty.const_int(labels.len() as u64, false);
        crate::error::llvm(builder.build_call(
            reg,
            &[kind_v.into(), names_ptr.into(), n.into()],
            "",
        ))?;
    }
    Ok(())
}

/// Register mangled instance methods in the runtime trait dictionary.
fn emit_trait_dict_registration<'ctx>(
    context: &'ctx Context,
    module: &LlvmModule<'ctx>,
    builder: &Builder<'ctx>,
    core: &CoreModule,
) -> Result<()> {
    let Some(reg) = module.get_function("lumia_dict_register") else {
        return Ok(());
    };
    // (prefix, trait_id) — ids from `lumia_abi::TRAIT_*` (must match rt dict).
    let specs: &[(&str, i64)] = &[
        ("__Show_", lumia_abi::TRAIT_SHOW as i64),
        ("__Eq_", lumia_abi::TRAIT_EQ as i64),
        ("__Ord_", lumia_abi::TRAIT_ORD as i64),
        ("__Hash_", lumia_abi::TRAIT_HASH as i64),
        ("__Num_", lumia_abi::TRAIT_NUM as i64),
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
                .context("dict type name")?;
            let tid = context.i32_type().const_int(trait_id as u64, false);
            crate::error::llvm(builder.build_call(
                reg,
                &[
                    tid.into(),
                    ty_str.as_pointer_value().into(),
                    fv.as_global_value().as_pointer_value().into(),
                ],
                "",
            ))?;
            break;
        }
    }
    Ok(())
}

pub(crate) struct Codegen<'ctx> {
    pub(crate) llvm: LlvmTypes<'ctx>,
    pub(crate) funs: FunTables<'ctx>,
    pub(crate) frame: FrameState<'ctx>,
    pub(crate) memo: MemoEmit<'ctx>,
    /// Release builds omit trap backtrace frames (hot-path call overhead).
    pub(crate) release: bool,
    /// Match [`CodegenOptions::dense_f64_sr`].
    pub(crate) dense_f64_sr: bool,
}

impl<'ctx> Codegen<'ctx> {
    fn new(context: &'ctx Context, name: &str, release: bool, dense_f64_sr: bool) -> Self {
        Self {
            llvm: LlvmTypes {
                context,
                module: context.create_module(name),
                builder: context.create_builder(),
                i64_ty: context.i64_type(),
            },
            funs: FunTables::default(),
            frame: FrameState::default(),
            memo: MemoEmit::default(),
            release,
            dense_f64_sr,
        }
    }

    /// `Option::{Some,None}` tag: module `adt_variant_names` if present, else langitem.
    pub(crate) fn option_variant_tag(&self, variant: &str) -> i64 {
        if let Some(names) = self.funs.adt_variant_names.get(lumia_hir::OPTION.name) {
            if let Some(i) = names.iter().position(|n| n == variant) {
                return i as i64;
            }
        }
        lumia_hir::OPTION
            .default_tag(variant)
            .unwrap_or(if variant == "None" { 1 } else { 0 })
    }

    /// Look up a `lumia_rt` symbol declared by [`declare_runtime`].
    ///
    /// Prefer this over ad-hoc `module.get_function`; error text flags declare_runtime drift.
    pub(crate) fn runtime_fn(&self, name: &str) -> Result<FunctionValue<'ctx>> {
        self.llvm
            .module
            .get_function(name)
            .with_context(|| format!("missing runtime function `{name}` (declare_runtime drift?)"))
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

/// ABI for `foreign` symbols implemented in `lumia_rt` (`lumia_*`):
/// String/List as heap object pointers; Bool/Char as i64 (not C `_Bool`).
fn runtime_abi_fn_type<'ctx>(
    context: &'ctx Context,
    params: &[Type],
    ret: &Type,
) -> inkwell::types::FunctionType<'ctx> {
    let i64_ty = context.i64_type();
    let f64_ty = context.f64_type();
    let ptr_ty = context.ptr_type(AddressSpace::default());
    let meta: Vec<BasicMetadataTypeEnum> = params
        .iter()
        .map(|t| match t {
            Type::Float => f64_ty.into(),
            Type::String | Type::List(_) => ptr_ty.into(),
            _ => i64_ty.into(),
        })
        .collect();
    match ret {
        Type::Unit => context.void_type().fn_type(&meta, false),
        Type::Float => f64_ty.fn_type(&meta, false),
        Type::String | Type::List(_) => ptr_ty.fn_type(&meta, false),
        _ => i64_ty.fn_type(&meta, false),
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
