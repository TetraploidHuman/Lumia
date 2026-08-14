//! Shared Core `Value` → [`Type`] / heap-root helpers for mono + codegen.

use crate::{AdtRepr, ListRepr, Local, MapRepr, SetRepr, Value};
use lumia_hir::Builtin;
use lumia_syntax::{BinOp, UnOp};
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Whether Lit* stack allocations count as heap for GC rooting / escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapPolicy {
    /// Escape / lambda analysis: every `Alloc*` is treated as heap.
    Conservative,
    /// Codegen roots: non-empty LitList/LitSet/LitMap/LitAdt live on the stack.
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
        Value::AllocSet { elems, repr } => match policy {
            HeapPolicy::Conservative => true,
            HeapPolicy::StackLitOk => !matches!(repr, SetRepr::LitSet) || elems.is_empty(),
        },
        Value::AllocMap { flat_pairs, repr } => match policy {
            HeapPolicy::Conservative => true,
            HeapPolicy::StackLitOk => !matches!(repr, MapRepr::LitMap) || flat_pairs.is_empty(),
        },
        Value::AllocAdt { repr, .. } => match policy {
            HeapPolicy::Conservative => true,
            HeapPolicy::StackLitOk => !matches!(repr, AdtRepr::LitAdt),
        },
        Value::AllocClosure { .. } | Value::ClosureCap { .. } | Value::FunRef(_) => true,
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
            | BinOp::Ge
            | BinOp::And
            | BinOp::Or => Type::Bool,
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
            if let Some(max) = ctx
                .sum_max_arity
                .and_then(|m| m.get(adt_name).copied())
            {
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
            let ret = if let Some(m) = ctx.fun_ret_tys {
                m.get(fun).cloned().unwrap_or(Type::Int)
            } else if let Some(f) = call_ret.as_mut() {
                f(fun, args).unwrap_or(Type::Int)
            } else {
                Type::Int
            };
            identity_float_call_ret(ret, fun, args, ctx)
        }
        Value::Builtin {
            name: Builtin::ListParMap,
            args,
        } => Type::List(Box::new(list_par_map_result_elem(args, ctx))),
        Value::Builtin { name, args } => builtin_value_ty(*name, args, ctx),
        Value::AllocClosure { .. } | Value::FunRef(_) | Value::ClosureCap { .. } => {
            Type::Fun(vec![], Box::new(Type::Int), Effect::pure())
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
            let ret = match ctx.local_tys.get(&callee.0) {
                Some(Type::Fun(_, ret, _)) => (**ret).clone(),
                _ => ctx
                    .funref_locals
                    .and_then(|m| m.get(&callee.0))
                    .and_then(|name| ctx.fun_ret_tys.and_then(|m| m.get(name).cloned()))
                    .unwrap_or(Type::Int),
            };
            if matches!(&ret, Type::List(e) if matches!(e.as_ref(), Type::Int)) {
                if let Some(name) = ctx.funref_locals.and_then(|m| m.get(&callee.0)) {
                    let ptys = ctx
                        .fun_param_tys
                        .and_then(|m| m.get(name).cloned())
                        .unwrap_or_default();
                    if args.len() == 1
                        && ptys.len() == 1
                        && matches!(ptys[0], Type::Int)
                        && matches!(ctx.local_tys.get(&args[0].0), Some(Type::Float))
                    {
                        return Type::Float;
                    }
                }
            }
            ret
        }
        Value::Loop { .. } | Value::Lambda { .. } => Type::Int,
    }
}

fn identity_float_call_ret(ret: Type, fun: &str, args: &[Local], ctx: InferValueCtx<'_>) -> Type {
    if matches!(&ret, Type::List(e) if matches!(e.as_ref(), Type::Int)) {
        let ptys = ctx
            .fun_param_tys
            .and_then(|m| m.get(fun).cloned())
            .unwrap_or_default();
        if args.len() == 1
            && ptys.len() == 1
            && matches!(ptys[0], Type::Int)
            && matches!(ctx.local_tys.get(&args[0].0), Some(Type::Float))
        {
            return Type::Float;
        }
    }
    ret
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
                Type::Int => return Type::Int,
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
fn adt_field_result_ty(args: &[Local], ctx: InferValueCtx<'_>) -> Type {
    let params: Option<&[Type]> =
        args.first()
            .and_then(|a| ctx.local_tys.get(&a.0))
            .and_then(|t| match t {
                Type::Adt { params, .. } if !params.is_empty() => Some(params.as_slice()),
                Type::Tuple(ts) | Type::TuplePrefix(ts) if !ts.is_empty() => Some(ts.as_slice()),
                _ => None,
            });
    let Some(params) = params else {
        return Type::Int;
    };
    let idx = args
        .get(1)
        .and_then(|a| ctx.local_int_consts.and_then(|m| m.get(&a.0).copied()))
        .unwrap_or(0);
    if idx < 0 {
        return Type::Int;
    }
    params.get(idx as usize).cloned().unwrap_or(Type::Int)
}

/// Merge branch result types for `Value::If` (pad sum ADT params to a shared width).
fn join_value_tys(a: &Type, b: &Type) -> Option<Type> {
    if a == b {
        return Some(a.clone());
    }
    match (a, b) {
        (
            Type::Adt {
                name: n1,
                params: p1,
            },
            Type::Adt {
                name: n2,
                params: p2,
            },
        ) if n1 == n2 => {
            let n = p1.len().max(p2.len());
            let mut params = Vec::with_capacity(n);
            for i in 0..n {
                let x = p1.get(i).cloned().unwrap_or(Type::Int);
                let y = p2.get(i).cloned().unwrap_or(Type::Int);
                params.push(if x == y {
                    x
                } else if matches!(x, Type::Int | Type::Var(_)) {
                    y
                } else if matches!(y, Type::Int | Type::Var(_)) {
                    x
                } else {
                    x
                });
            }
            Some(Type::Adt {
                name: n1.clone(),
                params,
            })
        }
        _ => None,
    }
}

fn builtin_value_ty(name: Builtin, args: &[Local], ctx: InferValueCtx<'_>) -> Type {
    let local_tys = ctx.local_tys;
    match name {
        Builtin::Show
        | Builtin::ReadStdin
        | Builtin::StrTrim
        | Builtin::StrSplit
        | Builtin::StrSubstring
        | Builtin::StrToLower
        | Builtin::StrToUpper
        | Builtin::ListJoin => Type::String,
        Builtin::ListLen | Builtin::AdtTag => Type::Int,
        Builtin::Contains | Builtin::StrStartsWith | Builtin::StrEndsWith => Type::Bool,
        Builtin::Println | Builtin::MatchFail | Builtin::Assert => Type::Unit,
        Builtin::ListGet => args
            .first()
            .and_then(|a| local_tys.get(&a.0))
            .map(|t| match t {
                Type::List(e) | Type::Set(e) => (**e).clone(),
                Type::Map(_, v) => Type::Adt {
                    name: "Option".into(),
                    params: vec![(**v).clone()],
                },
                Type::Adt { name, .. } if name == "Option" => t.clone(),
                _ => Type::Int,
            })
            .unwrap_or(Type::Int),
        Builtin::AdtField => adt_field_result_ty(args, ctx),
        Builtin::ListParFold => args
            .get(1)
            .and_then(|a| local_tys.get(&a.0).cloned())
            .unwrap_or(Type::Int),
        Builtin::ListSlice
        | Builtin::ListTake
        | Builtin::ListReverse
        | Builtin::ListConcat
        | Builtin::ListParMap
        | Builtin::ListSort
        | Builtin::ListSortByKeys => args
            .first()
            .and_then(|a| local_tys.get(&a.0).cloned())
            .unwrap_or(Type::List(Box::new(Type::Int))),
        Builtin::ListAppend => {
            let list_ty = args
                .first()
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::List(Box::new(Type::Int)));
            // Empty `listOf()` starts as List[Int]; appending a Float must
            // upgrade so later ListGet / println / == see Float ABI (and
            // `ensure_list_f64` already retags the runtime object).
            match (
                &list_ty,
                args.get(1).and_then(|a| local_tys.get(&a.0)),
            ) {
                (Type::List(_), Some(Type::Float)) => Type::List(Box::new(Type::Float)),
                _ => list_ty,
            }
        }
        Builtin::Elems => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::List(e) | Type::Set(e)) => Type::List(e.clone()),
            Some(Type::Map(k, _)) => Type::List(k.clone()),
            _ => Type::List(Box::new(Type::Int)),
        },
        Builtin::MapKeys => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::Map(k, _)) => Type::List(k.clone()),
            _ => Type::List(Box::new(Type::Int)),
        },
        Builtin::MapValues => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::Map(_, v)) => Type::List(v.clone()),
            _ => Type::List(Box::new(Type::Int)),
        },
        Builtin::Range | Builtin::RangeInclusive => Type::List(Box::new(Type::Int)),
        Builtin::MapItems => match args.first().and_then(|a| local_tys.get(&a.0)) {
            Some(Type::Map(k, v)) => Type::List(Box::new(Type::Adt {
                name: "__Tuple".into(),
                params: vec![(**k).clone(), (**v).clone()],
            })),
            Some(Type::List(elem)) => Type::List(elem.clone()),
            _ => Type::List(Box::new(Type::Adt {
                name: "__Tuple".into(),
                params: vec![Type::Int, Type::Int],
            })),
        },
        Builtin::MapSet => {
            let key_ty = args
                .get(1)
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::Int);
            let val_ty = args
                .get(2)
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::Int);
            match args.first().and_then(|a| local_tys.get(&a.0)) {
                Some(Type::List(e)) => {
                    let elem = if matches!(val_ty, Type::Float) {
                        Type::Float
                    } else {
                        (**e).clone()
                    };
                    Type::List(Box::new(elem))
                }
                Some(Type::Map(k, v)) => {
                    let k = if matches!(key_ty, Type::Float) {
                        Box::new(Type::Float)
                    } else {
                        k.clone()
                    };
                    let v = if matches!(val_ty, Type::Float) {
                        Box::new(Type::Float)
                    } else {
                        v.clone()
                    };
                    Type::Map(k, v)
                }
                // Free / poly: Int key ⇒ list index update (not Map).
                _ if matches!(key_ty, Type::Int) => {
                    Type::List(Box::new(if matches!(val_ty, Type::Float) {
                        Type::Float
                    } else {
                        val_ty
                    }))
                }
                _ => Type::Map(Box::new(key_ty), Box::new(val_ty)),
            }
        }
        Builtin::MapRemove => {
            let key_ty = args
                .get(1)
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::Int);
            match args.first().and_then(|a| local_tys.get(&a.0)) {
                Some(Type::List(e)) => Type::List(e.clone()),
                Some(Type::Map(k, v)) => {
                    let k = if matches!(key_ty, Type::Float) {
                        Box::new(Type::Float)
                    } else {
                        k.clone()
                    };
                    Type::Map(k, v.clone())
                }
                _ => Type::Map(Box::new(key_ty), Box::new(Type::Int)),
            }
        }
        Builtin::SetInsert => {
            let elem_ty = args
                .get(1)
                .and_then(|a| local_tys.get(&a.0).cloned())
                .unwrap_or(Type::Int);
            match args.first().and_then(|a| local_tys.get(&a.0)) {
                Some(Type::Set(e)) => {
                    if matches!(elem_ty, Type::Float) {
                        Type::Set(Box::new(Type::Float))
                    } else {
                        Type::Set(e.clone())
                    }
                }
                _ => Type::Set(Box::new(elem_ty)),
            }
        }
    }
}

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
    fn float_binary_promotes() {
        let mut tys = HashMap::default();
        tys.insert(0, Type::Float);
        tys.insert(1, Type::Int);
        let t = infer_value_ty(
            &Value::Binary {
                op: BinOp::Add,
                left: Local(0),
                right: Local(1),
            },
            &tys,
            |_, _| None,
        );
        assert_eq!(t, Type::Float);
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
        };
        let t = infer_value_ty_ctx(
            &Value::Builtin {
                name: Builtin::AdtField,
                args: vec![Local(0), Local(1)],
            },
            ctx,
            None,
        );
        assert_eq!(t, Type::List(Box::new(Type::Float)));
    }
}
