use lumia_core::{max_local_in_fun, Block, CoreFun, CoreModule, FunKind, Local, Op, Value};
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashSet as HashSet;
use std::sync::Arc;

pub(super) fn ensure_external(module: &mut CoreModule, sym: &str) {
    debug_assert!(
        lumia_abi::is_dense_f64_trampoline(sym),
        "dense_f64_sr may only inject trampoline kernels (lumia_abi::DENSE_F64_TRAMPOLINE_SYMS); got {sym}"
    );
    if module
        .functions
        .iter()
        .any(|f| f.name == sym || f.external.as_deref() == Some(sym))
    {
        return;
    }
    let (param_tys, ret_ty) = external_sig(sym);
    let n = param_tys.len();
    let params: Vec<Local> = (0..n as u32).map(Local).collect();
    let param_names: Vec<lumia_syntax::Sym> = (0..n)
        .map(|i| lumia_syntax::Sym::from(format!("a{i}")))
        .collect();
    module.functions.push(CoreFun {
        name: lumia_syntax::Sym::from(sym),
        params,
        param_names,
        param_tys,
        body: Block {
            ops: vec![],
            result: None,
        },
        ret_ty,
        effect: Effect::pure(),
        is_main: false,
        memo: None,
        external: Some(sym.to_string()),
        foreign_abi: lumia_core::ForeignAbi::Runtime,
        escaping: HashSet::default(),
        nsw_binop_locals: Default::default(),
        safe_divisor_locals: Default::default(),
        nonneg_iv_load_locals: Default::default(),
        scheme_poly: false,
        mono_of: None,
        kind: FunKind::Normal,
    });
}

pub(super) fn external_sig(sym: &str) -> (Vec<Type>, Type) {
    let lf = Type::List(Arc::new(Type::Float));
    match sym {
        "lumia_f64_gemv" | "lumia_f64_gemv_t" => (
            vec![Type::Int, Type::Int, lf.clone(), lf.clone(), lf.clone()],
            lf,
        ),
        "lumia_f64_addmm" => (
            vec![
                Type::Int,
                Type::Int,
                lf.clone(),
                lf.clone(),
                lf.clone(),
                Type::Float,
            ],
            lf,
        ),
        "lumia_f64_axpy" => (vec![lf.clone(), Type::Float, lf.clone()], lf),
        "lumia_f64_sub" | "lumia_f64_add" | "lumia_f64_mul" => {
            (vec![lf.clone(), lf.clone(), lf.clone()], lf)
        }
        "lumia_f64_clamp" => (vec![lf.clone(), Type::Float, Type::Float], lf),
        "lumia_f64_scale" | "lumia_f64_fill" => (vec![lf.clone(), Type::Float], lf),
        "lumia_f64_copy" => (vec![lf.clone(), lf.clone()], lf),
        "lumia_list_f64_zeros" => (vec![Type::Int], lf),
        "lumia_f64_sum_sq" | "lumia_f64_mean" | "lumia_f64_std" | "lumia_f64_l2_norm" => {
            (vec![lf], Type::Float)
        }
        "lumia_f64_softmax" => (vec![lf.clone()], lf),
        "lumia_f64_l2_normalize" => (vec![lf.clone(), Type::Float], lf),
        // Scalar helpers (`sqrt`/`exp`) are not trampoline-eligible; inject only
        // uses [`lumia_abi::DENSE_F64_TRAMPOLINE_SYMS`].
        other => panic!("dense_f64_sr external_sig: non-trampoline {other}"),
    }
}

pub(super) fn rewrite_body_to_call(fun: &mut CoreFun, sym: &str) {
    let r = Local(max_local_in_fun(fun).saturating_add(1));
    fun.body = Block {
        ops: vec![Op::Let {
            local: r,
            value: Value::Call {
                fun: sym.into(),
                args: fun.params.clone(),
            },
            pure_region: true,
        }],
        result: Some(r),
    };
    // Stamp RT signature so codegen trampoline sees List/Float (not leftover Int).
    let (param_tys, ret_ty) = external_sig(sym);
    if fun.params.len() == param_tys.len() {
        fun.param_tys = param_tys;
    }
    fun.ret_ty = ret_ty;
    fun.effect = Effect::pure();
}
