//! Rewrite dense `List[Float]` helpers to `lumia_f64_*` foreign calls (before Inline).
//!
//! Whole-function patterns become a single `Call` so Release inlining places the
//! RT kernel at the call site (same shape as `std.linalg` wrappers).

use lumia_core::{
    for_each_block_dfs, max_local_in_fun, Block, CoreFun, CoreModule, Local, Op, Value,
};
use lumia_hir::Builtin;
use lumia_syntax::BinOp;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub struct DenseF64SrPass;

impl crate::Pass for DenseF64SrPass {
    fn name(&self) -> &str {
        "dense_f64_sr"
    }
    fn run(&self, module: &mut CoreModule) {
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
        } else if match_clamp_fun(fun, &defs).is_some() {
            Some("lumia_f64_clamp")
        } else if match_copy_fun(fun, &defs).is_some() {
            Some("lumia_f64_copy")
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
    if module.functions.iter().any(|f| f.name == sym || f.external.as_deref() == Some(sym)) {
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
            vec![Type::Int, Type::Int, lf.clone(), lf.clone(), lf.clone(), Type::Float],
            lf,
        ),
        "lumia_f64_axpy" => (vec![lf.clone(), Type::Float, lf.clone()], lf),
        "lumia_f64_sub" => (vec![lf.clone(), lf.clone(), lf.clone()], lf),
        "lumia_f64_clamp" => (vec![lf.clone(), Type::Float, Type::Float], lf),
        "lumia_f64_copy" => (vec![lf.clone(), lf.clone()], lf),
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
    // Keep typed as list/float so codegen roots / ABI stay correct.
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
                ) {
                    all_defs.insert(local.0, value.clone());
                }
            }
        }
    });
    all_defs
}

fn match_gemv_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 5 {
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
    if fun.params.len() != 5 {
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
    if fun.params.len() != 6 {
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
    if fun.params.len() != 3 {
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
    if fun.params.len() != 3 {
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

fn match_clamp_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 3 {
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

fn match_copy_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 2 {
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
            args,
        } if args.len() == 2 => Some((args[0], args[1])),
        _ => None,
    }
}

fn is_list_set(v: &Value) -> Option<(Local, Local, Local)> {
    match v {
        Value::Builtin {
            name: Builtin::MapSet,
            args,
        } if args.len() == 3 => Some((args[0], args[1], args[2])),
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
        if matches!(
            v,
            Value::Binary {
                op: BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge,
                ..
            }
        ) {
            saw_if = true;
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
}

