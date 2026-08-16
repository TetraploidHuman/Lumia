//! LLVM codegen via inkwell (LLVM 21). Links against `lumia_rt`.

mod attrs;
mod closure_cap_tys;
mod emit_eq;
mod emit_fun;
mod emit_memo;
mod emit_value;
mod error;
mod funref;
mod link;
mod nsw_iv;
mod roots;
mod runtime_decls;
mod state;
mod tco;

pub use error::CodegenError;
pub use link::find_runtime_lib_prefer;
use link::link_executable;
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
use inkwell::{AddressSpace, OptimizationLevel};
use lumia_core::{CoreFun, CoreModule, Local, Op, Value};
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;
use std::path::PathBuf;

pub(crate) fn core_fun_is_param0_identity(f: &CoreFun) -> bool {
    let Some(p0) = f.params.first().map(|p| p.0) else {
        return false;
    };
    let Some(Local(result)) = f.body.result else {
        return false;
    };
    let mut root: HashMap<u32, u32> = HashMap::default();
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
            Op::Let { .. } | Op::Assign { .. } => return false,
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
    /// Emit `lumia_f64_*` for recognized dense float nests (default on).
    pub dense_f64_sr: bool,
    /// Extra linker args, e.g. `["-lm", "-L/opt/lib", "-lfoo"]`.
    pub link_args: Vec<String>,
}

/// Emit LLVM IR for `core`, run `module.verify()`, and return the IR text.
/// Does not write object files or invoke the linker.
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
    let mut cg = Codegen::new(
        context,
        &core.name,
        core.option_some_tag,
        core.option_none_tag,
        opts.release,
        opts.dense_f64_sr,
    );
    declare_runtime(context, &cg.llvm.module);
    cg.funs.tco_sccs = compute_tco_sccs(core);
    cg.funs.hash_adts = core.hash_adts.clone();
    cg.funs.adt_variant_names = core.adt_variant_names.clone();
    cg.funs.sum_max_arity = core.sum_max_arity.clone();
    cg.funs.channel_elem_hint = core.channel_elem_hint.clone();
    cg.funs.channel_elem_by_local = core.channel_elem_by_local.clone();
    cg.funs.closure_cap_tys = closure_cap_tys::collect_closure_cap_tys(core);
    cg.funs.adt_show_kinds = assign_adt_show_kinds(&cg.funs.adt_variant_names);

    for f in &core.functions {
        let name = if f.is_main {
            "lumia_user_main".to_string()
        } else if let Some(sym) = &f.external {
            sym.clone()
        } else {
            f.name.clone()
        };
        let fv = if let Some(sym) = &f.external {
            let runtime_abi = matches!(f.foreign_abi, lumia_core::ForeignAbi::Runtime);
            // Prefer the declaration from `declare_runtime` when present so
            // LLVM types match `lumia_rt` (e.g. Bool as i64, List as ptr).
            let fv = if let Some(existing) = cg.llvm.module.get_function(sym) {
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
            cg.funs.external_funs.insert(f.name.clone());
            if runtime_abi {
                cg.funs.runtime_external_funs.insert(f.name.clone());
            }
            fv
        } else {
            let fn_ty = cg.llvm.i64_ty.fn_type(
                &vec![BasicMetadataTypeEnum::from(cg.llvm.i64_ty); f.params.len()],
                false,
            );
            cg.llvm.module.add_function(&name, fn_ty, None)
        };
        cg.funs.functions.insert(f.name.clone(), fv);
        cg.funs.fun_ret_tys.insert(f.name.clone(), f.ret_ty.clone());
        cg.funs
            .fun_param_tys
            .insert(f.name.clone(), f.param_tys.clone());
        attrs::add_nounwind(context, fv);
        if core_fun_is_param0_identity(f) {
            cg.funs.fun_param0_identity.insert(f.name.clone());
        }
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

    if opts.emit_ir {
        let ir_path = opts.output.with_extension("ll");
        cg.llvm
            .module
            .print_to_file(&ir_path)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    cg.llvm
        .module
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

    // Release: run LLVM new-PM pipeline before object emit so mem2reg / loop
    // opts / vectorize see our mut-slot allocas and checked arithmetic.
    if opts.release {
        let pb = PassBuilderOptions::create();
        pb.set_loop_vectorization(true);
        pb.set_loop_slp_vectorization(true);
        pb.set_loop_unrolling(true);
        cg.llvm
            .module
            .run_passes("default<O3>", &tm, pb)
            .map_err(|e| anyhow::anyhow!("LLVM run_passes: {e}"))?;
        cg.llvm
            .module
            .verify()
            .map_err(|e| anyhow::anyhow!("LLVM verify after O3: {e}"))?;
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
    pub(crate) option_some_tag: i64,
    pub(crate) option_none_tag: i64,
    /// Release builds omit trap backtrace frames (hot-path call overhead).
    pub(crate) release: bool,
    /// Match [`CodegenOptions::dense_f64_sr`].
    pub(crate) dense_f64_sr: bool,
}

impl<'ctx> Codegen<'ctx> {
    fn new(
        context: &'ctx Context,
        name: &str,
        option_some_tag: i64,
        option_none_tag: i64,
        release: bool,
        dense_f64_sr: bool,
    ) -> Self {
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
            option_some_tag,
            option_none_tag,
            release,
            dense_f64_sr,
        }
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
mod tests {
    use super::*;
    use lumia_opt::{compile_file_to_optimized, OptOptions};
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
    }

    fn test_opts() -> CodegenOptions {
        CodegenOptions {
            release: false,
            output: PathBuf::from("/tmp/lumia_codegen_test"),
            emit_ir: false,
            runtime_lib: PathBuf::from("/tmp/unused_rt"),
            dense_f64_sr: true,
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
    fn emit_memo_tf_has_lookup_and_store() {
        let ir = emit_example("examples/memo_tf.lm", true);
        // C ABI symbols stay `lumia_memo_l2_*` (frozen); planner name is `T_f`.
        assert!(
            ir.contains("lumia_memo_l2_lookup"),
            "expected lumia_memo_l2_lookup in memo_tf IR"
        );
        assert!(
            ir.contains("lumia_memo_l2_store"),
            "expected lumia_memo_l2_store in memo_tf IR"
        );
    }

    #[test]
    fn emit_hof_float_apply_keeps_f64_ret() {
        let ir = emit_example("examples/hof_float_apply.lm", false);
        assert!(
            ir.contains("dbl$Float") || ir.contains("apply$"),
            "expected mono Float/HOF clone names in IR; snip:\n{}",
            &ir[..ir.len().min(2500)]
        );
        // Float C ABI uses LLVM `double` for specialized / HOF-refined returns.
        assert!(
            ir.contains("double"),
            "expected f64/`double` ABI in hof_float_apply IR; snip:\n{}",
            &ir[..ir.len().min(2500)]
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

    #[test]
    fn emit_float_map_keys_verifies() {
        let ir = emit_example("examples/float_map_keys.lm", false);
        assert!(
            ir.contains("lumia_ensure_map_f64") || ir.contains("lumia_map"),
            "expected float-map runtime symbols; snip:\n{}",
            &ir[..ir.len().min(2500)]
        );
    }

    #[test]
    fn emit_poly_option_map_verifies() {
        let _ir = emit_example("examples/poly_option_map.lm", false);
    }

    #[test]
    fn emit_par_map_verifies() {
        let ir = emit_example("examples/par_map.lm", false);
        assert!(
            ir.contains("lumia_list_par_map")
                || ir.contains("par_map")
                || ir.contains("ListParMap"),
            "expected parallel map-related IR; snip:\n{}",
            &ir[..ir.len().min(2500)]
        );
    }

    #[test]
    fn runtime_fn_missing_returns_err_not_panic() {
        let context = Context::create();
        let cg = Codegen::new(&context, "empty", 0, 1, false, true);
        let err = cg
            .runtime_fn("lumia_definitely_missing_symbol_zz")
            .expect_err("missing runtime symbol");
        let msg = err.to_string();
        assert!(
            msg.contains("missing runtime") || msg.contains("definitely_missing"),
            "unexpected: {msg}"
        );
    }

    #[test]
    fn codegen_error_display() {
        let e = CodegenError::msg("boom");
        assert_eq!(e.to_string(), "boom");
        let e2 = CodegenError::Llvm("bad".into());
        assert!(e2.to_string().contains("LLVM"));
    }
}
