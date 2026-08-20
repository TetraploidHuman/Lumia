//! Shared Core `Value` → [`Type`] / heap-root helpers for mono + codegen.
//!
//! Builtin arms live in [`builtin`] (not an in-file `fn` only).

use crate::{AdtRepr, CoreBinOp as BinOp, CoreUnOp as UnOp, ListRepr, Local, Value};
use lumia_hir::Builtin;
use lumia_syntax::Sym;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::sync::Arc;

/// Whether Lit* stack allocations count as heap for GC rooting / escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapPolicy {
    /// Escape / lambda analysis: every `Alloc*` is treated as heap.
    Conservative,
    /// Codegen roots: non-empty LitList/LitAdt may live on the stack.
    /// Map/Set never stack — empty is null; otherwise always heap+finish.
    StackLitOk,
}

/// Optional lookups for extended value-type inference (codegen / mono).
#[derive(Clone, Copy)]
pub struct InferValueCtx<'a> {
    pub local_tys: &'a HashMap<u32, Type>,
    pub slot_tys: Option<&'a HashMap<Sym, Type>>,
    pub fun_ret_tys: Option<&'a HashMap<Sym, Type>>,
    pub fun_param_tys: Option<&'a HashMap<Sym, Vec<Type>>>,
    pub fun_param0_identity: Option<&'a HashSet<Sym>>,
    pub funref_locals: Option<&'a HashMap<u32, Sym>>,
    /// SSA locals bound to `Value::Int` (for `AdtField` index → `params[i]`).
    pub local_int_consts: Option<&'a HashMap<u32, i64>>,
    /// Sum ADT name → max variant payload arity (pad `AllocAdt` params).
    pub sum_max_arity: Option<&'a HashMap<Sym, usize>>,
    /// Module-wide `ChannelSend` payload when all sends agree (else erased Int).
    pub channel_elem_hint: Option<&'a Type>,
    /// Current fun's capture-index → ty (typed Float ABI).
    pub closure_cap_tys: Option<&'a HashMap<u32, Type>>,
}

/// Grouped codegen tables so [`InferValueCtx::full`] stays a short call site.
#[derive(Clone, Copy)]
pub struct CodegenTypeTables<'a> {
    pub slot_tys: &'a HashMap<Sym, Type>,
    pub fun_ret_tys: &'a HashMap<Sym, Type>,
    pub fun_param_tys: &'a HashMap<Sym, Vec<Type>>,
    pub fun_param0_identity: &'a HashSet<Sym>,
    pub funref_locals: &'a HashMap<u32, Sym>,
    pub local_int_consts: &'a HashMap<u32, i64>,
    pub sum_max_arity: &'a HashMap<Sym, usize>,
    pub channel_elem_hint: Option<&'a Type>,
}

impl<'a> InferValueCtx<'a> {
    pub fn local_only(local_tys: &'a HashMap<u32, Type>) -> Self {
        Self {
            local_tys,
            slot_tys: None,
            fun_ret_tys: None,
            fun_param_tys: None,
            fun_param0_identity: None,
            funref_locals: None,
            local_int_consts: None,
            sum_max_arity: None,
            channel_elem_hint: None,
            closure_cap_tys: None,
        }
    }

    /// Locals + int consts (for `AdtField` index → `params[i]`).
    pub fn with_int_consts(
        local_tys: &'a HashMap<u32, Type>,
        local_int_consts: &'a HashMap<u32, i64>,
    ) -> Self {
        Self {
            local_tys,
            slot_tys: None,
            fun_ret_tys: None,
            fun_param_tys: None,
            fun_param0_identity: None,
            funref_locals: None,
            local_int_consts: Some(local_int_consts),
            sum_max_arity: None,
            channel_elem_hint: None,
            closure_cap_tys: None,
        }
    }

    /// Mid-end partial context: locals + optional slots + function ABI tables.
    ///
    /// Used by float_cap_fixup (and similar) where FunRef / int-const / sum-arity
    /// tables are not needed — thinner than [`Self::full`].
    pub fn with_fun_abi(
        local_tys: &'a HashMap<u32, Type>,
        slot_tys: Option<&'a HashMap<Sym, Type>>,
        fun_ret_tys: &'a HashMap<Sym, Type>,
        fun_param_tys: &'a HashMap<Sym, Vec<Type>>,
    ) -> Self {
        Self {
            local_tys,
            slot_tys,
            fun_ret_tys: Some(fun_ret_tys),
            fun_param_tys: Some(fun_param_tys),
            fun_param0_identity: None,
            funref_locals: None,
            local_int_consts: None,
            sum_max_arity: None,
            channel_elem_hint: None,
            closure_cap_tys: None,
        }
    }

    /// Full codegen tables (slots + function ABI + FunRef locals + int consts).
    pub fn full(local_tys: &'a HashMap<u32, Type>, tables: CodegenTypeTables<'a>) -> Self {
        Self {
            local_tys,
            slot_tys: Some(tables.slot_tys),
            fun_ret_tys: Some(tables.fun_ret_tys),
            fun_param_tys: Some(tables.fun_param_tys),
            fun_param0_identity: Some(tables.fun_param0_identity),
            funref_locals: Some(tables.funref_locals),
            local_int_consts: Some(tables.local_int_consts),
            sum_max_arity: Some(tables.sum_max_arity),
            channel_elem_hint: tables.channel_elem_hint,
            closure_cap_tys: None,
        }
    }
}

/// Whether emitting / analyzing `v` may produce a heap pointer under `policy`.
pub fn value_alloc_may_heap(v: &Value, policy: HeapPolicy) -> bool {
    match v {
        Value::String(_) | Value::Char(_) => true,
        Value::AllocList { elems, repr } => match policy {
            HeapPolicy::Conservative => true,
            HeapPolicy::StackLitOk => !matches!(repr, ListRepr::LitList) || elems.is_empty(),
        },
        // Empty Map/Set emit as null; LitSet/LitMap are PE tags, not stack layouts.
        Value::AllocSet { elems, .. } => !elems.is_empty(),
        Value::AllocMap { flat_pairs, .. } => !flat_pairs.is_empty(),
        Value::AllocAdt { repr, .. } => match policy {
            HeapPolicy::Conservative => true,
            HeapPolicy::StackLitOk => !matches!(repr, AdtRepr::LitAdt),
        },
        Value::AllocClosure { .. } | Value::ClosureCap { .. } | Value::FunRef(_) => true,
        _ => false,
    }
}

/// Whether a ground [`Type`] may be a heap pointer (GC root / COW).
///
/// Shared by codegen roots and lift/mono heap lattices so new container types
/// update one place. For Call/slot *unknown* projections see [`HeapMay`].
pub fn type_may_heap(ty: &Type) -> bool {
    match ty {
        Type::String
        | Type::Char
        | Type::List(_)
        | Type::Map(_, _)
        | Type::Set(_)
        | Type::Task(_)
        | Type::Channel(_)
        | Type::Adt { .. }
        | Type::Fun(_, _, _) => true,
        Type::Tuple(ts) | Type::TuplePrefix(ts) => ts.iter().any(type_may_heap),
        _ => false,
    }
}

/// Three-way heap likelihood for rooting vs slot sizing (Todo: 未知→非堆假收口).
///
/// Full `CoreTy::Unknown` is still deferred; this enum documents the intentional
/// dual projection in one place:
/// - [`Self::for_rooting`]: Unknown → true (prefer a useless root over a miss)
/// - [`Self::for_slot_alloc`]: Unknown → false (unknown slots size as Int until typed)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapMay {
    No,
    Yes,
    Unknown,
}

impl HeapMay {
    pub fn from_type(ty: &Type) -> Self {
        match ty {
            Type::Unknown => Self::Unknown,
            t if type_may_heap(t) => Self::Yes,
            _ => Self::No,
        }
    }

    /// Shadow-stack / handoff: over-root when unknown.
    pub fn for_rooting(self) -> bool {
        !matches!(self, Self::No)
    }

    /// Mut-slot prologue sizing: unknown stays non-heap until `slot_tys` says so.
    pub fn for_slot_alloc(self) -> bool {
        matches!(self, Self::Yes)
    }
}

/// Lift/codegen shared [`ResultHeap`] projection.
///
/// Typed + stamp → [`type_may_heap`]. Typed + no stamp: `ChannelRecv` /
/// `TaskJoin` stay non-heap (scalar-common until fixup); other Typed over-root
/// unless `infer` yields a ground type.
pub fn builtin_result_may_heap(
    name: Builtin,
    stamped: Option<&Type>,
    infer: impl FnOnce() -> Option<Type>,
) -> bool {
    use lumia_hir::ResultHeap;
    match name.result_heap() {
        ResultHeap::Never => false,
        ResultHeap::Always => true,
        ResultHeap::Typed => {
            if let Some(ty) = stamped {
                return type_may_heap(ty);
            }
            if matches!(name, Builtin::ChannelRecv | Builtin::TaskJoin) {
                return false;
            }
            match infer() {
                Some(ty) => type_may_heap(&ty),
                None => true,
            }
        }
    }
}

/// Infer a Core value's type from local SSA types + optional codegen lookups.
pub fn infer_value_ty(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    mut call_ret: impl FnMut(&str, &[Local]) -> Option<Type>,
) -> Type {
    infer_value_ty_ctx(
        value,
        InferValueCtx::local_only(local_tys),
        Some(&mut call_ret),
    )
}

/// Extended inference with slot / function tables (codegen) or per-call lookup (mono).
#[allow(clippy::type_complexity)]
pub fn infer_value_ty_ctx(
    value: &Value,
    ctx: InferValueCtx<'_>,
    mut call_ret: Option<&mut dyn FnMut(&str, &[Local]) -> Option<Type>>,
) -> Type {
    match value {
        Value::Float(_)
        | Value::Bool(_)
        | Value::Int(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit => lit_scalar_ty(value).unwrap_or(Type::Int),
        Value::Local(l) => ctx.local_tys.get(&l.0).cloned().unwrap_or(Type::Int),
        Value::Name(n) => ctx
            .slot_tys
            .and_then(|m| m.get(n).cloned())
            .unwrap_or(Type::Int),
        Value::Unary { op: UnOp::Not, .. } => Type::Bool,
        Value::Unary { operand, .. } => ctx.local_tys.get(&operand.0).cloned().unwrap_or(Type::Int),
        Value::Binary { op, left, right } => match op {
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Type::Bool,
            // HIR desugars `and`/`or` to `If`; residual Binary is an ICE.
            BinOp::And | BinOp::Or => {
                debug_assert!(false, "ICE: BinOp::And|Or in Core; expected If desugar");
                Type::Bool
            }
            _ => {
                let lt = ctx.local_tys.get(&left.0).cloned().unwrap_or(Type::Int);
                let rt = ctx.local_tys.get(&right.0).cloned().unwrap_or(Type::Int);
                if matches!(lt, Type::String) || matches!(rt, Type::String) {
                    Type::String
                } else {
                    binop_float_or_int(&lt, &rt)
                }
            }
        },
        Value::AllocList { elems, .. } => {
            let elem = elems
                .first()
                .and_then(|e| ctx.local_tys.get(&e.0).cloned())
                .unwrap_or(Type::Int);
            alloc_list_ty(elem)
        }
        Value::AllocSet { elems, .. } => {
            let elem = elems
                .first()
                .and_then(|e| ctx.local_tys.get(&e.0).cloned())
                .unwrap_or(Type::Int);
            alloc_set_ty(elem)
        }
        Value::AllocMap { flat_pairs, .. } => {
            let kv = if flat_pairs.len() >= 2 {
                Some((
                    ctx.local_tys
                        .get(&flat_pairs[0].0)
                        .cloned()
                        .unwrap_or(Type::Int),
                    ctx.local_tys
                        .get(&flat_pairs[1].0)
                        .cloned()
                        .unwrap_or(Type::Int),
                ))
            } else {
                None
            };
            alloc_map_from_pair(kv)
        }
        Value::AllocAdt {
            adt_name, fields, ..
        } => {
            let params: Vec<Type> = fields
                .iter()
                .map(|f| ctx.local_tys.get(&f.0).cloned().unwrap_or(Type::Int))
                .collect();
            let params = pad_adt_params(
                params,
                ctx.sum_max_arity.and_then(|m| m.get(adt_name.as_str()).copied()),
            );
            Type::Adt {
                name: adt_name.clone(),
                params,
            }
        }
        Value::Call { fun, args } => {
            let ret = match (
                ctx.fun_ret_tys.and_then(|m| m.get(fun.as_str()).cloned()),
                call_ret.as_mut(),
            ) {
                // Prefer an explicit table entry when present (mono clones / FunRefs).
                (Some(t), _) => t,
                // Otherwise ask the call-site mono / index callback.
                (None, Some(f)) => f(fun.as_str(), args).unwrap_or(Type::Int),
                (None, None) => Type::Int,
            };
            identity_passthrough_call_ret(ret, fun.as_str(), args, ctx)
        }
        Value::Builtin {
            name: Builtin::ListParMap,
            args,
            ..
        } => Type::List(Arc::new(list_par_map_result_elem(args, ctx))),
        Value::Builtin {
            name,
            args,
            result_ty,
        } => {
            if let Some(ty) = result_ty {
                ty.clone()
            } else {
                builtin_value_ty(*name, args, ctx)
            }
        }
        Value::ClosureCap { index, .. } => {
            if let Some(caps) = ctx.closure_cap_tys {
                if let Some(t) = caps.get(index) {
                    return t.clone();
                }
            }
            Type::Int
        }
        Value::FunRef(name) | Value::AllocClosure { fun: name, .. } => {
            let ret = ctx
                .fun_ret_tys
                .and_then(|m| m.get(name.as_str()).cloned())
                .unwrap_or(Type::Int);
            let params = ctx
                .fun_param_tys
                .and_then(|m| m.get(name.as_str()).cloned())
                .unwrap_or_default();
            Type::Fun(params, Arc::new(ret), Effect::pure())
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            let t = then_block
                .result
                .and_then(|Local(id)| ctx.local_tys.get(&id).cloned());
            let e = else_block
                .result
                .and_then(|Local(id)| ctx.local_tys.get(&id).cloned());
            match (t, e) {
                (Some(a), Some(b)) => join_value_tys(&a, &b).unwrap_or(a),
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => Type::Int,
            }
        }
        Value::IndirectCall { callee, args } => {
            let fun_ty = ctx.local_tys.get(&callee.0);
            let ret = fun_ret_of_callee_ty(fun_ty).unwrap_or_else(|| {
                ctx.funref_locals
                    .and_then(|m| m.get(&callee.0))
                    .and_then(|name| ctx.fun_ret_tys.and_then(|m| m.get(name).cloned()))
                    .unwrap_or(Type::Int)
            });
            if let Some(name) = ctx.funref_locals.and_then(|m| m.get(&callee.0)) {
                identity_passthrough_call_ret(ret, name, args, ctx)
            } else if let Some(t) = identity_shaped_fun_arg_passthrough(fun_ty, args, ctx.local_tys)
            {
                // `id` captured as ClosureCap: no funref_locals name, but Fun is
                // still Int→Int / List[Int] placeholder — adopt the arg ABI
                // (`id(listOf(1.0)).fold` must keep List[Float] elems).
                t
            } else {
                ret
            }
        }
        Value::Loop { .. } => Type::Int,
        // After lambda_lift, residual `Lambda` is an ICE (maps to Int only so
        // release builds still type-check walkers).
        Value::Lambda { .. } => {
            debug_assert!(
                false,
                "ICE: Value::Lambda after lift; expected FunRef/AllocClosure"
            );
            Type::Int
        }
    }
}

/// Open/identity Fun ret placeholders erased before mono Float clones exist.
fn identity_placeholder_ret(ret: &Type) -> bool {
    match ret {
        Type::Int | Type::Var(_) => true,
        Type::List(e) if matches!(e.as_ref(), Type::Int) => true,
        _ => false,
    }
}

/// Identity / open Fun with lift placeholder `Int`/`List[Int]` ret: adopt the
/// argument's concrete ABI so `m.get(k) alt id` / `id(xs).fold` ICall keep ABI.
fn identity_shaped_fun_arg_passthrough(
    fun_ty: Option<&Type>,
    args: &[Local],
    local_tys: &HashMap<u32, Type>,
) -> Option<Type> {
    let Type::Fun(ps, ret, _) = fun_ty? else {
        return None;
    };
    if ps.len() != 1 || args.len() != 1 {
        return None;
    }
    if !matches!(ps[0], Type::Int | Type::Var(_)) {
        return None;
    }
    if !identity_placeholder_ret(ret) {
        return None;
    }
    let arg_ty = local_tys.get(&args[0].0)?;
    match arg_ty {
        Type::Float
        | Type::List(_)
        | Type::Map(_, _)
        | Type::Set(_)
        | Type::Adt { .. }
        | Type::Fun(_, _, _)
        | Type::Task(_)
        | Type::Channel(_)
        | Type::String
        | Type::Char
        | Type::Bool => Some(arg_ty.clone()),
        _ => None,
    }
}

fn identity_passthrough_call_ret(
    ret: Type,
    fun: &str,
    args: &[Local],
    ctx: InferValueCtx<'_>,
) -> Type {
    let Some(arg0) = args.first() else {
        return ret;
    };
    let Some(arg_ty) = ctx.local_tys.get(&arg0.0) else {
        return ret;
    };
    let is_id = ctx.fun_param0_identity.is_some_and(|s| s.contains(fun));
    let ptys = ctx
        .fun_param_tys
        .and_then(|m| m.get(fun).cloned())
        .unwrap_or_default();
    // Legacy: Int-param + Float arg before identity set is wired.
    if !is_id {
        if args.len() == 1
            && ptys.len() == 1
            && matches!(ptys[0], Type::Int)
            && matches!(arg_ty, Type::Float)
        {
            return Type::Float;
        }
        // Open List[Int] ret still adopts a concrete list arg (mono-less icall).
        if matches!(&ret, Type::List(e) if matches!(e.as_ref(), Type::Int))
            && matches!(arg_ty, Type::List(_))
        {
            return arg_ty.clone();
        }
        return ret;
    }
    if !identity_placeholder_ret(&ret) {
        return ret;
    }
    match arg_ty {
        Type::Float
        | Type::Int
        | Type::Bool
        | Type::String
        | Type::Char
        | Type::Adt { .. }
        | Type::List(_)
        | Type::Map(_, _)
        | Type::Set(_)
        | Type::Fun(_, _, _)
        | Type::Task(_)
        | Type::Channel(_) => arg_ty.clone(),
        _ => ret,
    }
}

fn list_elem_preserved(args: &[Local], local_tys: &HashMap<u32, Type>) -> Type {
    if let Some(arg0) = args.first() {
        if let Some(Type::List(elem)) = local_tys.get(&arg0.0) {
            return Type::List(elem.clone());
        }
    }
    Type::List(Arc::new(Type::Int))
}

/// `ListParMap` result element from callback ret + source list elem.
///
/// Soft open `Var` on a Float list stays Float (specialize before mono).
/// **Concrete `Int` must not soft-upgrade** — auto-parallel `map` used to tag
/// `List[Float].map { _ -> 1 }` as Float and Show Int `1` as a denormal.
pub(crate) fn par_map_result_elem_ty(
    list_elem: &Type,
    cb_ret: &Type,
    identity_on_float_list: bool,
) -> Type {
    if identity_on_float_list && matches!(list_elem, Type::Float) {
        return Type::Float;
    }
    match cb_ret {
        Type::Float => Type::Float,
        Type::Var(_) if matches!(list_elem, Type::Float) => Type::Float,
        Type::Int => Type::Int,
        Type::Var(_) => list_elem.clone(),
        other => other.clone(),
    }
}

/// Early Float result for float_abi `ListParMap` (before via).
///
/// Returns `Some(Float)` when the shared lattice says Float elems; `None` means
/// fall through to via / soft projection (e.g. concrete Int on a Float list).
pub(crate) fn par_map_float_abi_early(
    list_elem: Option<&Type>,
    cb_ret: Option<&Type>,
) -> Option<Type> {
    match (cb_ret, list_elem) {
        (Some(cb), Some(le)) => {
            let e = par_map_result_elem_ty(le, cb, false);
            matches!(e, Type::Float).then_some(Type::Float)
        }
        (Some(Type::Float), None) => Some(Type::Float),
        // Unknown callback on Float list: keep Float ABI for specialize.
        (None, Some(Type::Float)) => Some(Type::Float),
        _ => None,
    }
}

/// Early Float/scalar result for float_abi `ListParFold` (before via).
///
/// `acc_is_float_local` is float_abi-only (def-order `float_locals` table).
pub(crate) fn par_fold_float_abi_early(
    acc_is_float_local: bool,
    list_elem: Option<&Type>,
    cb_ret: Option<&Type>,
) -> Option<Type> {
    if acc_is_float_local {
        return Some(Type::Float);
    }
    if matches!(list_elem, Some(Type::Float)) {
        return Some(Type::Float);
    }
    cb_ret
        .filter(|t| matches!(t, Type::Float | Type::Bool | Type::String | Type::Char))
        .cloned()
}

fn list_par_map_result_elem(args: &[Local], ctx: InferValueCtx<'_>) -> Type {
    let list_elem = match list_elem_preserved(args, ctx.local_tys) {
        Type::List(elem) => Type::unbox(elem),
        other => other,
    };
    if let Some(fun_local) = args.get(1) {
        let name = ctx.funref_locals.and_then(|m| m.get(&fun_local.0)).cloned();
        let fun_ret = name
            .as_ref()
            .and_then(|n| ctx.fun_ret_tys.and_then(|m| m.get(n).cloned()))
            .or_else(|| match ctx.local_tys.get(&fun_local.0) {
                Some(Type::Fun(_, ret, _)) => Some((**ret).clone()),
                _ => None,
            });
        if let Some(ret) = fun_ret {
            let identity = name
                .as_ref()
                .is_some_and(|n| ctx.fun_param0_identity.is_some_and(|s| s.contains(n)));
            return par_map_result_elem_ty(&list_elem, &ret, identity);
        }
    }
    list_elem
}

/// Public helper for codegen: element type of `List.parMap` / `ListParMap` result.
pub fn list_par_map_elem_ty(args: &[Local], ctx: InferValueCtx<'_>) -> Type {
    list_par_map_result_elem(args, ctx)
}

/// `AdtField(obj, idx)` → `params[idx]` (not always `params[0]`).
///
/// Mis-typing every field as `params[0]` made List fields look like Float, so ADT
/// float-masks skipped GC marks on live lists (UAF → `get unsupported type_id`).
fn join_value_tys(a: &Type, b: &Type) -> Option<Type> {
    join_abi_tys(a, b, JoinAbiKind::Value)
}

/// Empty `mapOf`/`setOf` placeholders use `Int`/`Var` elems; a later write of a
/// concrete scalar (Bool/String/Float/…) must upgrade — same idea as ListAppend.

/// Literal scalar / unit type (shared by value_ty / ret_ty / float_abi).
pub(crate) fn lit_scalar_ty(value: &Value) -> Option<Type> {
    match value {
        Value::String(_) => Some(Type::String),
        Value::Char(_) => Some(Type::Char),
        Value::Float(_) => Some(Type::Float),
        Value::Bool(_) => Some(Type::Bool),
        Value::Int(_) => Some(Type::Int),
        Value::Unit => Some(Type::Unit),
        _ => None,
    }
}

/// `AllocList` / `AllocSet` / `AllocMap` shape constructors (shared walkers).
pub(crate) fn alloc_list_ty(elem: Type) -> Type {
    Type::List(Arc::new(elem))
}

pub(crate) fn alloc_set_ty(elem: Type) -> Type {
    Type::Set(Arc::new(elem))
}

pub(crate) fn alloc_map_ty(k: Type, v: Type) -> Type {
    Type::Map(Arc::new(k), Arc::new(v))
}

pub(crate) fn alloc_map_from_pair(kv: Option<(Type, Type)>) -> Type {
    match kv {
        Some((k, v)) => alloc_map_ty(k, v),
        None => alloc_map_ty(Type::Int, Type::Int),
    }
}

/// Pad sum-ADT params to `max` with Int placeholders (value_ty / ret_ty).
pub(crate) fn pad_adt_params(mut params: Vec<Type>, max: Option<usize>) -> Vec<Type> {
    if let Some(max) = max {
        while params.len() < max {
            params.push(Type::Int);
        }
    }
    params
}

/// Fun ret payload from a callee type (IndirectCall shared arm).
pub(crate) fn fun_ret_of_callee_ty(callee: Option<&Type>) -> Option<Type> {
    match callee {
        Some(Type::Fun(_, ret, _)) => Some((**ret).clone()),
        _ => None,
    }
}

/// Float-wins numeric binop (value_ty adds String; float_abi returns None on miss).
pub(crate) fn binop_float_or_int(lt: &Type, rt: &Type) -> Type {
    if matches!(lt, Type::Float) || matches!(rt, Type::Float) {
        Type::Float
    } else {
        Type::Int
    }
}

/// float_abi arithmetic: only concrete Float is heap-authoritative.
pub(crate) fn float_arith_binop_ty(lt: Option<&Type>, rt: Option<&Type>) -> Option<Type> {
    if matches!(lt, Some(Type::Float)) || matches!(rt, Some(Type::Float)) {
        Some(Type::Float)
    } else {
        None
    }
}

mod builtin;
mod join;

pub(crate) use builtin::{
    adt_field_via, builtin_value_ty, channel_recv_ok, elems_family_recv_ok, float_adt_field_ty,
    float_list_append_ty, float_list_concat_ty, float_list_par_fold_ty, float_list_par_map_ty,
    float_map_remove_ty, float_map_set_ty, float_set_insert_ty, fun_recv_ok,
    is_fixed_result_builtin, list_concat_both_known, list_get_recv_ok, list_par_fold_via,
    list_par_map_via, list_passthrough_ok, stamp_or_via, stamp_or_via_gated_recv, task_recv_ok,
    via_gated_recv, via_gated_recv_seeded,
};
pub use join::{
    fold_slot_assign_ty, join_abi_tys, join_fixed_ty, join_if_arm_tys, join_slot_assign_ty,
    prefer_concrete_heap_ty, JoinAbiKind, JoinAssignKind,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ListRepr;

    #[test]
    fn lit_list_heap_policy() {
        let v = Value::AllocList {
            elems: vec![Local(0)],
            repr: ListRepr::LitList,
        };
        assert!(value_alloc_may_heap(&v, HeapPolicy::Conservative));
        assert!(!value_alloc_may_heap(&v, HeapPolicy::StackLitOk));
    }

    #[test]
    fn type_may_heap_covers_containers_not_scalars() {
        assert!(type_may_heap(&Type::String));
        assert!(type_may_heap(&Type::List(Arc::new(Type::Int))));
        assert!(type_may_heap(&Type::Fun(
            vec![Type::Int],
            Arc::new(Type::Int),
            Effect::pure()
        )));
        assert!(type_may_heap(&Type::Tuple(vec![Type::Int, Type::String])));
        assert!(!type_may_heap(&Type::Int));
        assert!(!type_may_heap(&Type::Float));
        assert!(!type_may_heap(&Type::Bool));
        assert!(!type_may_heap(&Type::Unit));
        assert!(type_may_heap(&Type::Char));
        assert!(!type_may_heap(&Type::Tuple(vec![Type::Int, Type::Bool])));
    }

    #[test]
    fn heap_may_dual_projection() {
        assert!(HeapMay::from_type(&Type::List(Arc::new(Type::Int))).for_rooting());
        assert!(HeapMay::from_type(&Type::List(Arc::new(Type::Int))).for_slot_alloc());
        assert!(!HeapMay::from_type(&Type::Int).for_rooting());
        assert!(!HeapMay::from_type(&Type::Int).for_slot_alloc());
        // Unknown: over-root for GC; under-size slots until typed.
        assert!(HeapMay::Unknown.for_rooting());
        assert!(!HeapMay::Unknown.for_slot_alloc());
    }

    #[test]
    fn builtin_result_may_heap_stamp_first() {
        assert!(!builtin_result_may_heap(
            lumia_hir::Builtin::ListGet,
            Some(&Type::Int),
            || Some(Type::List(Arc::new(Type::Int)))
        ));
        assert!(builtin_result_may_heap(
            lumia_hir::Builtin::ListGet,
            None,
            || Some(Type::List(Arc::new(Type::Int)))
        ));
    }

    #[test]
    fn list_append_upgrades_int_elem_to_task() {
        let mut tys = HashMap::default();
        tys.insert(0, Type::List(Arc::new(Type::Int)));
        tys.insert(1, Type::Task(Arc::new(Type::Float)));
        let t = infer_value_ty(
            &Value::Builtin {
                name: lumia_hir::Builtin::ListAppend,
                args: vec![Local(0), Local(1)],
                result_ty: None,
            },
            &tys,
            |_, _| None,
        );
        assert_eq!(t, Type::List(Arc::new(Type::Task(Arc::new(Type::Float)))));
    }

    #[test]
    fn list_append_prefers_nested_float_list_elem() {
        // Soft Int-only upgrade would keep List[List[Int]]; prefer upgrades elems.
        let mut tys = HashMap::default();
        tys.insert(0, Type::List(Arc::new(Type::List(Arc::new(Type::Int)))));
        tys.insert(1, Type::List(Arc::new(Type::Float)));
        let t = infer_value_ty(
            &Value::Builtin {
                name: lumia_hir::Builtin::ListAppend,
                args: vec![Local(0), Local(1)],
                result_ty: None,
            },
            &tys,
            |_, _| None,
        );
        assert_eq!(
            t,
            Type::List(Arc::new(Type::List(Arc::new(Type::Float)))),
            "ListAppend must prefer nested Float elems, got {t:?}"
        );
    }

    #[test]
    fn map_set_upgrades_int_key_val_to_bool() {
        // Empty mapOf is Map[Int,Int]; `.set(true, false)` must become Map[Bool,Bool]
        // so println uses lumia_show_map_bool (not {1: 0}).
        let mut tys = HashMap::default();
        tys.insert(0, Type::Map(Arc::new(Type::Int), Arc::new(Type::Int)));
        tys.insert(1, Type::Bool);
        tys.insert(2, Type::Bool);
        let t = infer_value_ty(
            &Value::Builtin {
                name: lumia_hir::Builtin::MapSet,
                args: vec![Local(0), Local(1), Local(2)],
                result_ty: None,
            },
            &tys,
            |_, _| None,
        );
        assert_eq!(t, Type::Map(Arc::new(Type::Bool), Arc::new(Type::Bool)));
    }

    #[test]
    fn set_insert_upgrades_int_elem_to_bool() {
        let mut tys = HashMap::default();
        tys.insert(0, Type::Set(Arc::new(Type::Int)));
        tys.insert(1, Type::Bool);
        let t = infer_value_ty(
            &Value::Builtin {
                name: lumia_hir::Builtin::SetInsert,
                args: vec![Local(0), Local(1)],
                result_ty: None,
            },
            &tys,
            |_, _| None,
        );
        assert_eq!(t, Type::Set(Arc::new(Type::Bool)));
    }

    #[test]
    fn float_list_append_open_invents_list_elem() {
        let args = [Local(0), Local(1)];
        assert_eq!(
            float_list_append_ty(&args, None, Type::Float),
            Some(Type::List(Arc::new(Type::Float)))
        );
        assert_eq!(
            float_list_append_ty(&args, None, Type::Int),
            Some(Type::List(Arc::new(Type::Int)))
        );
        // Non-List known recv passes through (no invent).
        assert_eq!(
            float_list_append_ty(&args, Some(Type::String), Type::Float),
            Some(Type::String)
        );
    }

    #[test]
    fn float_map_set_open_never_int_key_to_list() {
        // builtin_value_ty open Int-key → List; float_abi must stay Map.
        let args = [Local(0), Local(1), Local(2)];
        assert_eq!(
            float_map_set_ty(&args, None, Type::Int, Type::Float),
            Some(Type::Map(Arc::new(Type::Int), Arc::new(Type::Float)))
        );
        // Known List still via → List with preferred elem.
        assert_eq!(
            float_map_set_ty(
                &args,
                Some(Type::List(Arc::new(Type::Int))),
                Type::Int,
                Type::Float
            ),
            Some(Type::List(Arc::new(Type::Float)))
        );
    }

    #[test]
    fn float_set_insert_open_seeds_set() {
        let args = [Local(0), Local(1)];
        assert_eq!(
            float_set_insert_ty(&args, None, Type::Float),
            Some(Type::Set(Arc::new(Type::Float)))
        );
    }

    #[test]
    fn float_map_remove_open_soft_map() {
        let args = [Local(0), Local(1)];
        assert_eq!(
            float_map_remove_ty(&args, None, Type::Float),
            Some(Type::Map(Arc::new(Type::Float), Arc::new(Type::Int)))
        );
        // Known Set stays Set (via prefer).
        assert_eq!(
            float_map_remove_ty(&args, Some(Type::Set(Arc::new(Type::Int))), Type::Float),
            Some(Type::Set(Arc::new(Type::Float)))
        );
    }

    #[test]
    fn float_list_concat_open_one_side_and_both() {
        let args = [Local(0), Local(1)];
        // One side only: keep concrete List/String; never invent soft List[Int].
        assert_eq!(
            float_list_concat_ty(&args, Some(Type::List(Arc::new(Type::Float))), None),
            Some(Type::List(Arc::new(Type::Float)))
        );
        assert_eq!(
            float_list_concat_ty(&args, None, Some(Type::String)),
            Some(Type::String)
        );
        assert_eq!(float_list_concat_ty(&args, None, None), None);
        // Both List: prefer nested Float over Int placeholder.
        assert_eq!(
            float_list_concat_ty(
                &args,
                Some(Type::List(Arc::new(Type::Int))),
                Some(Type::List(Arc::new(Type::Float))),
            ),
            Some(Type::List(Arc::new(Type::Float)))
        );
        // ret_ty gate: List×scalar → None (not float open policy).
        assert_eq!(
            list_concat_both_known(&args, Type::List(Arc::new(Type::Int)), Type::Int),
            None
        );
    }

    #[test]
    fn list_concat_upgrades_int_acc_to_fun_elem() {
        // flatMap empty acc is List[Int]; chunk listOf({…}) is List[Fun].
        let mut tys = HashMap::default();
        tys.insert(0, Type::List(Arc::new(Type::Int)));
        tys.insert(
            1,
            Type::List(Arc::new(Type::Fun(
                vec![Type::Float],
                Arc::new(Type::Float),
                Effect::pure(),
            ))),
        );
        let t = infer_value_ty(
            &Value::Builtin {
                name: lumia_hir::Builtin::ListConcat,
                args: vec![Local(0), Local(1)],
                result_ty: None,
            },
            &tys,
            |_, _| None,
        );
        assert!(
            matches!(
                &t,
                Type::List(e) if matches!(
                    e.as_ref(),
                    Type::Fun(_, ret, _) if matches!(ret.as_ref(), Type::Float)
                )
            ),
            "expected List[Fun(_, Float)], got {t:?}"
        );
    }

    #[test]
    fn list_concat_prefers_nested_float_over_int_list() {
        // Soft-only used to keep left List[List[Int]]; prefer upgrades elems.
        let mut tys = HashMap::default();
        tys.insert(0, Type::List(Arc::new(Type::List(Arc::new(Type::Int)))));
        tys.insert(1, Type::List(Arc::new(Type::List(Arc::new(Type::Float)))));
        let t = infer_value_ty(
            &Value::Builtin {
                name: lumia_hir::Builtin::ListConcat,
                args: vec![Local(0), Local(1)],
                result_ty: None,
            },
            &tys,
            |_, _| None,
        );
        assert_eq!(
            t,
            Type::List(Arc::new(Type::List(Arc::new(Type::Float)))),
            "List×List must prefer nested Float elems, got {t:?}"
        );
    }

    #[test]
    fn par_map_elem_ty_from_funref_ret() {
        let mut local_tys = HashMap::default();
        local_tys.insert(0, Type::List(Arc::new(Type::Int)));
        local_tys.insert(
            1,
            Type::Fun(vec![Type::Int], Arc::new(Type::Float), Effect::pure()),
        );
        let mut funref = HashMap::default();
        funref.insert(1, "dbl".into());
        let mut rets = HashMap::default();
        rets.insert("dbl".into(), Type::Float);
        let ctx = InferValueCtx {
            local_tys: &local_tys,
            slot_tys: None,
            fun_ret_tys: Some(&rets),
            fun_param_tys: None,
            fun_param0_identity: None,
            funref_locals: Some(&funref),
            local_int_consts: None,
            sum_max_arity: None,
            channel_elem_hint: None,
            closure_cap_tys: None,
        };
        assert_eq!(
            list_par_map_elem_ty(&[Local(0), Local(1)], ctx),
            Type::Float
        );
    }

    #[test]
    fn par_map_elem_ty_concrete_int_on_float_list_stays_int() {
        // Auto-parallel used to soft-upgrade Int→Float and Show 1 as a denormal.
        let mut local_tys = HashMap::default();
        local_tys.insert(0, Type::List(Arc::new(Type::Float)));
        local_tys.insert(
            1,
            Type::Fun(vec![Type::Float], Arc::new(Type::Int), Effect::pure()),
        );
        let ctx = InferValueCtx {
            local_tys: &local_tys,
            slot_tys: None,
            fun_ret_tys: None,
            fun_param_tys: None,
            fun_param0_identity: None,
            funref_locals: None,
            local_int_consts: None,
            sum_max_arity: None,
            channel_elem_hint: None,
            closure_cap_tys: None,
        };
        assert_eq!(list_par_map_elem_ty(&[Local(0), Local(1)], ctx), Type::Int);
    }

    #[test]
    fn par_map_elem_ty_open_var_on_float_list_stays_float() {
        let mut local_tys = HashMap::default();
        local_tys.insert(0, Type::List(Arc::new(Type::Float)));
        local_tys.insert(
            1,
            Type::Fun(vec![Type::Float], Arc::new(Type::Var(0)), Effect::pure()),
        );
        let ctx = InferValueCtx {
            local_tys: &local_tys,
            slot_tys: None,
            fun_ret_tys: None,
            fun_param_tys: None,
            fun_param0_identity: None,
            funref_locals: None,
            local_int_consts: None,
            sum_max_arity: None,
            channel_elem_hint: None,
            closure_cap_tys: None,
        };
        assert_eq!(
            list_par_map_elem_ty(&[Local(0), Local(1)], ctx),
            Type::Float
        );
    }

    #[test]
    fn par_map_float_abi_early_int_on_float_list_is_none() {
        // Concrete Int must not early-return Float (fall through to via / Int TID).
        assert!(par_map_float_abi_early(Some(&Type::Float), Some(&Type::Int)).is_none());
        assert_eq!(
            par_map_float_abi_early(Some(&Type::Float), Some(&Type::Var(0))),
            Some(Type::Float)
        );
        assert_eq!(
            par_map_float_abi_early(Some(&Type::Float), None),
            Some(Type::Float)
        );
    }

    #[test]
    fn par_fold_float_abi_early_list_float_and_scalars() {
        assert_eq!(
            par_fold_float_abi_early(true, None, None),
            Some(Type::Float)
        );
        assert_eq!(
            par_fold_float_abi_early(false, Some(&Type::Float), None),
            Some(Type::Float)
        );
        assert_eq!(
            par_fold_float_abi_early(false, Some(&Type::Int), Some(&Type::Bool)),
            Some(Type::Bool)
        );
        assert!(par_fold_float_abi_early(false, Some(&Type::Int), Some(&Type::Int)).is_none());
    }

    #[test]
    fn mono_float_clone_keeps_concrete_int_ret() {
        // `{ x -> 1 }` at Float still returns Int — not MonoKey-homogeneous Float.
        let src = r#"
module D
val main = {
    val xs = listOf(1.5, 2.5).map({ x -> 1 })
    xs
}
"#;
        let m = crate::compile_source_to_core_with_parallel(src, true).unwrap();
        let float_clone = m
            .functions
            .iter()
            .find(|f| f.name.contains("$Float"))
            .expect("$Float mono clone");
        assert!(
            matches!(float_clone.ret_ty, lumia_ty::Type::Int),
            "$Float map Int lambda ret must stay Int, got {:?}",
            float_clone.ret_ty
        );
    }

    #[test]
    fn mono_float_clone_keeps_float_add_ret() {
        let src = r#"
module E
val main = {
    val xs = listOf(1.5, 2.5).map({ x -> x + x })
    xs
}
"#;
        let m = crate::compile_source_to_core_with_parallel(src, true).unwrap();
        let float_clone = m
            .functions
            .iter()
            .find(|f| f.name.contains("$Float"))
            .expect("$Float mono clone");
        assert!(
            matches!(float_clone.ret_ty, lumia_ty::Type::Float),
            "$Float map (x+x) ret must stay Float, got {:?}",
            float_clone.ret_ty
        );
    }

    #[test]
    fn adt_field_uses_index_not_params0() {
        let mut local_tys = HashMap::default();
        local_tys.insert(
            0,
            Type::Adt {
                name: "Eco".into(),
                params: vec![Type::Float, Type::Float, Type::List(Arc::new(Type::Float))],
            },
        );
        local_tys.insert(1, Type::Int);
        let mut consts = HashMap::default();
        consts.insert(1, 2i64);
        let ctx = InferValueCtx {
            local_tys: &local_tys,
            slot_tys: None,
            fun_ret_tys: None,
            fun_param_tys: None,
            fun_param0_identity: None,
            funref_locals: None,
            local_int_consts: Some(&consts),
            sum_max_arity: None,
            channel_elem_hint: None,
            closure_cap_tys: None,
        };
        let t = infer_value_ty_ctx(
            &Value::Builtin {
                name: Builtin::AdtField,
                args: vec![Local(0), Local(1)],
                result_ty: None,
            },
            ctx,
            None,
        );
        assert_eq!(t, Type::List(Arc::new(Type::Float)));
    }
}
