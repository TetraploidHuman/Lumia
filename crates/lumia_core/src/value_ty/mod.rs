//! Shared Core `Value` → [`Type`] / heap-root helpers for mono + codegen.
//!
//! Builtin arms live in [`builtin`] (not an in-file `fn` only).

use crate::{AdtRepr, CoreBinOp as BinOp, CoreUnOp as UnOp, ListRepr, Local, Value};
use lumia_hir::Builtin;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

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
    pub slot_tys: Option<&'a HashMap<String, Type>>,
    pub fun_ret_tys: Option<&'a HashMap<String, Type>>,
    pub fun_param_tys: Option<&'a HashMap<String, Vec<Type>>>,
    pub fun_param0_identity: Option<&'a HashSet<String>>,
    pub funref_locals: Option<&'a HashMap<u32, String>>,
    /// SSA locals bound to `Value::Int` (for `AdtField` index → `params[i]`).
    pub local_int_consts: Option<&'a HashMap<u32, i64>>,
    /// Sum ADT name → max variant payload arity (pad `AllocAdt` params).
    pub sum_max_arity: Option<&'a HashMap<String, usize>>,
    /// Module-wide `ChannelSend` payload when all sends agree (else erased Int).
    pub channel_elem_hint: Option<&'a Type>,
}

/// Grouped codegen tables so [`InferValueCtx::full`] stays a short call site.
#[derive(Clone, Copy)]
pub struct CodegenTypeTables<'a> {
    pub slot_tys: &'a HashMap<String, Type>,
    pub fun_ret_tys: &'a HashMap<String, Type>,
    pub fun_param_tys: &'a HashMap<String, Vec<Type>>,
    pub fun_param0_identity: &'a HashSet<String>,
    pub funref_locals: &'a HashMap<u32, String>,
    pub local_int_consts: &'a HashMap<u32, i64>,
    pub sum_max_arity: &'a HashMap<String, usize>,
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
        }
    }

    /// Mid-end partial context: locals + optional slots + function ABI tables.
    ///
    /// Used by float_cap_fixup (and similar) where FunRef / int-const / sum-arity
    /// tables are not needed — thinner than [`Self::full`].
    pub fn with_fun_abi(
        local_tys: &'a HashMap<u32, Type>,
        slot_tys: Option<&'a HashMap<String, Type>>,
        fun_ret_tys: &'a HashMap<String, Type>,
        fun_param_tys: &'a HashMap<String, Vec<Type>>,
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
/// Shared by codegen roots and (eventually) lift/mono heap lattices so new
/// container types update one place (Todo: 多套「是否堆」启发式).
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
pub fn infer_value_ty_ctx(
    value: &Value,
    ctx: InferValueCtx<'_>,
    mut call_ret: Option<&mut dyn FnMut(&str, &[Local]) -> Option<Type>>,
) -> Type {
    match value {
        Value::Float(_) => Type::Float,
        Value::Bool(_) => Type::Bool,
        Value::Int(_) => Type::Int,
        Value::String(_) => Type::String,
        Value::Char(_) => Type::Char,
        Value::Unit => Type::Unit,
        Value::Local(l) => ctx.local_tys.get(&l.0).cloned().unwrap_or(Type::Int),
        Value::Name(n) => ctx
            .slot_tys
            .and_then(|m| m.get(n).cloned())
            .unwrap_or(Type::Int),
        Value::Unary { op: UnOp::Not, .. } => Type::Bool,
        Value::Unary { operand, .. } => ctx.local_tys.get(&operand.0).cloned().unwrap_or(Type::Int),
        Value::Binary { op, left, right } => match op {
            BinOp::Eq
            | BinOp::Ne
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge => Type::Bool,
            // HIR desugars `and`/`or` to `If`; residual Binary is an ICE.
            BinOp::And | BinOp::Or => {
                debug_assert!(false, "ICE: BinOp::And|Or in Core; expected If desugar");
                Type::Bool
            }
            _ => {
                let lt = ctx.local_tys.get(&left.0).cloned().unwrap_or(Type::Int);
                let rt = ctx.local_tys.get(&right.0).cloned().unwrap_or(Type::Int);
                if matches!(lt, Type::Float) || matches!(rt, Type::Float) {
                    Type::Float
                } else if matches!(lt, Type::String) || matches!(rt, Type::String) {
                    Type::String
                } else {
                    Type::Int
                }
            }
        },
        Value::AllocList { elems, .. } => {
            let elem = elems
                .first()
                .and_then(|e| ctx.local_tys.get(&e.0).cloned())
                .unwrap_or(Type::Int);
            Type::List(Box::new(elem))
        }
        Value::AllocSet { elems, .. } => {
            let elem = elems
                .first()
                .and_then(|e| ctx.local_tys.get(&e.0).cloned())
                .unwrap_or(Type::Int);
            Type::Set(Box::new(elem))
        }
        Value::AllocMap { flat_pairs, .. } => {
            let (k, v) = if flat_pairs.len() >= 2 {
                (
                    ctx.local_tys
                        .get(&flat_pairs[0].0)
                        .cloned()
                        .unwrap_or(Type::Int),
                    ctx.local_tys
                        .get(&flat_pairs[1].0)
                        .cloned()
                        .unwrap_or(Type::Int),
                )
            } else {
                (Type::Int, Type::Int)
            };
            Type::Map(Box::new(k), Box::new(v))
        }
        Value::AllocAdt {
            adt_name, fields, ..
        } => {
            let mut params: Vec<Type> = fields
                .iter()
                .map(|f| ctx.local_tys.get(&f.0).cloned().unwrap_or(Type::Int))
                .collect();
            if let Some(max) = ctx.sum_max_arity.and_then(|m| m.get(adt_name).copied()) {
                while params.len() < max {
                    params.push(Type::Int);
                }
            }
            Type::Adt {
                name: adt_name.clone(),
                params,
            }
        }
        Value::Call { fun, args } => {
            let ret = match (
                ctx.fun_ret_tys.and_then(|m| m.get(fun).cloned()),
                call_ret.as_mut(),
            ) {
                // Prefer an explicit table entry when present (mono clones / FunRefs).
                (Some(t), _) => t,
                // Otherwise ask the call-site mono / index callback.
                (None, Some(f)) => f(fun, args).unwrap_or(Type::Int),
                (None, None) => Type::Int,
            };
            identity_passthrough_call_ret(ret, fun, args, ctx)
        }
        Value::Builtin {
            name: Builtin::ListParMap,
            args, .. } => Type::List(Box::new(list_par_map_result_elem(args, ctx))),
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
        Value::ClosureCap { as_float: true, .. } => Type::Float,
        Value::ClosureCap { .. } => Type::Int,
        Value::FunRef(name) | Value::AllocClosure { fun: name, .. } => {
            let ret = ctx
                .fun_ret_tys
                .and_then(|m| m.get(name).cloned())
                .unwrap_or(Type::Int);
            let params = ctx
                .fun_param_tys
                .and_then(|m| m.get(name).cloned())
                .unwrap_or_default();
            Type::Fun(params, Box::new(ret), Effect::pure())
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
            let ret = match fun_ty {
                Some(Type::Fun(_, ret, _)) => (**ret).clone(),
                _ => ctx
                    .funref_locals
                    .and_then(|m| m.get(&callee.0))
                    .and_then(|name| ctx.fun_ret_tys.and_then(|m| m.get(name).cloned()))
                    .unwrap_or(Type::Int),
            };
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
            debug_assert!(false, "ICE: Value::Lambda after lift; expected FunRef/AllocClosure");
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
    let is_id = ctx
        .fun_param0_identity
        .is_some_and(|s| s.contains(fun));
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
    Type::List(Box::new(Type::Int))
}

fn list_par_map_result_elem(args: &[Local], ctx: InferValueCtx<'_>) -> Type {
    let list_elem = match list_elem_preserved(args, ctx.local_tys) {
        Type::List(elem) => *elem,
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
            if identity && matches!(list_elem, Type::Float) {
                return Type::Float;
            }
            match &ret {
                Type::Float => return Type::Float,
                // Polymorphic FunRefs keep Int/Var ABI until mono; float source
                // lists must stay List[Float] so fold/map specialize.
                Type::Int | Type::Var(_) if matches!(list_elem, Type::Float) => {
                    return Type::Float;
                }
                Type::Int => return Type::Int,
                Type::Var(_) => return list_elem,
                _ => return ret,
            }
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

mod builtin;
mod join;

pub(crate) use builtin::{
    builtin_value_ty, elems_family_recv_ok, via_gated_recv, via_gated_recv_seeded,
};
pub use join::{join_abi_tys, prefer_concrete_heap_ty, JoinAbiKind};

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
        assert!(type_may_heap(&Type::List(Box::new(Type::Int))));
        assert!(type_may_heap(&Type::Fun(
            vec![Type::Int],
            Box::new(Type::Int),
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
    fn list_append_upgrades_int_elem_to_task() {
        let mut tys = HashMap::default();
        tys.insert(0, Type::List(Box::new(Type::Int)));
        tys.insert(1, Type::Task(Box::new(Type::Float)));
        let t = infer_value_ty(
            &Value::Builtin {
                name: lumia_hir::Builtin::ListAppend,
                args: vec![Local(0), Local(1)],
                    result_ty: None,
                },
            &tys,
            |_, _| None,
        );
        assert_eq!(t, Type::List(Box::new(Type::Task(Box::new(Type::Float)))));
    }

    #[test]
    fn map_set_upgrades_int_key_val_to_bool() {
        // Empty mapOf is Map[Int,Int]; `.set(true, false)` must become Map[Bool,Bool]
        // so println uses lumia_show_map_bool (not {1: 0}).
        let mut tys = HashMap::default();
        tys.insert(0, Type::Map(Box::new(Type::Int), Box::new(Type::Int)));
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
        assert_eq!(
            t,
            Type::Map(Box::new(Type::Bool), Box::new(Type::Bool))
        );
    }

    #[test]
    fn set_insert_upgrades_int_elem_to_bool() {
        let mut tys = HashMap::default();
        tys.insert(0, Type::Set(Box::new(Type::Int)));
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
        assert_eq!(t, Type::Set(Box::new(Type::Bool)));
    }

    #[test]
    fn list_concat_upgrades_int_acc_to_fun_elem() {
        // flatMap empty acc is List[Int]; chunk listOf({…}) is List[Fun].
        let mut tys = HashMap::default();
        tys.insert(0, Type::List(Box::new(Type::Int)));
        tys.insert(
            1,
            Type::List(Box::new(Type::Fun(
                vec![Type::Float],
                Box::new(Type::Float),
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
    fn par_map_elem_ty_from_funref_ret() {
        let mut local_tys = HashMap::default();
        local_tys.insert(0, Type::List(Box::new(Type::Int)));
        local_tys.insert(
            1,
            Type::Fun(vec![Type::Int], Box::new(Type::Float), Effect::pure()),
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
        };
        assert_eq!(
            list_par_map_elem_ty(&[Local(0), Local(1)], ctx),
            Type::Float
        );
    }

    #[test]
    fn adt_field_uses_index_not_params0() {
        let mut local_tys = HashMap::default();
        local_tys.insert(
            0,
            Type::Adt {
                name: "Eco".into(),
                params: vec![Type::Float, Type::Float, Type::List(Box::new(Type::Float))],
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
        assert_eq!(t, Type::List(Box::new(Type::Float)));
    }
}
