//! Infer channel payload hints from `ChannelSend` sites.
//!
//! Prefer ground `Channel[T]` stamped onto `ChannelNew` from HIR `type_at`.
//! When a stamp is missing, recover payload from agreeing `ChannelSend` sites
//! (Float / lists / ADTs) instead of erased Int.
//!
//! Per-channel map: locals are unique after lift (`max_local` monotonic), so
//! `ChannelNew` local ids key [`CoreModule::channel_elem_by_local`]. The module
//! hint is set only when every channel agrees on the same payload.

use super::float_abi::{collect_fun_cap_tys, compute_float_locals_in_block};
use crate::ir::{Block, CoreModule, Local, Op, Value};
use crate::visit::for_each_nested_block;
use lumia_hir::Builtin;
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(crate) fn refine_channel_elem_hint(module: &mut CoreModule) {
    let mut lam_caps: HashMap<String, Vec<Local>> = HashMap::default();
    for fun in &module.functions {
        collect_alloc_closure_caps(&fun.body, &mut lam_caps);
    }

    let fun_ret_tys: HashMap<String, Type> = module
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.ret_ty.clone()))
        .collect();
    let fun_param_tys: HashMap<String, Vec<Type>> = module
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.param_tys.clone()))
        .collect();
    // Spawn thunks may be scanned before their AllocClosure site; cap tys are
    // collected with a fixpoint so `ch.send(f)` via ClosureCap keeps Fun ABI
    // instead of falling back to Int (mixed-payload reject).
    let fun_cap_tys = collect_fun_cap_tys(module, &fun_ret_tys, &fun_param_tys);

    let mut root_of: HashMap<u32, u32> = HashMap::default();
    let mut by_ch: HashMap<u32, Option<Type>> = HashMap::default();
    let mut poisoned_ch: HashSet<u32> = HashSet::default();
    let mut conflicts: Vec<(Type, Type)> = Vec::new();

    // Pass 1: register every `ChannelNew` before following ClosureCap aliases.
    // Spawn thunks are often listed before their AllocClosure site; a single
    // forward scan would miss sends through captured channels.
    for fun in &module.functions {
        register_channel_news(&fun.body, &mut root_of, &mut by_ch);
    }
    // Pass 2: fixpoint Local / ClosureCap → ChannelNew root.
    loop {
        let mut changed = false;
        for fun in &module.functions {
            let caps = lam_caps.get(&fun.name).map(|c| c.as_slice());
            changed |= propagate_channel_aliases(&fun.body, &mut root_of, caps);
        }
        if !changed {
            break;
        }
    }
    // Pass 3: attribute sends (payload tys) onto roots.
    for fun in &module.functions {
        let caps = lam_caps.get(&fun.name).cloned();
        let mut local_tys: HashMap<u32, Type> = HashMap::default();
        for (p, ty) in fun.params.iter().zip(fun.param_tys.iter()) {
            local_tys.insert(p.0, ty.clone());
        }
        scan_block(
            &fun.body,
            &fun.name,
            &mut local_tys,
            &mut root_of,
            &mut by_ch,
            &mut poisoned_ch,
            &mut conflicts,
            caps.as_deref(),
            &fun_ret_tys,
            &fun_param_tys,
            &fun_cap_tys,
        );
    }

    let mut agreed: Option<Type> = None;
    let mut module_poisoned = false;
    let mut per = HashMap::default();
    for (root, slot) in by_ch {
        if poisoned_ch.contains(&root) {
            module_poisoned = true;
            continue;
        }
        if let Some(ty) = slot {
            per.insert(root, ty.clone());
            match &agreed {
                None => agreed = Some(ty),
                Some(prev) if *prev == ty => {}
                Some(_) => module_poisoned = true,
            }
        }
    }
    module.channel_elem_by_local = per;
    module.channel_elem_hint = if module_poisoned { None } else { agreed };
    module.channel_elem_conflicts = conflicts;
}

fn register_channel_news(
    block: &Block,
    root_of: &mut HashMap<u32, u32>,
    by_ch: &mut HashMap<u32, Option<Type>>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                if let Value::Builtin {
                    name: Builtin::ChannelNew,
                    result_ty,
                    ..
                } = value
                {
                    root_of.insert(local.0, local.0);
                    let seed = channel_new_elem_ty(result_ty);
                    by_ch.entry(local.0).or_insert(seed);
                }
                for_each_nested_block(value, &mut |b| {
                    register_channel_news(b, root_of, by_ch);
                });
            }
            _ => {}
        }
    }
}

fn channel_new_elem_ty(result_ty: &Option<Type>) -> Option<Type> {
    match result_ty {
        Some(Type::Channel(e)) => Some((**e).clone()),
        _ => None,
    }
}

fn propagate_channel_aliases(
    block: &Block,
    root_of: &mut HashMap<u32, u32>,
    caps: Option<&[Local]>,
) -> bool {
    let mut changed = false;
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                changed |= note_channel_alias(local.0, value, root_of, caps);
                for_each_nested_block(value, &mut |b| {
                    changed |= propagate_channel_aliases(b, root_of, caps);
                });
            }
            _ => {}
        }
    }
    changed
}

fn note_channel_alias(
    local: u32,
    value: &Value,
    root_of: &mut HashMap<u32, u32>,
    caps: Option<&[Local]>,
) -> bool {
    let root = match value {
        Value::Local(Local(src)) => root_of.get(src).copied(),
        Value::ClosureCap { index, .. } => caps
            .and_then(|c| c.get(*index as usize))
            .and_then(|outer| root_of.get(&outer.0).copied()),
        _ => None,
    };
    let Some(r) = root else {
        return false;
    };
    match root_of.get(&local).copied() {
        Some(prev) if prev == r => false,
        _ => {
            root_of.insert(local, r);
            true
        }
    }
}

fn collect_alloc_closure_caps(block: &Block, lam_caps: &mut HashMap<String, Vec<Local>>) {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } => {
                if let Value::AllocClosure { fun, captures } = value {
                    lam_caps.insert(fun.clone(), captures.clone());
                }
                for_each_nested_block(value, &mut |b| {
                    collect_alloc_closure_caps(b, lam_caps);
                });
            }
            _ => {}
        }
    }
}

fn scan_block(
    block: &Block,
    fun_name: &str,
    local_tys: &mut HashMap<u32, Type>,
    root_of: &mut HashMap<u32, u32>,
    by_ch: &mut HashMap<u32, Option<Type>>,
    poisoned_ch: &mut HashSet<u32>,
    conflicts: &mut Vec<(Type, Type)>,
    caps: Option<&[Local]>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    fun_cap_tys: &HashMap<String, HashMap<u32, Type>>,
) {
    let float_locals = compute_float_locals_in_block(block);
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                note_channel_root(local.0, value, root_of, by_ch, caps);
                note_send(
                    value,
                    local_tys,
                    root_of,
                    by_ch,
                    poisoned_ch,
                    conflicts,
                );
                local_tys.insert(
                    local.0,
                    guess_local_ty(
                        value,
                        fun_name,
                        local_tys,
                        &float_locals,
                        fun_ret_tys,
                        fun_param_tys,
                        fun_cap_tys,
                    ),
                );
                for_each_nested_block(value, &mut |b| {
                    scan_block(
                        b,
                        fun_name,
                        local_tys,
                        root_of,
                        by_ch,
                        poisoned_ch,
                        conflicts,
                        caps,
                        fun_ret_tys,
                        fun_param_tys,
                        fun_cap_tys,
                    );
                });
            }
            _ => {}
        }
    }
}

fn note_channel_root(
    local: u32,
    value: &Value,
    root_of: &mut HashMap<u32, u32>,
    by_ch: &mut HashMap<u32, Option<Type>>,
    caps: Option<&[Local]>,
) {
    match value {
        Value::Builtin {
            name: Builtin::ChannelNew,
            result_ty,
            ..
        } => {
            root_of.insert(local, local);
            let seed = channel_new_elem_ty(result_ty);
            by_ch.entry(local).or_insert(seed);
        }
        Value::Local(Local(src)) => {
            if let Some(r) = root_of.get(src).copied() {
                root_of.insert(local, r);
            }
        }
        Value::ClosureCap { index, .. } => {
            if let Some(outer) = caps.and_then(|c| c.get(*index as usize)) {
                if let Some(r) = root_of.get(&outer.0).copied() {
                    root_of.insert(local, r);
                }
            }
        }
        _ => {}
    }
}

fn note_send(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    root_of: &HashMap<u32, u32>,
    by_ch: &mut HashMap<u32, Option<Type>>,
    poisoned_ch: &mut HashSet<u32>,
    conflicts: &mut Vec<(Type, Type)>,
) {
    let Value::Builtin {
        name: Builtin::ChannelSend,
        args, .. } = value
    else {
        return;
    };
    let (Some(ch), Some(payload)) = (args.first(), args.get(1)) else {
        return;
    };
    let Some(ty) = local_tys.get(&payload.0).cloned() else {
        return;
    };
    // Record Int too: skipping it let `send(1); send(1.5)` / `send(1); send("x")`
    // keep only the concrete hint, so recv printed IEEE bits / wrong ABI.
    let Some(root) = root_of.get(&ch.0).copied() else {
        return;
    };
    if poisoned_ch.contains(&root) {
        return;
    }
    match by_ch.entry(root).or_insert(None) {
        slot @ None => *slot = Some(ty),
        Some(prev) => match join_channel_payload(prev, &ty) {
            Some(joined) => *prev = joined,
            None => {
                conflicts.push((prev.clone(), ty));
                poisoned_ch.insert(root);
                by_ch.insert(root, None);
            }
        },
    }
}

fn guess_local_ty(
    value: &Value,
    fun_name: &str,
    local_tys: &HashMap<u32, Type>,
    float_locals: &HashSet<u32>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    fun_cap_tys: &HashMap<String, HashMap<u32, Type>>,
) -> Type {
    match value {
        Value::Float(_) => Type::Float,
        Value::Int(_) => Type::Int,
        Value::Bool(_) => Type::Bool,
        Value::String(_) => Type::String,
        Value::Char(_) => Type::Char,
        Value::Local(Local(id)) => local_tys.get(id).cloned().unwrap_or(Type::Int),
        Value::ClosureCap {
            as_float: true,
            ..
        } => Type::Float,
        Value::ClosureCap { index, .. } => fun_cap_tys
            .get(fun_name)
            .and_then(|m| m.get(index).cloned())
            .unwrap_or(Type::Int),
        Value::FunRef(name) | Value::AllocClosure { fun: name, .. } => {
            fun_ty_from_tables(name, fun_ret_tys, fun_param_tys)
                .unwrap_or(Type::Int)
        }
        Value::Builtin {
            name: Builtin::TaskSpawn,
            args, .. } if !args.is_empty() => {
            let elem = match local_tys.get(&args[0].0) {
                Some(Type::Fun(_, r, _)) => (**r).clone(),
                _ => Type::Int,
            };
            Type::Task(Box::new(elem))
        }
        Value::Binary { op, left, right } => match op {
            lumia_syntax::BinOp::Eq
            | lumia_syntax::BinOp::Ne
            | lumia_syntax::BinOp::Lt
            | lumia_syntax::BinOp::Le
            | lumia_syntax::BinOp::Gt
            | lumia_syntax::BinOp::Ge => Type::Bool,
            lumia_syntax::BinOp::And | lumia_syntax::BinOp::Or => {
                debug_assert!(false, "ICE: BinOp::And|Or in Core; expected If desugar");
                Type::Bool
            }
            lumia_syntax::BinOp::Add
            | lumia_syntax::BinOp::Sub
            | lumia_syntax::BinOp::Mul
            | lumia_syntax::BinOp::Div
            | lumia_syntax::BinOp::Rem
                if float_locals.contains(&left.0) || float_locals.contains(&right.0) =>
            {
                Type::Float
            }
            _ => Type::Int,
        },
        Value::Unary {
            op: lumia_syntax::UnOp::Not,
            ..
        } => Type::Bool,
        Value::Unary {
            op: lumia_syntax::UnOp::Neg,
            operand,
        } => {
            if float_locals.contains(&operand.0) {
                Type::Float
            } else {
                Type::Int
            }
        }
        Value::AllocList { elems, .. } => {
            if !elems.is_empty() && elems.iter().all(|e| float_locals.contains(&e.0)) {
                Type::List(Box::new(Type::Float))
            } else {
                Type::List(Box::new(guess_elems_ty(elems, local_tys)))
            }
        }
        Value::AllocSet { elems, .. } => {
            if !elems.is_empty() && elems.iter().all(|e| float_locals.contains(&e.0)) {
                Type::Set(Box::new(Type::Float))
            } else {
                Type::Set(Box::new(guess_elems_ty(elems, local_tys)))
            }
        }
        Value::AllocMap { flat_pairs, .. } => {
            let (k, v) = if flat_pairs.len() >= 2 {
                (
                    if float_locals.contains(&flat_pairs[0].0) {
                        Type::Float
                    } else {
                        local_tys
                            .get(&flat_pairs[0].0)
                            .cloned()
                            .unwrap_or(Type::Int)
                    },
                    if float_locals.contains(&flat_pairs[1].0) {
                        Type::Float
                    } else {
                        local_tys
                            .get(&flat_pairs[1].0)
                            .cloned()
                            .unwrap_or(Type::Int)
                    },
                )
            } else {
                (Type::Int, Type::Int)
            };
            Type::Map(Box::new(k), Box::new(v))
        }
        Value::AllocAdt {
            adt_name, tag, fields, ..
        } => adt_payload_ty(adt_name, *tag, fields, local_tys, float_locals),
        Value::Builtin {
            name: Builtin::ChannelNew,
            result_ty,
            ..
        } => match result_ty {
            Some(Type::Channel(e)) => Type::Channel(e.clone()),
            Some(other) => other.clone(),
            None => Type::Channel(Box::new(Type::Int)),
        },
        _ => Type::Int,
    }
}

/// Constructor fields are not type params — rebuild Option[T] / Result[A,B] shape.
fn adt_payload_ty(
    adt_name: &str,
    tag: i64,
    fields: &[Local],
    local_tys: &HashMap<u32, Type>,
    float_locals: &HashSet<u32>,
) -> Type {
    let field_ty = |f: &Local| -> Type {
        if float_locals.contains(&f.0) {
            Type::Float
        } else {
            local_tys.get(&f.0).cloned().unwrap_or(Type::Int)
        }
    };
    // Prelude tags: Some=0 None=1; Ok=0 Err=1 (`ensure_prelude_adt`).
    if adt_name == "Option" {
        let param = if tag == 1 || fields.is_empty() {
            // None — flexible param joined with Some(T) later.
            Type::Var(u32::MAX)
        } else {
            field_ty(&fields[0])
        };
        return Type::Adt {
            name: adt_name.into(),
            params: vec![param],
        };
    }
    if adt_name == "Result" {
        let payload = fields.first().map(field_ty).unwrap_or(Type::Int);
        let (ok, err) = if tag == 0 {
            (payload, Type::Var(u32::MAX))
        } else {
            (Type::Var(u32::MAX), payload)
        };
        return Type::Adt {
            name: adt_name.into(),
            params: vec![ok, err],
        };
    }
    Type::Adt {
        name: adt_name.into(),
        params: fields.iter().map(field_ty).collect(),
    }
}

/// Merge two channel payload types; `None` = hard conflict.
fn join_channel_payload(prev: &Type, new: &Type) -> Option<Type> {
    if prev == new {
        return Some(prev.clone());
    }
    match (prev, new) {
        (
            Type::Adt {
                name: n1,
                params: p1,
            },
            Type::Adt {
                name: n2,
                params: p2,
            },
        ) if n1 == n2 && (n1 == "Option" || n1 == "Result") => {
            let want = if n1.as_str() == "Option" { 1 } else { 2 };
            let mut params = Vec::with_capacity(want);
            for i in 0..want {
                let a = p1.get(i);
                let b = p2.get(i);
                match join_option_param(a, b) {
                    Some(t) => params.push(t),
                    None => return None,
                }
            }
            Some(Type::Adt {
                name: n1.clone(),
                params,
            })
        }
        _ => {
            // Soft scalar refine (Int/Var → concrete).
            let joined = prefer_payload_ty(prev.clone(), new.clone());
            if &joined == prev || &joined == new || joined == prefer_payload_ty(new.clone(), prev.clone())
            {
                // Only accept if prefer actually picked one side without hard mismatch.
                if is_flexible_ty(prev) || is_flexible_ty(new) || prev == new {
                    return Some(joined);
                }
            }
            None
        }
    }
}

fn is_flexible_ty(t: &Type) -> bool {
    matches!(t, Type::Var(_) | Type::Int)
}

fn join_option_param(a: Option<&Type>, b: Option<&Type>) -> Option<Type> {
    match (a, b) {
        (None, None) => Some(Type::Var(u32::MAX)),
        (Some(t), None) | (None, Some(t)) => Some(t.clone()),
        (Some(x), Some(y)) if x == y => Some(x.clone()),
        (Some(x), Some(y)) if is_flexible_ty(x) => Some(y.clone()),
        (Some(x), Some(y)) if is_flexible_ty(y) => Some(x.clone()),
        (Some(Type::Float), Some(Type::Int)) | (Some(Type::Int), Some(Type::Float)) => {
            Some(Type::Float)
        }
        _ => None,
    }
}

fn prefer_payload_ty(a: Type, b: Type) -> Type {
    if a == b {
        return a;
    }
    match (&a, &b) {
        (Type::Float, _) | (_, Type::Float) => Type::Float,
        (Type::Int | Type::Var(_), other) => other.clone(),
        (other, Type::Int | Type::Var(_)) => other.clone(),
        _ => a,
    }
}

fn guess_elems_ty(elems: &[Local], local_tys: &HashMap<u32, Type>) -> Type {
    let mut acc: Option<Type> = None;
    for e in elems {
        let t = local_tys.get(&e.0).cloned().unwrap_or(Type::Int);
        acc = Some(match acc {
            None => t,
            Some(prev) if prev == t => prev,
            Some(prev) => prefer_payload_ty(prev, t),
        });
    }
    acc.unwrap_or(Type::Int)
}

fn fun_ty_from_tables(
    name: &str,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
) -> Option<Type> {
    super::fun_ty_from_tables(name, fun_ret_tys, fun_param_tys, &HashSet::default())
}

#[cfg(test)]
#[path = "channel_hint_tests.rs"]
mod tests;
