//! Shared chase helpers for heap typing and HOF/callee result ABI.

use crate::find_local_def;
use crate::find_top_level_local_def;
use crate::ir::{Block, Local, Value};
use crate::value_ty::{fold_slot_assign_ty, JoinAssignKind};
use lumia_syntax::Sym;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Heap type of a mutable/immutable slot (`Name` / `Assign`), joining all writes.
pub(super) fn slot_heap_ty(
    block: &Block,
    name: &Sym,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    seen_slots: &mut HashSet<Sym>,
) -> Option<Type> {
    if !seen_slots.insert(name.clone()) {
        return None;
    }
    let mut acc: Option<Type> = None;
    collect_slot_assigns(
        block,
        block,
        name,
        float_locals,
        fun_ret_tys,
        fun_param_tys,
        cap_tys,
        seen,
        seen_slots,
        &mut acc,
    );
    acc
}

pub(super) fn collect_slot_assigns(
    walk: &Block,
    defs_root: &Block,
    name: &Sym,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    seen_slots: &mut HashSet<Sym>,
    acc: &mut Option<Type>,
) {
    crate::for_each_named_slot_assign_in_block(walk, name, &mut |Local(src)| {
        if let Some(t) = super::local_heap::local_heap_ty(
            defs_root,
            src,
            float_locals,
            fun_ret_tys,
            fun_param_tys,
            cap_tys,
            seen,
            seen_slots,
        ) {
            fold_slot_assign_ty(acc, t, JoinAssignKind::Heap);
        }
    });
}

pub(super) fn stamped_abi_is_authoritative(ty: &Type) -> bool {
    !matches!(ty, Type::Int | Type::Var(_)) && abi_ty_is_ground(ty)
}

pub(super) fn abi_ty_is_ground(t: &Type) -> bool {
    match t {
        Type::Var(_) => false,
        Type::Fun(ps, r, _) => ps.iter().all(abi_ty_is_ground) && abi_ty_is_ground(r),
        Type::List(e) | Type::Set(e) | Type::Task(e) | Type::Channel(e) => abi_ty_is_ground(e),
        Type::Map(k, v) => abi_ty_is_ground(k) && abi_ty_is_ground(v),
        Type::Tuple(ts) | Type::TuplePrefix(ts) | Type::Adt { params: ts, .. } => {
            ts.iter().all(abi_ty_is_ground)
        }
        _ => true,
    }
}

pub(super) fn fun_ret_of_local(
    block: &Block,
    id: u32,
    fun_ret_tys: &HashMap<Sym, Type>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    match find_local_def(block, id)? {
        Value::Local(Local(src)) => fun_ret_of_local(block, *src, fun_ret_tys, seen),
        Value::FunRef(name) | Value::AllocClosure { fun: name, .. } => {
            fun_ret_tys.get(name.as_str()).cloned()
        }
        _ => None,
    }
}

pub(super) fn alloc_elems_ty(
    block: &Block,
    elems: &[Local],
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
    cap_tys: &HashMap<u32, Type>,
    seen: &mut HashSet<u32>,
    seen_slots: &mut HashSet<Sym>,
) -> Type {
    if elems.is_empty() {
        return Type::Int;
    }
    if elems.iter().all(|e| float_locals.contains(&e.0)) {
        return Type::Float;
    }
    let mut acc: Option<Type> = None;
    for e in elems {
        let t = if float_locals.contains(&e.0) {
            Type::Float
        } else {
            super::local_heap::local_heap_ty(
                block,
                e.0,
                float_locals,
                fun_ret_tys,
                fun_param_tys,
                cap_tys,
                seen,
                seen_slots,
            )
            .unwrap_or(Type::Int)
        };
        fold_slot_assign_ty(&mut acc, t, JoinAssignKind::Heap);
    }
    acc.unwrap_or(Type::Int)
}

/// Concrete return type from a body's result `Call` / alias chain (post-lower /
/// post-mono callee tables), so `spawn { dbl(1.5) }.join()` keeps Float ABI.
pub(crate) fn block_result_callee_ty(
    block: &Block,
    fun_ret_tys: &HashMap<Sym, Type>,
) -> Option<Type> {
    let Local(r) = block.result?;
    local_callee_ty(block, r, fun_ret_tys, &mut HashSet::default())
}

/// `icall` of a `ClosureCap` whose outer capture is a known FunRef/closure.
/// Covers `spawn { dbl(1.5) }` when `dbl` is a local lambda (env capture).
pub(crate) fn block_result_icall_cap_ty(
    block: &Block,
    cap_srcs: &[Local],
    funref_locals: &HashMap<u32, Sym>,
    fun_ret_tys: &HashMap<Sym, Type>,
) -> Option<Type> {
    let Local(r) = block.result?;
    local_icall_cap_ty(
        block,
        r,
        cap_srcs,
        funref_locals,
        fun_ret_tys,
        &mut HashSet::default(),
    )
}

/// Resolve `IndirectCall` → `ClosureCap(index)` → captured fun name → ret.
pub(crate) fn block_result_icall_cap_ty_by_index(
    block: &Block,
    cap_funs: &HashMap<u32, Sym>,
    fun_ret_tys: &HashMap<Sym, Type>,
) -> Option<Type> {
    let Local(r) = block.result?;
    local_icall_cap_ty_by_index(block, r, cap_funs, fun_ret_tys, &mut HashSet::default())
}

/// Body result is `FunRef` / `AllocClosure` — keep a `Fun` ret so
/// `spawn { { x -> x * 2.0 } }.join()(1.5)` uses Float icall ABI.
pub(crate) fn block_result_fun_ty(
    block: &Block,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
) -> Option<Type> {
    let Local(r) = block.result?;
    local_fun_ty(
        block,
        r,
        fun_ret_tys,
        fun_param_tys,
        &mut HashSet::default(),
    )
}

/// Known HOF shapes for spawn/icall Float ABI (apply / compose / id).
#[derive(Default, Clone)]
pub(crate) struct HofSets {
    pub apply: HashSet<String>,
    pub compose: HashSet<String>,
    pub id: HashSet<String>,
}

impl HofSets {
    pub(crate) fn note(&mut self, name: &str, params: &[Local], body: &Block) {
        if is_apply_hof(params, body) {
            self.apply.insert(name.to_string());
        }
        if is_compose_hof(params, body) {
            self.compose.insert(name.to_string());
        }
        if is_id_hof(params, body) {
            self.id.insert(name.to_string());
        }
    }

    pub(crate) fn from_module_funs<'a, I>(funs: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a [Local], &'a Block)>,
    {
        let mut h = Self::default();
        for (name, params, body) in funs {
            h.note(name, params, body);
        }
        h
    }
}

/// `{ f, x -> f(x) }` / `{ f, g, x -> g(f(x)) }` pipeline HOFs.
pub(crate) fn block_result_known_hof_ty(
    block: &Block,
    hof: &HofSets,
    fun_ret_tys: &HashMap<Sym, Type>,
    cap_funs: Option<&HashMap<u32, Sym>>,
) -> Option<Type> {
    let Local(r) = block.result?;
    local_known_hof_ty(
        block,
        r,
        hof,
        fun_ret_tys,
        cap_funs,
        &mut HashSet::default(),
    )
}

/// True when the body is exactly `icall f(args…)` with `f`/`args` = formals
/// (optional leading env param for lifted closures).
pub(crate) fn is_apply_hof(params: &[Local], body: &Block) -> bool {
    if params.len() < 2 {
        return false;
    }
    let Some(Local(r)) = body.result else {
        return false;
    };
    let Some((callee, args)) = resolve_icall(body, r) else {
        return false;
    };
    // Nullary-env: params[0] is the fun, params[1..] are args.
    if local_aliases(body, callee, params[0].0)
        && args.len() == params.len() - 1
        && args
            .iter()
            .zip(params[1..].iter())
            .all(|(a, p)| local_aliases(body, *a, p.0))
    {
        return true;
    }
    // Env closure: params[0]=env, params[1]=fun, params[2..]=args.
    if params.len() >= 3
        && local_aliases(body, callee, params[1].0)
        && args.len() == params.len() - 2
        && args
            .iter()
            .zip(params[2..].iter())
            .all(|(a, p)| local_aliases(body, *a, p.0))
    {
        return true;
    }
    false
}

/// `{ f, g, x -> g(f(x)) }` (optional leading env).
pub(crate) fn is_compose_hof(params: &[Local], body: &Block) -> bool {
    let (f, g, x) = match params.len() {
        3 => (params[0], params[1], params[2]),
        4 => (params[1], params[2], params[3]),
        _ => return false,
    };
    let Some(Local(r)) = body.result else {
        return false;
    };
    let Some((g_cal, g_args)) = resolve_icall(body, r) else {
        return false;
    };
    if g_args.len() != 1 || !local_aliases(body, g_cal, g.0) {
        return false;
    }
    let Some((f_cal, f_args)) = resolve_icall(body, g_args[0]) else {
        return false;
    };
    f_args.len() == 1 && local_aliases(body, f_cal, f.0) && local_aliases(body, f_args[0], x.0)
}

/// `{ f -> f }` identity (optional leading env).
pub(crate) fn is_id_hof(params: &[Local], body: &Block) -> bool {
    let p = match params.len() {
        1 => params[0],
        2 => params[1],
        _ => return false,
    };
    let Some(Local(r)) = body.result else {
        return false;
    };
    local_aliases(body, r, p.0)
}

pub(super) fn resolve_icall(block: &Block, id: u32) -> Option<(u32, Vec<u32>)> {
    let mut seen = HashSet::default();
    let mut cur = id;
    loop {
        if !seen.insert(cur) {
            return None;
        }
        match find_top_level_local_def(block, cur)? {
            Value::Local(Local(src)) => cur = *src,
            Value::IndirectCall { callee, args } => {
                return Some((callee.0, args.iter().map(|a| a.0).collect()));
            }
            _ => return None,
        }
    }
}

pub(super) fn ret_ty_from_callee_table(t: &Type) -> Option<Type> {
    match t {
        // `List[Int]` is the lift may-heap placeholder — not a real payload type.
        Type::List(e) if matches!(e.as_ref(), Type::Int) => None,
        Type::Float
        | Type::String
        | Type::Char
        | Type::List(_)
        | Type::Map(_, _)
        | Type::Set(_)
        | Type::Adt { .. }
        | Type::Task(_)
        | Type::Channel(_)
        | Type::Fun(_, _, _) => Some(t.clone()),
        _ => None,
    }
}

pub(super) fn local_aliases(block: &Block, id: u32, target: u32) -> bool {
    let mut seen = HashSet::default();
    let mut cur = id;
    loop {
        if cur == target {
            return true;
        }
        if !seen.insert(cur) {
            return false;
        }
        match find_top_level_local_def(block, cur) {
            Some(Value::Local(Local(src))) => cur = *src,
            _ => return false,
        }
    }
}

pub(super) fn local_fun_ty(
    block: &Block,
    id: u32,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    match find_top_level_local_def(block, id)? {
        Value::Local(Local(src)) => local_fun_ty(block, *src, fun_ret_tys, fun_param_tys, seen),
        Value::FunRef(name) => {
            let ret = fun_ret_tys.get(name.as_str())?.clone();
            let params = fun_param_tys
                .get(name.as_str())
                .cloned()
                .unwrap_or_default();
            Some(Type::Fun(params, Box::new(ret), Effect::pure()))
        }
        Value::AllocClosure { fun, .. } => {
            let ret = fun_ret_tys.get(fun.as_str())?.clone();
            // Drop env pointer param for the user-facing Fun type.
            let params = fun_param_tys.get(fun.as_str()).cloned().unwrap_or_default();
            let params = if params.len() > 1 {
                params[1..].to_vec()
            } else {
                Vec::new()
            };
            Some(Type::Fun(params, Box::new(ret), Effect::pure()))
        }
        _ => None,
    }
}

pub(super) fn local_known_hof_ty(
    block: &Block,
    id: u32,
    hof: &HofSets,
    fun_ret_tys: &HashMap<Sym, Type>,
    cap_funs: Option<&HashMap<u32, Sym>>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    match find_top_level_local_def(block, id)? {
        Value::Local(Local(src)) => {
            local_known_hof_ty(block, *src, hof, fun_ret_tys, cap_funs, seen)
        }
        Value::IndirectCall { callee, args } => {
            let cal = resolve_fun_name(block, callee.0, cap_funs, hof)?;
            if hof.apply.contains(cal.as_str()) {
                let farg = args.first()?;
                let fname = resolve_fun_name(block, farg.0, cap_funs, hof)?;
                return fun_ret_tys.get(fname.as_str()).and_then(ret_ty_from_callee_table);
            }
            if hof.compose.contains(cal.as_str()) && args.len() >= 2 {
                // andThen(f, g, x): result type is g's return.
                let g_arg = &args[args.len() - 2];
                let fname = resolve_fun_name(block, g_arg.0, cap_funs, hof)?;
                return fun_ret_tys.get(fname.as_str()).and_then(ret_ty_from_callee_table);
            }
            None
        }
        _ => None,
    }
}

pub(super) fn resolve_fun_name(
    block: &Block,
    id: u32,
    cap_funs: Option<&HashMap<u32, Sym>>,
    hof: &HofSets,
) -> Option<Sym> {
    let mut seen = HashSet::default();
    let mut cur = id;
    loop {
        if !seen.insert(cur) {
            return None;
        }
        match find_top_level_local_def(block, cur)? {
            Value::Local(Local(src)) => cur = *src,
            Value::FunRef(name) | Value::AllocClosure { fun: name, .. } => {
                return Some(name.name.clone());
            }
            Value::ClosureCap { index, .. } => {
                return cap_funs.and_then(|m| m.get(index).cloned());
            }
            Value::IndirectCall { callee, args } => {
                let cal = resolve_fun_name(block, callee.0, cap_funs, hof)?;
                // id(f) → f; apply returning a Fun is uncommon — treat first arg.
                if hof.id.contains(cal.as_str()) {
                    let farg = args.first()?;
                    cur = farg.0;
                    continue;
                }
                if hof.apply.contains(cal.as_str()) {
                    let farg = args.first()?;
                    cur = farg.0;
                    continue;
                }
                return None;
            }
            _ => return None,
        }
    }
}

pub(super) fn local_callee_ty(
    block: &Block,
    id: u32,
    fun_ret_tys: &HashMap<Sym, Type>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    match find_top_level_local_def(block, id)? {
        Value::Local(Local(src)) => local_callee_ty(block, *src, fun_ret_tys, seen),
        Value::Call { fun, .. } => fun_ret_tys
            .get(fun.as_str())
            .and_then(ret_ty_from_callee_table),
        _ => None,
    }
}

pub(super) fn local_icall_cap_ty(
    block: &Block,
    id: u32,
    cap_srcs: &[Local],
    funref_locals: &HashMap<u32, Sym>,
    fun_ret_tys: &HashMap<Sym, Type>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    match find_top_level_local_def(block, id)? {
        Value::Local(Local(src)) => {
            local_icall_cap_ty(block, *src, cap_srcs, funref_locals, fun_ret_tys, seen)
        }
        Value::IndirectCall { callee, .. } => {
            let idx = closure_cap_index(block, callee.0, &mut HashSet::default())?;
            let src = cap_srcs.get(idx as usize)?;
            let name = funref_locals.get(&src.0)?;
            fun_ret_tys.get(name).and_then(ret_ty_from_callee_table)
        }
        _ => None,
    }
}

pub(super) fn local_icall_cap_ty_by_index(
    block: &Block,
    id: u32,
    cap_funs: &HashMap<u32, Sym>,
    fun_ret_tys: &HashMap<Sym, Type>,
    seen: &mut HashSet<u32>,
) -> Option<Type> {
    if !seen.insert(id) {
        return None;
    }
    match find_top_level_local_def(block, id)? {
        Value::Local(Local(src)) => {
            local_icall_cap_ty_by_index(block, *src, cap_funs, fun_ret_tys, seen)
        }
        Value::IndirectCall { callee, .. } => {
            let idx = closure_cap_index(block, callee.0, &mut HashSet::default())?;
            let name = cap_funs.get(&idx)?;
            fun_ret_tys.get(name).and_then(ret_ty_from_callee_table)
        }
        _ => None,
    }
}

pub(super) fn closure_cap_index(block: &Block, id: u32, seen: &mut HashSet<u32>) -> Option<u32> {
    if !seen.insert(id) {
        return None;
    }
    match find_top_level_local_def(block, id)? {
        Value::Local(Local(src)) => closure_cap_index(block, *src, seen),
        Value::ClosureCap { index, .. } => Some(*index),
        _ => None,
    }
}
