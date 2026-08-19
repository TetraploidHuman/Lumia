use super::super::fun_index::FunIndex;
use crate::ir::{Block, CoreFun, Local, Op, Value};
use lumia_hir::Builtin;
use rustc_hash::{FxHashMap as HashMap, FxHashSet};

/// Nested Fun-in-container snapshot for ListGet / AdtField / TaskJoin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FunrefElem {
    Fun(String),
    List(Vec<Option<FunrefElem>>),
    Adt(Vec<Option<FunrefElem>>),
}

pub(super) type FunrefSlots = Vec<Option<FunrefElem>>;

pub(super) fn funref_elem_of_local(
    id: u32,
    funref_of: &HashMap<u32, String>,
    list_funrefs: &HashMap<u32, FunrefSlots>,
    adt_funrefs: &HashMap<u32, FunrefSlots>,
) -> Option<FunrefElem> {
    if let Some(n) = funref_of.get(&id) {
        return Some(FunrefElem::Fun(n.clone()));
    }
    if let Some(v) = list_funrefs.get(&id) {
        return Some(FunrefElem::List(v.clone()));
    }
    if let Some(v) = adt_funrefs.get(&id) {
        return Some(FunrefElem::Adt(v.clone()));
    }
    None
}

pub(super) fn apply_funref_elem(
    local: u32,
    elem: Option<FunrefElem>,
    funref_of: &mut HashMap<u32, String>,
    list_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_funrefs: &mut HashMap<u32, FunrefSlots>,
) {
    match elem {
        Some(FunrefElem::Fun(n)) => {
            funref_of.insert(local, n);
            list_funrefs.remove(&local);
            adt_funrefs.remove(&local);
        }
        Some(FunrefElem::List(v)) => {
            if v.iter().any(|x| x.is_some()) {
                list_funrefs.insert(local, v);
            } else {
                list_funrefs.remove(&local);
            }
            funref_of.remove(&local);
            adt_funrefs.remove(&local);
        }
        Some(FunrefElem::Adt(v)) => {
            if v.iter().any(|x| x.is_some()) {
                adt_funrefs.insert(local, v);
            } else {
                adt_funrefs.remove(&local);
            }
            funref_of.remove(&local);
            list_funrefs.remove(&local);
        }
        None => {
            funref_of.remove(&local);
            list_funrefs.remove(&local);
            adt_funrefs.remove(&local);
        }
    }
}

/// When `ListGet` index is not a const (filter loop), recover a Fun if every
/// present slot names the same function / nested shape.
pub(super) fn homogeneous_funref_elem(slots: &FunrefSlots) -> Option<FunrefElem> {
    let mut found: Option<&FunrefElem> = None;
    for s in slots {
        let Some(e) = s else {
            continue;
        };
        match found {
            None => found = Some(e),
            Some(prev) if prev == e => {}
            _ => return None,
        }
    }
    found.cloned()
}

/// If `fun`'s result is a constant `FunRef` / `AllocClosure`, return that name.
pub(super) fn constant_returned_funref(fun: &str, index: &FunIndex<'_>) -> Option<String> {
    let f = index.get(fun)?;
    constant_returned_funref_in_body(&f.body)
}

pub(super) fn constant_returned_funref_in_body(body: &Block) -> Option<String> {
    let Local(mut cur) = body.result?;
    let mut seen = FxHashSet::default();
    loop {
        if !seen.insert(cur) {
            return None;
        }
        let mut found: Option<&Value> = None;
        for op in &body.ops {
            if let Op::Let { local, value, .. } = op {
                if local.0 == cur {
                    found = Some(value);
                    break;
                }
            }
        }
        match found? {
            Value::Local(Local(src)) => cur = *src,
            Value::FunRef(n) | Value::AllocClosure { fun: n, .. } => return Some(n.name.clone()),
            _ => return None,
        }
    }
}

/// Snapshot before rewrite — FunSig shadow has empty bodies; chase uses this map.
pub(super) fn constant_funref_ret_map(functions: &[CoreFun]) -> HashMap<String, String> {
    let mut out = HashMap::default();
    for f in functions {
        if let Some(n) = constant_returned_funref_in_body(&f.body) {
            out.insert(f.name.to_string(), n);
        }
    }
    out
}

/// Spawn bodies that return `listOf(fun, …)` — elem FunRefs for join→ListGet.
pub(super) fn constant_list_funref_ret_map(functions: &[CoreFun]) -> HashMap<String, FunrefSlots> {
    let mut out = HashMap::default();
    for f in functions {
        if let Some(v) = constant_returned_list_funrefs_in_body(&f.body) {
            out.insert(f.name.to_string(), v);
        }
    }
    out
}

/// Spawn bodies that return `Box { f = fun, … }` — field FunRefs for join→AdtField.
pub(super) fn constant_adt_funref_ret_map(functions: &[CoreFun]) -> HashMap<String, FunrefSlots> {
    let mut out = HashMap::default();
    for f in functions {
        if let Some(v) = constant_returned_adt_funrefs_in_body(&f.body) {
            out.insert(f.name.to_string(), v);
        }
    }
    out
}

pub(super) fn constant_returned_list_funrefs(
    fun: &str,
    index: &FunIndex<'_>,
) -> Option<FunrefSlots> {
    let f = index.get(fun)?;
    constant_returned_list_funrefs_in_body(&f.body)
}

pub(super) fn constant_returned_list_funrefs_in_body(body: &Block) -> Option<FunrefSlots> {
    let Local(mut cur) = body.result?;
    let mut seen = FxHashSet::default();
    loop {
        if !seen.insert(cur) {
            return None;
        }
        let found = def_of(body, cur)?;
        match found {
            Value::Local(Local(src)) => cur = *src,
            Value::AllocList { elems, .. } => {
                let frs: FunrefSlots = elems
                    .iter()
                    .map(|e| chase_local_funref_elem(body, e.0))
                    .collect();
                if frs.iter().any(|x| x.is_some()) {
                    return Some(frs);
                }
                return None;
            }
            _ => return None,
        }
    }
}

pub(super) fn constant_returned_adt_funrefs(
    fun: &str,
    index: &FunIndex<'_>,
) -> Option<FunrefSlots> {
    let f = index.get(fun)?;
    constant_returned_adt_funrefs_in_body(&f.body)
}

pub(super) fn constant_returned_adt_funrefs_in_body(body: &Block) -> Option<FunrefSlots> {
    let Local(mut cur) = body.result?;
    let mut seen = FxHashSet::default();
    loop {
        if !seen.insert(cur) {
            return None;
        }
        let found = def_of(body, cur)?;
        match found {
            Value::Local(Local(src)) => cur = *src,
            Value::AllocAdt { fields, .. } => {
                let frs: FunrefSlots = fields
                    .iter()
                    .map(|e| chase_local_funref_elem(body, e.0))
                    .collect();
                if frs.iter().any(|x| x.is_some()) {
                    return Some(frs);
                }
                return None;
            }
            _ => return None,
        }
    }
}

pub(super) fn def_of(body: &Block, id: u32) -> Option<&Value> {
    for op in &body.ops {
        if let Op::Let { local, value, .. } = op {
            if local.0 == id {
                return Some(value);
            }
        }
    }
    None
}

pub(super) fn chase_local_funref(body: &Block, id: u32) -> Option<String> {
    match chase_local_funref_elem(body, id)? {
        FunrefElem::Fun(n) => Some(n),
        _ => None,
    }
}

/// Chase Fun / nested List / nested Adt funrefs inside a single function body.
pub(super) fn chase_local_funref_elem(body: &Block, id: u32) -> Option<FunrefElem> {
    let mut cur = id;
    let mut seen = FxHashSet::default();
    for _ in 0..24 {
        if !seen.insert(cur) {
            return None;
        }
        match def_of(body, cur)? {
            Value::Local(Local(src)) => cur = *src,
            Value::FunRef(n) | Value::AllocClosure { fun: n, .. } => {
                return Some(FunrefElem::Fun(n.name.clone()));
            }
            Value::AllocList { elems, .. } => {
                let frs: FunrefSlots = elems
                    .iter()
                    .map(|e| chase_local_funref_elem(body, e.0))
                    .collect();
                if frs.iter().any(|x| x.is_some()) {
                    return Some(FunrefElem::List(frs));
                }
                return None;
            }
            Value::AllocAdt { fields, .. } => {
                let frs: FunrefSlots = fields
                    .iter()
                    .map(|e| chase_local_funref_elem(body, e.0))
                    .collect();
                if frs.iter().any(|x| x.is_some()) {
                    return Some(FunrefElem::Adt(frs));
                }
                return None;
            }
            _ => return None,
        }
    }
    None
}

pub(super) fn result_def_is_adt_field(body: &Block) -> bool {
    let Some(Local(mut cur)) = body.result else {
        return false;
    };
    let mut seen = FxHashSet::default();
    for _ in 0..8 {
        if !seen.insert(cur) {
            return false;
        }
        match def_of(body, cur) {
            Some(Value::Local(Local(src))) => cur = *src,
            Some(Value::Builtin {
                name: Builtin::AdtField,
                ..
            }) => return true,
            _ => return false,
        }
    }
    false
}

/// Resolve funref for an If arm result, including `AdtField` of a known ADT.
pub(super) fn chase_arm_funref(
    body: &Block,
    id: u32,
    funref_of: &HashMap<u32, String>,
    adt_funrefs: &HashMap<u32, FunrefSlots>,
    int_consts: &HashMap<u32, i64>,
) -> Option<String> {
    let mut cur = id;
    let mut seen = FxHashSet::default();
    for _ in 0..16 {
        if !seen.insert(cur) {
            return None;
        }
        if let Some(n) = funref_of.get(&cur) {
            return Some(n.clone());
        }
        match def_of(body, cur) {
            Some(Value::Local(Local(src))) => cur = *src,
            Some(Value::FunRef(n) | Value::AllocClosure { fun: n, .. }) => {
                return Some(n.name.clone());
            }
            Some(Value::Builtin {
                name: Builtin::AdtField,
                args,
                ..
            }) if args.len() >= 2 => {
                let idx = int_consts.get(&args[1].0).copied().unwrap_or(-1);
                if idx < 0 {
                    return None;
                }
                return adt_funrefs
                    .get(&args[0].0)
                    .and_then(|v| v.get(idx as usize))
                    .and_then(|o| match o {
                        Some(FunrefElem::Fun(n)) => Some(n.clone()),
                        _ => None,
                    });
            }
            _ => return chase_local_funref(body, cur),
        }
    }
    None
}
