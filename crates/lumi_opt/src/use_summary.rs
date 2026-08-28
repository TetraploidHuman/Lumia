//! Per-local use-pattern summary for §7.1.1 prove→specialize.
//!
//! Only counts that are **statically certain** from Core SSA (no heat guessing).
//! Weak / unknown uses must not drive SortedTree or other speculative reps.

use lumi_core::{for_each_block_dfs, Block, CoreFun, Local, Op, Value};
use lumi_hir::Builtin;
use rustc_hash::FxHashMap as HashMap;

/// How a local is observed after its defining `Let`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalUse {
    pub list_get: u32,
    pub list_append: u32,
    pub list_len: u32,
    /// `MapSet` overloaded for List.set / Map.set / Set paths via dispatch.
    pub map_set: u32,
    pub map_remove: u32,
    pub contains: u32,
    pub set_insert: u32,
    /// Passed to `Call` / `IndirectCall` / returned / assigned to escaping slot — opaque.
    pub opaque: u32,
}

impl LocalUse {
    #[inline]
    pub fn lookup_only_map(&self) -> bool {
        self.map_set == 0
            && self.map_remove == 0
            && self.set_insert == 0
            && self.list_append == 0
            && self.opaque == 0
            && (self.contains + self.list_get) > 0
    }

    #[inline]
    pub fn read_only_list(&self) -> bool {
        self.list_append == 0
            && self.map_set == 0
            && self.opaque == 0
            && (self.list_get + self.list_len + self.contains) > 0
    }
}

/// Summarize builtin / call uses of each SSA local in `fun`.
pub fn summarize_fun(fun: &CoreFun) -> HashMap<Local, LocalUse> {
    let mut out: HashMap<Local, LocalUse> = HashMap::default();
    for_each_block_dfs(&fun.body, &mut |b| {
        collect_block_uses(b, &mut out);
    });
    out
}

fn bump(out: &mut HashMap<Local, LocalUse>, local: Local, f: impl FnOnce(&mut LocalUse)) {
    f(out.entry(local).or_default());
}

fn collect_block_uses(block: &Block, out: &mut HashMap<Local, LocalUse>) {
    if let Some(r) = block.result {
        bump(out, r, |u| u.opaque += 1);
    }
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value } => match value {
                Value::Builtin { name, args } => note_builtin(*name, args, out),
                Value::Call { args, .. } | Value::IndirectCall { args, .. } => {
                    for a in args {
                        bump(out, *a, |u| u.opaque += 1);
                    }
                }
                Value::AllocList { elems, .. } | Value::AllocSet { elems, .. } => {
                    for e in elems {
                        bump(out, *e, |u| u.opaque += 1);
                    }
                }
                Value::AllocMap { flat_pairs, .. } => {
                    for e in flat_pairs {
                        bump(out, *e, |u| u.opaque += 1);
                    }
                }
                Value::AllocAdt { fields, .. } => {
                    for e in fields {
                        bump(out, *e, |u| u.opaque += 1);
                    }
                }
                _ => {}
            },
            Op::Assign { value, .. } | Op::Return { value } => {
                bump(out, *value, |u| u.opaque += 1);
            }
            Op::Break | Op::Continue => {}
        }
    }
}

fn note_builtin(name: Builtin, args: &[Local], out: &mut HashMap<Local, LocalUse>) {
    let Some(recv) = args.first().copied() else {
        for a in args {
            bump(out, *a, |u| u.opaque += 1);
        }
        return;
    };
    let classified = match name {
        Builtin::ListGet => {
            bump(out, recv, |u| u.list_get += 1);
            true
        }
        Builtin::ListLen => {
            bump(out, recv, |u| u.list_len += 1);
            true
        }
        Builtin::ListAppend | Builtin::ListConcat | Builtin::ListTake | Builtin::ListSlice => {
            bump(out, recv, |u| u.list_append += 1);
            true
        }
        Builtin::MapSet => {
            bump(out, recv, |u| u.map_set += 1);
            true
        }
        Builtin::MapRemove => {
            bump(out, recv, |u| u.map_remove += 1);
            true
        }
        Builtin::Contains => {
            bump(out, recv, |u| u.contains += 1);
            true
        }
        Builtin::SetInsert => {
            bump(out, recv, |u| u.set_insert += 1);
            true
        }
        _ => false,
    };
    if classified {
        for a in args.iter().skip(1) {
            bump(out, *a, |u| u.opaque += 1);
        }
    } else {
        for a in args {
            bump(out, *a, |u| u.opaque += 1);
        }
    }
}

/// Collect `Let` RHS by local id (nested If/Loop included).
pub fn collect_let_defs(fun: &CoreFun) -> HashMap<u32, Value> {
    let mut defs = HashMap::default();
    for_each_block_dfs(&fun.body, &mut |b| {
        for op in &b.ops {
            if let Op::Let { local, value, .. } = op {
                defs.insert(local.0, value.clone());
            }
        }
    });
    defs
}

/// Key Hash capability proved from SSA defs (§3.5 / §7.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyHashProof {
    /// Scalars / collections / ADT with `instance Hash`.
    HasHash,
    /// ADT product/sum with **no** `instance Hash` → AssocList forever.
    NoHash,
    /// Cannot prove (open var, field extract, call result, …) → default path.
    Unknown,
}

/// Prove Hash-ability of a map key local. `Unknown` must not select AssocList.
pub fn prove_key_hash(
    key: Local,
    defs: &HashMap<u32, Value>,
    hash_adts: &rustc_hash::FxHashSet<String>,
) -> KeyHashProof {
    prove_key_hash_value(key, defs, hash_adts, 0)
}

fn prove_key_hash_value(
    key: Local,
    defs: &HashMap<u32, Value>,
    hash_adts: &rustc_hash::FxHashSet<String>,
    depth: u32,
) -> KeyHashProof {
    if depth > 32 {
        return KeyHashProof::Unknown;
    }
    let Some(v) = defs.get(&key.0) else {
        return KeyHashProof::Unknown;
    };
    match v {
        Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::AllocList { .. }
        | Value::AllocMap { .. }
        | Value::AllocSet { .. } => KeyHashProof::HasHash,
        Value::AllocAdt { adt_name, .. } => {
            if hash_adts.contains(adt_name) {
                KeyHashProof::HasHash
            } else {
                KeyHashProof::NoHash
            }
        }
        Value::Local(Local(src)) => prove_key_hash_value(Local(*src), defs, hash_adts, depth + 1),
        _ => KeyHashProof::Unknown,
    }
}

/// All map keys prove `NoHash` (and there is at least one key).
pub fn prove_all_keys_no_hash(
    flat_pairs: &[Local],
    defs: &HashMap<u32, Value>,
    hash_adts: &rustc_hash::FxHashSet<String>,
) -> bool {
    if flat_pairs.len() < 2 {
        return false;
    }
    let mut saw = false;
    for (i, k) in flat_pairs.iter().enumerate() {
        if i % 2 != 0 {
            continue;
        }
        saw = true;
        if prove_key_hash(*k, defs, hash_adts) != KeyHashProof::NoHash {
            return false;
        }
    }
    saw
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumi_core::{Block, CoreFun, Op};
    use lumi_ty::{Effect, Type};
    use rustc_hash::FxHashSet as HashSet;

    fn fun(ops: Vec<Op>, result: Option<Local>) -> CoreFun {
        CoreFun {
            name: "f".into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            body: Block {
                params: vec![],
                ops,
                result,
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
        }
    }

    #[test]
    fn counts_list_get_and_append() {
        let f = fun(
            vec![
                Op::Let {
                    local: Local(0),
                    value: Value::AllocList {
                        elems: vec![],
                        repr: lumi_core::ListRepr::HeapList,
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Int(0),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Builtin {
                        name: Builtin::ListGet,
                        args: vec![Local(0), Local(1)],
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(3),
                    value: Value::Builtin {
                        name: Builtin::ListAppend,
                        args: vec![Local(0), Local(1)],
                    },
                    pure_region: true,
                },
            ],
            Some(Local(3)),
        );
        let s = summarize_fun(&f);
        let u = s.get(&Local(0)).copied().unwrap_or_default();
        assert_eq!(u.list_get, 1);
        assert_eq!(u.list_append, 1);
        assert!(!u.read_only_list());
    }

    #[test]
    fn lookup_only_map_pattern() {
        let map_l = Local(0);
        let key_l = Local(1);
        let mut pairs = Vec::new();
        for i in 0..9u32 {
            pairs.push(Local(2 + i * 2));
            pairs.push(Local(3 + i * 2));
        }
        let f = fun(
            vec![
                Op::Let {
                    local: map_l,
                    value: Value::AllocMap {
                        flat_pairs: pairs,
                        repr: lumi_core::MapRepr::HashOrdered,
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: key_l,
                    value: Value::Int(2),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(30),
                    value: Value::Builtin {
                        name: Builtin::ListGet,
                        args: vec![map_l, key_l],
                    },
                    pure_region: true,
                },
            ],
            Some(Local(30)),
        );
        let s = summarize_fun(&f);
        let u = s.get(&map_l).copied().unwrap_or_default();
        assert!(u.lookup_only_map());
    }

    #[test]
    fn prove_adt_key_without_hash() {
        let defs = HashMap::from_iter([(
            0u32,
            Value::AllocAdt {
                adt_name: "Point".into(),
                tag: 0,
                fields: vec![],
                repr: lumi_core::AdtRepr::HeapAdt,
            },
        )]);
        let hash_adts = HashSet::default();
        assert_eq!(
            prove_key_hash(Local(0), &defs, &hash_adts),
            KeyHashProof::NoHash
        );
    }
}
