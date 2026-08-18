use lumia_core::{max_local_in_fun, Block, CoreFun, CoreModule, FunKind, Local, Op, Value};
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashSet as HashSet;

const DOMAIN_RT_SYMS: &[&str] = &[
    "lumia_collatz_total",
    "lumia_collatz_strided",
    "lumia_count_primes",
    "lumia_affine2_rem_sum",
    "lumia_gcd_sum",
    "lumia_divisor_sum",
    "lumia_product_rem_sum",
    "lumia_affine1_rem_sum",
    "lumia_matmul_affine_checksum",
    "lumia_mandelbrot_checksum",
    "lumia_mem_traffic_checksum",
];

/// Argument to an injected RT Call: function param or Int literal.
#[derive(Debug, Clone, Copy)]
pub(super) enum RtArg {
    Param(usize),
    Const(i64),
}

pub(super) fn ensure_external(module: &mut CoreModule, sym: &str) {
    debug_assert!(
        DOMAIN_RT_SYMS.contains(&sym),
        "domain_sr may only inject known RT kernels; got {sym}"
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
    let param_names: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    module.functions.push(CoreFun {
        name: sym.to_string(),
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
    match sym {
        "lumia_collatz_total"
        | "lumia_count_primes"
        | "lumia_gcd_sum"
        | "lumia_divisor_sum"
        | "lumia_mandelbrot_checksum" => (vec![Type::Int], Type::Int),
        "lumia_collatz_strided" => (vec![Type::Int, Type::Int, Type::Int], Type::Int),
        "lumia_product_rem_sum" | "lumia_matmul_affine_checksum" => {
            (vec![Type::Int, Type::Int], Type::Int)
        }
        "lumia_affine1_rem_sum" => (vec![Type::Int, Type::Int, Type::Int, Type::Int], Type::Int),
        "lumia_affine2_rem_sum" => (
            vec![Type::Int, Type::Int, Type::Int, Type::Int, Type::Int],
            Type::Int,
        ),
        "lumia_mem_traffic_checksum" => (vec![Type::Int, Type::Int, Type::Int], Type::Int),
        other => panic!("domain_sr external_sig: unknown {other}"),
    }
}

pub(super) fn rewrite_body_to_call(fun: &mut CoreFun, sym: &str) {
    let args: Vec<RtArg> = (0..fun.params.len()).map(RtArg::Param).collect();
    rewrite_body_to_rt(fun, sym, &args);
}

pub(super) fn rewrite_body_to_rt(fun: &mut CoreFun, sym: &str, args: &[RtArg]) {
    let mut next = max_local_in_fun(fun).saturating_add(1);
    let mut ops = Vec::new();
    let mut call_args = Vec::with_capacity(args.len());
    for a in args {
        match *a {
            RtArg::Param(i) => {
                call_args.push(
                    *fun.params
                        .get(i)
                        .unwrap_or_else(|| panic!("domain_sr: bad param index {i} for {sym}")),
                );
            }
            RtArg::Const(c) => {
                let l = Local(next);
                next = next.saturating_add(1);
                ops.push(Op::Let {
                    local: l,
                    value: Value::Int(c),
                    pure_region: true,
                });
                call_args.push(l);
            }
        }
    }
    let r = Local(next);
    ops.push(Op::Let {
        local: r,
        value: Value::Call {
            fun: sym.into(),
            args: call_args,
        },
        pure_region: true,
    });
    fun.body = Block {
        ops,
        result: Some(r),
    };
    let (_param_tys, ret_ty) = external_sig(sym);
    fun.ret_ty = ret_ty;
    fun.effect = Effect::pure();
    fun.nsw_binop_locals = Default::default();
    fun.safe_divisor_locals = Default::default();
    fun.nonneg_iv_load_locals = Default::default();
}
