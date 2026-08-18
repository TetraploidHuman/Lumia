//! Local def / FunRef lookup helpers for ABI refresh.

use crate::ir::{Block, Op, Value};
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) fn infer_local_fun_ty(
    block: &Block,
    id: u32,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
) -> Option<Type> {
    let mut cur = id;
    let mut seen = HashSet::default();
    for _ in 0..16 {
        if !seen.insert(cur) {
            return None;
        }
        match local_def(block, cur)? {
            Value::Local(crate::Local(src)) => cur = *src,
            Value::ClosureCap { index, .. } => return caps.get(index).cloned(),
            Value::FunRef(n) | Value::AllocClosure { fun: n, .. } => {
                return super::super::fun_ty_from_tables_tls(n, fun_ret_tys, fun_param_tys);
            }
            _ => return None,
        }
    }
    None
}

pub(super) fn local_def<'a>(block: &'a Block, id: u32) -> Option<&'a Value> {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                if local.0 == id {
                    return Some(value);
                }
                if let Some(v) = local_def_in_value(value, id) {
                    return Some(v);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn local_def_in_value<'a>(value: &'a Value, id: u32) -> Option<&'a Value> {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => local_def(then_block, id).or_else(|| local_def(else_block, id)),
        Value::Loop {
            header,
            body,
            latch,
        } => local_def(header, id)
            .or_else(|| local_def(body, id))
            .or_else(|| local_def(latch, id)),
        Value::Lambda { body, .. } => local_def(body, id),
        _ => None,
    }
}

pub(super) fn funref_name_of_local(block: &Block, id: u32) -> Option<String> {
    let mut cur = id;
    let mut seen = HashSet::default();
    for _ in 0..16 {
        if !seen.insert(cur) {
            return None;
        }
        match local_def(block, cur)? {
            Value::Local(crate::Local(src)) => cur = *src,
            Value::FunRef(n) | Value::AllocClosure { fun: n, .. } => return Some(n.name.clone()),
            _ => return None,
        }
    }
    None
}
