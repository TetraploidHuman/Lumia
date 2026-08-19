//! Local def / FunRef lookup helpers for ABI refresh.

use crate::find_local_def;
use crate::ir::{Block, Value};
use lumia_hir::Sym;
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) fn infer_local_fun_ty(
    block: &Block,
    id: u32,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
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
                return super::super::fun_ty_from_tables_tls(n.as_str(), fun_ret_tys, fun_param_tys);
            }
            _ => return None,
        }
    }
    None
}

pub(super) fn local_def<'a>(block: &'a Block, id: u32) -> Option<&'a Value> {
    find_local_def(block, id)
}

pub(super) fn funref_name_of_local(block: &Block, id: u32) -> Option<Sym> {
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
