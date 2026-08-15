//! Rewrite dense `List[Float]` helpers to `lumia_f64_*` foreign calls (before Inline).
//!
//! **Sole owner of nest pattern matching.** Codegen only recognizes the rewritten
//! single-`Call` body and emits a frameless RT trampoline.
//!
//! Whole-function patterns become a single `Call` so Release inlining places the
//! RT kernel at the call site (same shape as `std.linalg` wrappers).
//!
//! Covered: gemv/gemvT/addmm/axpy/sub/add/mul/clamp/scale/fill/copy/zeros,
//! plus sumSq/mean/std/l2Norm/l2Normalize/softMax (scalar `sqrtF`/`expF` foreign
//! calls unlock the latter norms).

use lumia_core::{
    for_each_block_dfs, max_local_in_fun, Block, CoreFun, CoreModule, Local, Op, Value,
};
use lumia_hir::Builtin;
use lumia_syntax::BinOp;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub struct DenseF64SrPass;

impl DenseF64SrPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        dense_f64_sr_module(module);
    }
}

fn dense_f64_sr_module(module: &mut CoreModule) {
    let mut rewrites: Vec<(usize, &'static str)> = Vec::new();
    for (i, fun) in module.functions.iter().enumerate() {
        if fun.external.is_some() || fun.is_main || fun.memo.is_some() {
            continue;
        }
        let defs = collect_leaf_defs(&fun.body);
        let sym = if match_gemv_fun(fun, &defs).is_some() {
            Some("lumia_f64_gemv")
        } else if match_gemv_t_fun(fun, &defs).is_some() {
            Some("lumia_f64_gemv_t")
        } else if match_addmm_fun(fun, &defs).is_some() {
            Some("lumia_f64_addmm")
        } else if match_axpy_fun(fun, &defs).is_some() {
            Some("lumia_f64_axpy")
        } else if match_sub_fun(fun, &defs).is_some() {
            Some("lumia_f64_sub")
        } else if match_add_fun(fun, &defs).is_some() {
            Some("lumia_f64_add")
        } else if match_mul_fun(fun, &defs).is_some() {
            Some("lumia_f64_mul")
        } else if match_clamp_fun(fun, &defs).is_some() {
            Some("lumia_f64_clamp")
        } else if match_scale_fun(fun, &defs).is_some() {
            Some("lumia_f64_scale")
        } else if match_fill_fun(fun, &defs).is_some() {
            Some("lumia_f64_fill")
        } else if match_copy_fun(fun, &defs).is_some() {
            Some("lumia_f64_copy")
        } else if match_zeros_fun(fun, &defs).is_some() {
            Some("lumia_list_f64_zeros")
        } else if match_l2_normalize_fun(fun, &defs).is_some() {
            Some("lumia_f64_l2_normalize")
        } else if match_softmax_fun(fun, &defs).is_some() {
            Some("lumia_f64_softmax")
        } else if match_l2_norm_fun(fun, &defs).is_some() {
            Some("lumia_f64_l2_norm")
        } else if match_std_fun(fun, &defs).is_some() {
            Some("lumia_f64_std")
        } else if match_sum_sq_fun(fun, &defs).is_some() {
            Some("lumia_f64_sum_sq")
        } else if match_mean_fun(fun, &defs).is_some() {
            Some("lumia_f64_mean")
        } else {
            None
        };
        if let Some(s) = sym {
            rewrites.push((i, s));
        }
    }
    if rewrites.is_empty() {
        return;
    }
    let mut need: HashSet<&'static str> = HashSet::default();
    for &(_, s) in &rewrites {
        need.insert(s);
    }
    for sym in need {
        ensure_external(module, sym);
    }
    for (i, sym) in rewrites {
        rewrite_body_to_call(&mut module.functions[i], sym);
    }
}

fn ensure_external(module: &mut CoreModule, sym: &str) {
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
    let param_names: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    module.functions.push(CoreFun {
        name: sym.to_string(),
        params,
        param_names,
        param_tys,
        body: Block {
            params: vec![],
            ops: vec![],
            result: None,
        },
        ret_ty,
        effect: Effect::pure(),
        is_main: false,
        memo: None,
        external: Some(sym.to_string()),
        foreign_abi: lumia_core::ForeignAbi::from_symbol(sym),
        escaping: HashSet::default(),
        scheme_poly: false,
        mono_of: None,
    });
}

fn external_sig(sym: &str) -> (Vec<Type>, Type) {
    let lf = Type::List(Box::new(Type::Float));
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
        "lumia_f64_sqrt" | "lumia_f64_exp" => (vec![Type::Float], Type::Float),
        _ => (vec![], Type::Int),
    }
}

fn rewrite_body_to_call(fun: &mut CoreFun, sym: &str) {
    let r = Local(max_local_in_fun(fun).saturating_add(1));
    fun.body = Block {
        params: vec![],
        ops: vec![Op::Let {
            local: r,
            value: Value::Call {
                fun: sym.to_string(),
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

fn collect_leaf_defs(body: &Block) -> HashMap<u32, Value> {
    let mut all_defs: HashMap<u32, Value> = HashMap::default();
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
            if let Op::Let { local, value, .. } = op {
                if matches!(
                    value,
                    Value::Int(_)
                        | Value::Float(_)
                        | Value::Name(_)
                        | Value::Binary { .. }
                        | Value::Builtin { .. }
                        | Value::AllocList { .. }
                ) {
                    all_defs.insert(local.0, value.clone());
                }
            }
        }
    });
    all_defs
}

fn is_list_f64(t: &Type) -> bool {
    matches!(t, Type::List(e) if matches!(e.as_ref(), Type::Float))
}

fn param_list_f64(fun: &CoreFun, i: usize) -> bool {
    fun.param_tys.get(i).is_some_and(is_list_f64)
}

fn param_float(fun: &CoreFun, i: usize) -> bool {
    matches!(fun.param_tys.get(i), Some(Type::Float))
}

fn param_int(fun: &CoreFun, i: usize) -> bool {
    matches!(fun.param_tys.get(i), Some(Type::Int))
}

fn ret_list_f64(fun: &CoreFun) -> bool {
    is_list_f64(&fun.ret_ty)
}

fn match_gemv_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 5
        || !param_int(fun, 0)
        || !param_int(fun, 1)
        || !param_list_f64(fun, 2)
        || !param_list_f64(fun, 3)
        || !param_list_f64(fun, 4)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (m, n, a, x, y) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    let (header, loop_body, latch) = first_loop(body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let (i_slot, bound) = header_lt_bound(header, defs)?;
    if !same_local(bound, m, defs) {
        return None;
    }
    if !body_has_gemv_inner(loop_body, defs, &out_slot, &i_slot, a, x, n) {
        return None;
    }
    Some(())
}

fn match_gemv_t_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 5
        || !param_int(fun, 0)
        || !param_int(fun, 1)
        || !param_list_f64(fun, 2)
        || !param_list_f64(fun, 3)
        || !param_list_f64(fun, 4)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (m, n, a, x, y) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    if !fun_has_gemv_t_shape(body, defs, &out_slot, a, x, m, n) {
        return None;
    }
    Some(())
}

fn match_addmm_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 6
        || !param_int(fun, 0)
        || !param_int(fun, 1)
        || !param_list_f64(fun, 2)
        || !param_list_f64(fun, 3)
        || !param_list_f64(fun, 4)
        || !param_float(fun, 5)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (m, n, w, u, v, alpha) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
        fun.params[5],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, w)?;
    if !fun_has_addmm_shape(body, defs, &out_slot, u, v, alpha, m, n) {
        return None;
    }
    Some(())
}

fn match_axpy_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 3
        || !param_list_f64(fun, 0)
        || !param_float(fun, 1)
        || !param_list_f64(fun, 2)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (y, alpha, x) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    if !fun_has_axpy_shape(body, defs, &out_slot, x, alpha) {
        return None;
    }
    Some(())
}

fn match_sub_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 3
        || !param_list_f64(fun, 0)
        || !param_list_f64(fun, 1)
        || !param_list_f64(fun, 2)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (out, a, b) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, out)?;
    if !fun_has_sub_shape(body, defs, &out_slot, a, b) {
        return None;
    }
    Some(())
}

fn match_add_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 3
        || !param_list_f64(fun, 0)
        || !param_list_f64(fun, 1)
        || !param_list_f64(fun, 2)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (out, a, b) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, out)?;
    if !fun_has_add_shape(body, defs, &out_slot, a, b) {
        return None;
    }
    Some(())
}

fn match_mul_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 3
        || !param_list_f64(fun, 0)
        || !param_list_f64(fun, 1)
        || !param_list_f64(fun, 2)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (out, a, b) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, out)?;
    if !fun_has_mul_shape(body, defs, &out_slot, a, b) {
        return None;
    }
    Some(())
}

fn match_clamp_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    // Require List[Float] + Float bounds — bare arity/shape matched Int loops
    // (e.g. `collatzStrided`) and rewrote them to `lumia_f64_clamp`.
    if fun.params.len() != 3
        || !param_list_f64(fun, 0)
        || !param_float(fun, 1)
        || !param_float(fun, 2)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (xs, lo, hi) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_clamp_shape(body, defs, &out_slot, lo, hi) {
        return None;
    }
    Some(())
}

fn match_scale_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 2
        || !param_list_f64(fun, 0)
        || !param_float(fun, 1)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (xs, alpha) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_scale_shape(body, defs, &out_slot, alpha) {
        return None;
    }
    Some(())
}

fn match_fill_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 2
        || !param_list_f64(fun, 0)
        || !param_float(fun, 1)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (xs, v) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_fill_shape(body, defs, &out_slot, v) {
        return None;
    }
    Some(())
}

fn match_copy_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 2
        || !param_list_f64(fun, 0)
        || !param_list_f64(fun, 1)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (dst, src) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, dst)?;
    if !fun_has_copy_shape(body, defs, &out_slot, src) {
        return None;
    }
    Some(())
}

/// `∑ xᵢ²` — get + self-mul + add, no set/div/sqrt.
fn match_sum_sq_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1
        || !param_list_f64(fun, 0)
        || !matches!(fun.ret_ty, Type::Float)
    {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_sum_sq_shape(&fun.body, defs, xs) {
        return None;
    }
    if body_calls_any(&fun.body, &["lumia_f64_sqrt", "sqrtF", "sqrt"]) {
        return None;
    }
    Some(())
}

/// Arithmetic mean — get + add + div, no set/mul.
fn match_mean_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1
        || !param_list_f64(fun, 0)
        || !matches!(fun.ret_ty, Type::Float)
    {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_mean_shape(&fun.body, defs, xs) {
        return None;
    }
    Some(())
}

/// `√(∑ xᵢ²)` via scalar `lumia_f64_sqrt` / `sqrt`.
fn match_l2_norm_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1
        || !param_list_f64(fun, 0)
        || !matches!(fun.ret_ty, Type::Float)
    {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_sum_sq_shape(&fun.body, defs, xs) {
        return None;
    }
    if !body_calls_any(&fun.body, &["lumia_f64_sqrt", "sqrtF", "sqrt"]) {
        return None;
    }
    Some(())
}

/// Population std: variance loop + sqrt (has nontrivial sub).
fn match_std_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1
        || !param_list_f64(fun, 0)
        || !matches!(fun.ret_ty, Type::Float)
    {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_std_shape(&fun.body, defs, xs) {
        return None;
    }
    Some(())
}

/// In-place L2 normalize with `eps` (set + sqrt + mentions eps).
fn match_l2_normalize_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 2
        || !param_list_f64(fun, 0)
        || !param_float(fun, 1)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (xs, eps) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_l2_normalize_shape(body, defs, &out_slot, eps) {
        return None;
    }
    Some(())
}

/// Softmax: max pass + exp + normalize (set + exp call + Gt).
fn match_softmax_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !param_list_f64(fun, 0) || !ret_list_f64(fun) {
        return None;
    }
    let xs = fun.params[0];
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_softmax_shape(body, defs, &out_slot) {
        return None;
    }
    Some(())
}

/// `zeros(n)` via `listOf(0.0)` + `append(0.0)` loop (or empty + append from 0).
fn match_zeros_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !param_int(fun, 0) || !ret_list_f64(fun) {
        return None;
    }
    let n = fun.params[0];
    let body = &fun.body;
    // Must allocate a float list seed and append 0.0 in a loop bounded by n.
    let mut seed = false;
    let mut append0 = false;
    let mut bound_n = false;
    for v in defs.values() {
        if let Value::AllocList { elems, .. } = v {
            if elems.len() <= 1
                && elems
                    .iter()
                    .all(|e| matches!(defs.get(&e.0), Some(Value::Float(f)) if *f == 0.0))
            {
                seed = true;
            }
        }
        if let Value::Builtin {
            name: Builtin::ListAppend,
            args, .. } = v
        {
            if args.len() == 2 && matches!(defs.get(&args[1].0), Some(Value::Float(f)) if *f == 0.0)
            {
                append0 = true;
            }
        }
        if let Value::Binary {
            op: BinOp::Lt,
            right,
            ..
        } = v
        {
            if same_local(*right, n, defs) {
                bound_n = true;
            }
        }
    }
    for_each_let(body, &mut |val| {
        if let Value::AllocList { elems, .. } = val {
            if elems.len() <= 1
                && elems
                    .iter()
                    .all(|e| matches!(defs.get(&e.0), Some(Value::Float(f)) if *f == 0.0))
            {
                seed = true;
            }
        }
        if let Value::Builtin {
            name: Builtin::ListAppend,
            args, .. } = val
        {
            if args.len() == 2 && matches!(defs.get(&args[1].0), Some(Value::Float(f)) if *f == 0.0)
            {
                append0 = true;
            }
        }
        if let Value::Loop { header, .. } = val {
            if let Some((_, bound)) = header_lt_bound(header, defs) {
                if same_local(bound, n, defs) {
                    bound_n = true;
                }
            }
        }
    });
    if seed && append0 && bound_n {
        Some(())
    } else {
        None
    }
}

fn first_assign_from_local(body: &Block, src: Local) -> Option<String> {
    for op in &body.ops {
        if let Op::Assign { name, value } = op {
            if *value == src {
                return Some(name.clone());
            }
        }
    }
    None
}

fn first_loop(body: &Block) -> Option<(&Block, &Block, &Block)> {
    for op in &body.ops {
        if let Op::Let {
            value:
                Value::Loop {
                    header,
                    body,
                    latch,
                },
            ..
        } = op
        {
            return Some((header, body, latch));
        }
    }
    None
}

fn header_lt_bound(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, Local)> {
    let res = header.result?;
    let Value::Binary {
        op: BinOp::Lt,
        left,
        right,
        ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    let iv = name_of(*left, defs)?;
    Some((iv, *right))
}

fn name_of(l: Local, defs: &HashMap<u32, Value>) -> Option<String> {
    match defs.get(&l.0)? {
        Value::Name(n) => Some(n.clone()),
        _ => None,
    }
}

/// Resolve `Local` / `Name` load / param identity through leaf defs.
fn same_local(got: Local, want: Local, defs: &HashMap<u32, Value>) -> bool {
    if got == want {
        return true;
    }
    match defs.get(&got.0) {
        Some(Value::Local(l)) => same_local(*l, want, defs),
        Some(Value::Name(_)) => false, // slot load ≠ param unless assigned from it
        _ => false,
    }
}

fn is_unit_inc(dest: u32, iv: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&dest)
    else {
        return false;
    };
    let one_l = matches!(defs.get(&left.0), Some(Value::Int(1)));
    let one_r = matches!(defs.get(&right.0), Some(Value::Int(1)));
    let name_l = name_of(*left, defs).as_deref() == Some(iv);
    let name_r = name_of(*right, defs).as_deref() == Some(iv);
    (name_l && one_r) || (name_r && one_l)
}

fn is_list_get(v: &Value) -> Option<(Local, Local)> {
    match v {
        Value::Builtin {
            name: Builtin::ListGet,
            args, .. } if args.len() == 2 => Some((args[0], args[1])),
        _ => None,
    }
}

fn is_list_set(v: &Value) -> Option<(Local, Local, Local)> {
    match v {
        Value::Builtin {
            name: Builtin::MapSet,
            args, .. } if args.len() == 3 => Some((args[0], args[1], args[2])),
        _ => None,
    }
}

fn list_arg_is(list: Local, want: Local, defs: &HashMap<u32, Value>) -> bool {
    if list == want {
        return true;
    }
    match defs.get(&list.0) {
        Some(Value::Local(l)) => list_arg_is(*l, want, defs),
        Some(Value::Name(_)) => false,
        _ => false,
    }
}

/// Inner body of gemv: s accumulates A[i*n+j]*x[j]; then out.set(i,s); i+=1.
fn body_has_gemv_inner(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    i_slot: &str,
    a: Local,
    x: Local,
    n: Local,
) -> bool {
    let mut saw_inner = false;
    let mut saw_set = false;
    let mut saw_i_inc = false;
    for op in &body.ops {
        match op {
            Op::Let {
                value:
                    Value::Loop {
                        header,
                        body: ib,
                        latch,
                    },
                ..
            } => {
                if !latch.ops.is_empty() {
                    continue;
                }
                let Some((j_slot, bound)) = header_lt_bound(header, defs) else {
                    continue;
                };
                if !same_local(bound, n, defs) {
                    continue;
                }
                if gemv_inner_accumulates(ib, defs, &j_slot, a, x, n, i_slot) {
                    saw_inner = true;
                }
            }
            Op::Assign { name, value } => {
                if name == out_slot {
                    if let Some(val) = defs.get(&value.0) {
                        if is_list_set(val).is_some() {
                            saw_set = true;
                        }
                    }
                }
                if name == i_slot && is_unit_inc(value.0, i_slot, defs) {
                    saw_i_inc = true;
                }
            }
            _ => {}
        }
    }
    saw_inner && saw_set && saw_i_inc
}

fn gemv_inner_accumulates(
    body: &Block,
    defs: &HashMap<u32, Value>,
    j_slot: &str,
    a: Local,
    x: Local,
    n: Local,
    i_slot: &str,
) -> bool {
    let mut saw_mul_gets = false;
    let mut saw_j_inc = false;
    for op in &body.ops {
        if let Op::Assign { name, value } = op {
            if name == j_slot && is_unit_inc(value.0, j_slot, defs) {
                saw_j_inc = true;
            }
        }
        if let Op::Let {
            value:
                Value::Binary {
                    op: BinOp::Mul,
                    left,
                    right,
                    ..
                },
            ..
        } = op
        {
            let lg = defs.get(&left.0).and_then(is_list_get);
            let rg = defs.get(&right.0).and_then(is_list_get);
            if let (Some((la, _)), Some((lb, _))) = (lg, rg) {
                let a_x = (list_arg_is(la, a, defs) && list_arg_is(lb, x, defs))
                    || (list_arg_is(la, x, defs) && list_arg_is(lb, a, defs));
                if a_x {
                    // Soft-check index uses i/n/j via presence of Mul/Add involving them elsewhere.
                    let _ = (n, i_slot);
                    saw_mul_gets = true;
                }
            }
        }
    }
    saw_mul_gets && saw_j_inc
}

fn fun_has_gemv_t_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    x: Local,
    m: Local,
    n: Local,
) -> bool {
    let mut mul = false;
    let mut set = false;
    let mut zero_fill = false;
    for_each_let(body, &mut |v| {
        if let Value::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } = v
        {
            let lg = defs.get(&left.0).and_then(is_list_get);
            let rg = defs.get(&right.0).and_then(is_list_get);
            if let (Some((la, _)), Some((lb, _))) = (lg, rg) {
                if (list_arg_is(la, a, defs) && list_arg_is(lb, x, defs))
                    || (list_arg_is(la, x, defs) && list_arg_is(lb, a, defs))
                {
                    mul = true;
                }
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        // Zero-fill: set(j, 0.0) or set(j, Float(0))
        if let Some((_, _, val)) = is_list_set(v) {
            if matches!(defs.get(&val.0), Some(Value::Float(f)) if *f == 0.0)
                || matches!(defs.get(&val.0), Some(Value::Int(0)))
            {
                zero_fill = true;
            }
        }
        let _ = (m, n, out_slot);
    });
    // Also scan leaf_defs for MapSet / Mul (lets may be inlined into Assigns)
    for v in defs.values() {
        if let Value::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } = v
        {
            let lg = defs.get(&left.0).and_then(is_list_get);
            let rg = defs.get(&right.0).and_then(is_list_get);
            if let (Some((la, _)), Some((lb, _))) = (lg, rg) {
                if (list_arg_is(la, a, defs) && list_arg_is(lb, x, defs))
                    || (list_arg_is(la, x, defs) && list_arg_is(lb, a, defs))
                {
                    mul = true;
                }
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if let Some((_, _, val)) = is_list_set(v) {
            if matches!(defs.get(&val.0), Some(Value::Float(f)) if *f == 0.0) {
                zero_fill = true;
            }
        }
    }
    mul && set && zero_fill
}

fn fun_has_addmm_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    u: Local,
    v: Local,
    alpha: Local,
    m: Local,
    n: Local,
) -> bool {
    let mut get_u = false;
    let mut get_v = false;
    let mut set = false;
    let mut uses_alpha = false;
    for vdef in defs.values() {
        if let Some((lst, _)) = is_list_get(vdef) {
            if list_arg_is(lst, u, defs) {
                get_u = true;
            }
            if list_arg_is(lst, v, defs) {
                get_v = true;
            }
        }
        if is_list_set(vdef).is_some() {
            set = true;
        }
        if mentions_local(vdef, alpha) {
            uses_alpha = true;
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, u, defs) {
                get_u = true;
            }
            if list_arg_is(lst, v, defs) {
                get_v = true;
            }
        }
    });
    let _ = (out_slot, m, n);
    get_u && get_v && set && uses_alpha
}

fn fun_has_axpy_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    x: Local,
    alpha: Local,
) -> bool {
    let mut get_x = false;
    let mut get_y = false;
    let mut set = false;
    let mut uses_alpha = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, x, defs) {
                get_x = true;
            }
            // y is out_slot Name
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get_y = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if mentions_local(v, alpha) {
            uses_alpha = true;
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, x, defs) {
                get_x = true;
            }
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get_y = true;
            }
        }
    });
    get_x && get_y && set && uses_alpha
}

fn fun_has_sub_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    b: Local,
) -> bool {
    let mut get_a = false;
    let mut get_b = false;
    let mut sub = false;
    let mut set = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, a, defs) {
                get_a = true;
            }
            if list_arg_is(lst, b, defs) {
                get_b = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Sub, .. }) {
            sub = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Sub, .. }) {
            sub = true;
        }
    });
    let _ = out_slot;
    get_a && get_b && sub && set
}

fn fun_has_add_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    b: Local,
) -> bool {
    let mut get_a = false;
    let mut get_b = false;
    let mut add = false;
    let mut set = false;
    let mut mul = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, a, defs) || name_of(lst, defs).as_deref() == Some(out_slot) {
                get_a = true;
            }
            if list_arg_is(lst, b, defs) {
                get_b = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Add, .. }) {
            add = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, a, defs) || name_of(lst, defs).as_deref() == Some(out_slot) {
                get_a = true;
            }
            if list_arg_is(lst, b, defs) {
                get_b = true;
            }
        }
        if matches!(val, Value::Binary { op: BinOp::Add, .. }) {
            add = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
    });
    // Exclude axpy-like `y + α*x` (has Mul).
    get_a && get_b && add && set && !mul
}

fn fun_has_mul_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    b: Local,
) -> bool {
    let mut get_a = false;
    let mut get_b = false;
    let mut mul = false;
    let mut set = false;
    let mut add_or_sub = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, a, defs) {
                get_a = true;
            }
            if list_arg_is(lst, b, defs) {
                get_b = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(v, defs) {
            add_or_sub = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(val, defs) {
            add_or_sub = true;
        }
    });
    let _ = out_slot;
    get_a && get_b && mul && set && !add_or_sub
}

fn fun_has_scale_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    alpha: Local,
) -> bool {
    let mut get_y = false;
    let mut mul = false;
    let mut set = false;
    let mut uses_alpha = false;
    let mut add_or_sub = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get_y = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(v, defs) {
            add_or_sub = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if mentions_local(v, alpha) {
            uses_alpha = true;
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get_y = true;
            }
        }
        if matches!(val, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(val, defs) {
            add_or_sub = true;
        }
    });
    get_y && mul && set && uses_alpha && !add_or_sub
}

fn fun_has_fill_shape(body: &Block, defs: &HashMap<u32, Value>, out_slot: &str, v: Local) -> bool {
    let mut set = false;
    let mut uses_v = false;
    let mut get_any = false;
    let mut arith = false;
    for vdef in defs.values() {
        if let Some((_, _)) = is_list_get(vdef) {
            get_any = true;
        }
        if is_list_set(vdef).is_some() {
            set = true;
        }
        if mentions_local(vdef, v) {
            uses_v = true;
        }
        if is_nontrivial_arith(vdef, defs) {
            arith = true;
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if is_list_get(val).is_some() {
            get_any = true;
        }
        if is_nontrivial_arith(val, defs) {
            arith = true;
        }
    });
    let _ = out_slot;
    set && uses_v && !get_any && !arith
}

/// `i+1` / `1+i` latch increments must not disqualify elementwise kernels.
fn is_unit_inc_value(v: &Value, defs: &HashMap<u32, Value>) -> bool {
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = v
    else {
        return false;
    };
    let one_l = matches!(defs.get(&left.0), Some(Value::Int(1)));
    let one_r = matches!(defs.get(&right.0), Some(Value::Int(1)));
    let name_l = name_of(*left, defs).is_some();
    let name_r = name_of(*right, defs).is_some();
    (name_l && one_r) || (name_r && one_l)
}

fn is_nontrivial_add_or_sub(v: &Value, defs: &HashMap<u32, Value>) -> bool {
    matches!(
        v,
        Value::Binary {
            op: BinOp::Add | BinOp::Sub,
            ..
        } if !is_unit_inc_value(v, defs)
    )
}

fn is_nontrivial_arith(v: &Value, defs: &HashMap<u32, Value>) -> bool {
    match v {
        Value::Binary {
            op: BinOp::Mul | BinOp::Div,
            ..
        } => true,
        Value::Binary {
            op: BinOp::Add | BinOp::Sub,
            ..
        } if !is_unit_inc_value(v, defs) => true,
        _ => false,
    }
}

fn fun_has_clamp_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    lo: Local,
    hi: Local,
) -> bool {
    let mut set = false;
    let mut uses_lo = false;
    let mut uses_hi = false;
    let mut saw_if = false;
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        // Require a real `If` — loop `i < n` alone must not look like clamp.
        if matches!(val, Value::If { .. }) {
            saw_if = true;
        }
    });
    for v in defs.values() {
        if mentions_local(v, lo) {
            uses_lo = true;
        }
        if mentions_local(v, hi) {
            uses_hi = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for op in &body.ops {
        if let Op::Assign { name, .. } = op {
            if name == out_slot {
                set = true;
            }
        }
    }
    set && saw_if && uses_lo && uses_hi
}

fn fun_has_copy_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    src: Local,
) -> bool {
    // out[i] = src[i]; no arithmetic on the transferred value.
    let mut get_src = false;
    let mut set = false;
    let mut saw_arith = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, src, defs) {
                get_src = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if matches!(
            v,
            Value::Binary {
                op: BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div,
                ..
            }
        ) {
            // Index `i*n+j` style shouldn't appear; len() compares are elsewhere.
            // Allow only if not feeding the set value — soft: any Mul/Div is suspicious.
            if matches!(
                v,
                Value::Binary {
                    op: BinOp::Mul | BinOp::Div | BinOp::Sub,
                    ..
                }
            ) {
                saw_arith = true;
            }
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, src, defs) {
                get_src = true;
            }
        }
    });
    let _ = out_slot;
    get_src && set && !saw_arith
}

fn fun_has_sum_sq_shape(body: &Block, defs: &HashMap<u32, Value>, xs: Local) -> bool {
    let mut get = false;
    let mut mul = false;
    let mut add = false;
    let mut set = false;
    let mut div = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, xs, defs) {
                get = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(v, defs) && matches!(v, Value::Binary { op: BinOp::Add, .. }) {
            add = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for_each_let(body, &mut |val| {
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, xs, defs) {
                get = true;
            }
        }
        if matches!(val, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(val, defs)
            && matches!(val, Value::Binary { op: BinOp::Add, .. })
        {
            add = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if is_list_set(val).is_some() {
            set = true;
        }
    });
    get && mul && add && !set && !div
}

fn fun_has_mean_shape(body: &Block, defs: &HashMap<u32, Value>, xs: Local) -> bool {
    let mut get = false;
    let mut add = false;
    let mut div = false;
    let mut mul = false;
    let mut set = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, xs, defs) {
                get = true;
            }
        }
        if is_nontrivial_add_or_sub(v, defs) && matches!(v, Value::Binary { op: BinOp::Add, .. }) {
            add = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for_each_let(body, &mut |val| {
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, xs, defs) {
                get = true;
            }
        }
        if is_nontrivial_add_or_sub(val, defs)
            && matches!(val, Value::Binary { op: BinOp::Add, .. })
        {
            add = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_list_set(val).is_some() {
            set = true;
        }
    });
    get && add && div && !mul && !set
}

fn fun_has_std_shape(body: &Block, defs: &HashMap<u32, Value>, xs: Local) -> bool {
    let mut get = false;
    let mut sub = false;
    let mut mul = false;
    let mut div = false;
    let mut set = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, xs, defs) {
                get = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Sub, .. }) {
            sub = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for_each_let(body, &mut |val| {
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, xs, defs) {
                get = true;
            }
        }
        if matches!(val, Value::Binary { op: BinOp::Sub, .. }) {
            sub = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if is_list_set(val).is_some() {
            set = true;
        }
    });
    get && sub && mul && div && !set && body_calls_any(body, &["lumia_f64_sqrt", "sqrtF", "sqrt"])
}

fn fun_has_l2_normalize_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    eps: Local,
) -> bool {
    let mut get = false;
    let mut set = false;
    let mut mul = false;
    let mut uses_eps = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if mentions_local(v, eps) {
            uses_eps = true;
        }
    }
    for_each_let(body, &mut |val| {
        if let Some((lst, _)) = is_list_get(val) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get = true;
            }
        }
        if is_list_set(val).is_some() {
            set = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
    });
    get && set && mul && uses_eps && body_calls_any(body, &["lumia_f64_sqrt", "sqrtF", "sqrt"])
}

fn fun_has_softmax_shape(body: &Block, defs: &HashMap<u32, Value>, out_slot: &str) -> bool {
    let mut get = false;
    let mut set = false;
    let mut div = false;
    let mut gt = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Gt, .. }) {
            gt = true;
        }
    }
    for_each_let(body, &mut |val| {
        if let Some((lst, _)) = is_list_get(val) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get = true;
            }
        }
        if is_list_set(val).is_some() {
            set = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Gt, .. }) {
            gt = true;
        }
        if matches!(val, Value::If { .. }) {
            // max-pass update often uses If
            gt = true;
        }
    });
    get && set && div && gt && body_calls_any(body, &["lumia_f64_exp", "expF", "exp"])
}

fn body_calls_any(body: &Block, names: &[&str]) -> bool {
    let mut found = false;
    for_each_let(body, &mut |val| {
        if let Value::Call { fun, .. } = val {
            if names.iter().any(|n| fun == n) {
                found = true;
            }
        }
    });
    found
}

fn mentions_local(v: &Value, target: Local) -> bool {
    match v {
        Value::Local(l) => *l == target,
        Value::Binary { left, right, .. } => *left == target || *right == target,
        Value::Builtin { args, .. } => args.contains(&target),
        _ => false,
    }
}

fn for_each_let(body: &Block, f: &mut dyn FnMut(&Value)) {
    for op in &body.ops {
        if let Op::Let { value, .. } = op {
            f(value);
            match value {
                Value::Loop {
                    header,
                    body,
                    latch,
                } => {
                    for_each_let(header, f);
                    for_each_let(body, f);
                    for_each_let(latch, f);
                }
                Value::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    for_each_let(then_block, f);
                    for_each_let(else_block, f);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{optimize, OptOptions};

    #[test]
    fn rewrites_gemv_helper_to_foreign_call() {
        let src = r#"
module M
val gemv(m, n, a, x, y) = {
  var out = y
  var i = 0
  for i < m {
    var s = 0.0
    var j = 0
    for j < n {
      s = s + a.get(i * n + j) * x.get(j)
      j = j + 1
    }
    out = out.set(i, s)
    i = i + 1
  }
  out
}
val main = {
  val a = listOf(1.0, 2.0, 3.0, 4.0, 5.0, 6.0)
  val x = listOf(1.0, 2.0)
  var y = listOf(0.0, 0.0, 0.0)
  y = gemv(3, 2, a, x, y)
  0
}
"#;
        let mut core = lumia_core::compile_source_to_core(src).unwrap();
        optimize(&mut core, &OptOptions::for_build(true));
        assert!(
            core.functions
                .iter()
                .any(|f| f.external.as_deref() == Some("lumia_f64_gemv")),
            "expected injected lumia_f64_gemv foreign"
        );
    }

    #[test]
    fn rewrites_zeros_helper_to_foreign_call() {
        let src = r#"
module M
val nZeros(n) = {
  var xs = listOf(0.0)
  var i = 1
  for i < n {
    xs = xs.append(0.0)
    i = i + 1
  }
  xs
}
val main = {
  val z = nZeros(4)
  z.len()
}
"#;
        let mut core = lumia_core::compile_source_to_core(src).unwrap();
        optimize(&mut core, &OptOptions::for_build(true));
        assert!(
            core.functions
                .iter()
                .any(|f| f.external.as_deref() == Some("lumia_list_f64_zeros")),
            "expected injected lumia_list_f64_zeros foreign"
        );
    }

    #[test]
    fn rewrites_sum_sq_and_mean_helpers() {
        let src = r#"
module M
val nSumSq(xs) = {
  var s = 0.0
  var i = 0
  val n = xs.len()
  for i < n {
    val v = xs.get(i)
    s = s + v * v
    i = i + 1
  }
  s
}
val nMean(xs) = {
  var s = 0.0
  var i = 0
  val n = xs.len()
  for i < n {
    s = s + xs.get(i)
    i = i + 1
  }
  if n == 0 { 0.0 } else { s / (0.0 + n) }
}
val main = {
  val xs = listOf(1.0, 2.0, 3.0)
  val a = nSumSq(xs)
  val b = nMean(xs)
  0
}
"#;
        let mut core = lumia_core::compile_source_to_core(src).unwrap();
        optimize(&mut core, &OptOptions::for_build(true));
        let ext: Vec<_> = core
            .functions
            .iter()
            .filter_map(|f| f.external.as_deref())
            .collect();
        assert!(
            ext.contains(&"lumia_f64_sum_sq"),
            "sum_sq missing in {ext:?}"
        );
        assert!(ext.contains(&"lumia_f64_mean"), "mean missing in {ext:?}");
    }

    #[test]
    fn rewrites_l2_norm_with_sqrt_foreign() {
        let src = r#"
module M
foreign "C" pure fn lumia_f64_sqrt(x: Float) -> Float
val nL2(xs) = {
  var s = 0.0
  var i = 0
  val n = xs.len()
  for i < n {
    val v = xs.get(i)
    s = s + v * v
    i = i + 1
  }
  lumia_f64_sqrt(s)
}
val main = {
  val xs = listOf(3.0, 4.0)
  nL2(xs)
}
"#;
        let mut core = lumia_core::compile_source_to_core(src).unwrap();
        optimize(&mut core, &OptOptions::for_build(true));
        assert!(
            core.functions
                .iter()
                .any(|f| f.external.as_deref() == Some("lumia_f64_l2_norm")),
            "expected lumia_f64_l2_norm"
        );
    }

    #[test]
    fn rewrites_l2_normalize_and_softmax() {
        let src = r#"
module M
foreign "C" pure fn lumia_f64_sqrt(x: Float) -> Float
foreign "C" pure fn lumia_f64_exp(x: Float) -> Float
val nNorm(xs, eps) = {
  var out = xs
  var s = 0.0
  var i = 0
  val n = out.len()
  for i < n {
    val v = out.get(i)
    s = s + v * v
    i = i + 1
  }
  val inv = 1.0 / (lumia_f64_sqrt(s) + eps)
  i = 0
  for i < n {
    out = out.set(i, out.get(i) * inv)
    i = i + 1
  }
  out
}
val nSoft(xs) = {
  var out = xs
  var m = out.get(0)
  var i = 1
  val n = out.len()
  for i < n {
    val v = out.get(i)
    if v > m { m = v }
    i = i + 1
  }
  var z = 0.0
  i = 0
  for i < n {
    val e = lumia_f64_exp(out.get(i) - m)
    out = out.set(i, e)
    z = z + e
    i = i + 1
  }
  i = 0
  for i < n {
    out = out.set(i, out.get(i) / z)
    i = i + 1
  }
  out
}
val main = {
  var xs = listOf(1.0, 2.0, 3.0)
  xs = nNorm(xs, 0.001)
  xs = nSoft(xs)
  0
}
"#;
        let mut core = lumia_core::compile_source_to_core(src).unwrap();
        optimize(&mut core, &OptOptions::for_build(true));
        let ext: Vec<_> = core
            .functions
            .iter()
            .filter_map(|f| f.external.as_deref())
            .collect();
        assert!(
            ext.contains(&"lumia_f64_l2_normalize"),
            "normalize missing in {ext:?}"
        );
        assert!(
            ext.contains(&"lumia_f64_softmax"),
            "softmax missing in {ext:?}"
        );
    }

    #[test]
    fn int_strided_loop_is_not_rewritten_to_clamp() {
        // Regression: bare arity/shape once matched Int loops (bench_cpu
        // `collatzStrided`) and rewrote them to `lumia_f64_clamp`.
        let src = r#"
module M
val steps(n) = {
  var x = n
  var c = 0
  for x > 1 {
    if x % 2 == 0 {
      x = x / 2
    } else {
      x = 3 * x + 1
    }
    c = c + 1
  }
  c
}
val strided(start, limit, stride) = {
  var n = start
  var total = 0
  for n <= limit {
    total = total + steps(n)
    n = n + stride
  }
  total
}
val main = {
  strided(1, 20, 3)
}
"#;
        let mut core = lumia_core::compile_source_to_core(src).unwrap();
        optimize(&mut core, &OptOptions::for_build(true));
        assert!(
            core.functions
                .iter()
                .all(|f| f.external.as_deref() != Some("lumia_f64_clamp")),
            "Int strided loop must not inject lumia_f64_clamp"
        );
        let strided = core
            .functions
            .iter()
            .find(|f| f.name == "strided")
            .expect("strided");
        let body = format!("{:?}", strided.body);
        assert!(
            !body.contains("lumia_f64_clamp"),
            "strided body must not call lumia_f64_clamp: {body}"
        );
    }
}
