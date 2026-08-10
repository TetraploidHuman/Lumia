use lumia_core::{AdtRepr, Block, ListRepr, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::{BinOp, UnOp};
use rustc_hash::FxHashMap as HashMap;

use super::cse::rewrite_value;

pub(crate) fn const_fold_block(block: &mut Block) {
    let mut known_int: HashMap<u32, i64> = HashMap::default();
    // Local → element locals of a literal `AllocList` (for ListLen / ListGet fold).
    let mut known_list: HashMap<u32, Vec<Local>> = HashMap::default();
    // Local → field locals of a literal `AllocAdt` (for AdtField fold).
    let mut known_adt: HashMap<u32, Vec<Local>> = HashMap::default();
    // Local → tag of a literal `AllocAdt` (for AdtTag fold).
    let mut known_adt_tag: HashMap<u32, i64> = HashMap::default();
    // Local → flat key/value locals of a literal `AllocMap`.
    let mut known_map: HashMap<u32, Vec<Local>> = HashMap::default();
    // Local → element locals of a literal `AllocSet`.
    let mut known_set: HashMap<u32, Vec<Local>> = HashMap::default();
    for op in &mut block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                match value {
                    Value::Int(n) => {
                        known_int.insert(local.0, *n);
                    }
                    Value::Bool(b) => {
                        known_int.insert(local.0, if *b { 1 } else { 0 });
                    }
                    Value::Local(Local(src)) => {
                        // Track constants through aliases; keep Local for CSE sharing.
                        if let Some(&n) = known_int.get(src) {
                            known_int.insert(local.0, n);
                        }
                        if let Some(elems) = known_list.get(src).cloned() {
                            known_list.insert(local.0, elems);
                        }
                        if let Some(fields) = known_adt.get(src).cloned() {
                            known_adt.insert(local.0, fields);
                        }
                        if let Some(&tag) = known_adt_tag.get(src) {
                            known_adt_tag.insert(local.0, tag);
                        }
                        if let Some(pairs) = known_map.get(src).cloned() {
                            known_map.insert(local.0, pairs);
                        }
                        if let Some(elems) = known_set.get(src).cloned() {
                            known_set.insert(local.0, elems);
                        }
                    }
                    Value::AllocList { elems, .. } => {
                        known_list.insert(local.0, elems.clone());
                    }
                    Value::AllocMap { flat_pairs, .. } => {
                        known_map.insert(local.0, flat_pairs.clone());
                    }
                    Value::AllocSet { elems, .. } => {
                        known_set.insert(local.0, elems.clone());
                    }
                    Value::AllocAdt { tag, fields, .. } => {
                        known_adt.insert(local.0, fields.clone());
                        known_adt_tag.insert(local.0, *tag);
                    }
                    Value::Unary {
                        op: UnOp::Neg,
                        operand,
                    } => {
                        if let Some(&n) = known_int.get(&operand.0) {
                            if let Some(r) = n.checked_neg() {
                                *value = Value::Int(r);
                                known_int.insert(local.0, r);
                            }
                            // Overflow (i64::MIN): leave Neg for runtime trap.
                        }
                    }
                    Value::Unary {
                        op: UnOp::Not,
                        operand,
                    } => {
                        if let Some(&n) = known_int.get(&operand.0) {
                            let r = n == 0;
                            *value = Value::Bool(r);
                            known_int.insert(local.0, if r { 1 } else { 0 });
                        }
                    }
                    Value::Binary { op, left, right } => {
                        if let (Some(&a), Some(&b)) =
                            (known_int.get(&left.0), known_int.get(&right.0))
                        {
                            if let Some(r) = fold_bin(*op, a, b) {
                                // Keep Bool for cmp/logic so println / ABI typing stay correct.
                                *value = if matches!(
                                    op,
                                    BinOp::Eq
                                        | BinOp::Ne
                                        | BinOp::Lt
                                        | BinOp::Le
                                        | BinOp::Gt
                                        | BinOp::Ge
                                        | BinOp::And
                                        | BinOp::Or
                                ) {
                                    Value::Bool(r != 0)
                                } else {
                                    Value::Int(r)
                                };
                                known_int.insert(local.0, r);
                            }
                        }
                    }
                    Value::Builtin { name, args } => match (*name, args.as_slice()) {
                        (Builtin::ListLen, [xs]) => {
                            if let Some(elems) = known_list.get(&xs.0) {
                                let n = elems.len() as i64;
                                *value = Value::Int(n);
                                known_int.insert(local.0, n);
                            } else if let Some(pairs) = known_map.get(&xs.0) {
                                let n = (pairs.len() / 2) as i64;
                                *value = Value::Int(n);
                                known_int.insert(local.0, n);
                            } else if let Some(elems) = known_set.get(&xs.0) {
                                let n = elems.len() as i64;
                                *value = Value::Int(n);
                                known_int.insert(local.0, n);
                            }
                        }
                        (Builtin::ListGet, [xs, idx]) => {
                            if let (Some(elems), Some(&i)) =
                                (known_list.get(&xs.0), known_int.get(&idx.0))
                            {
                                if i >= 0 && (i as usize) < elems.len() {
                                    let el = elems[i as usize];
                                    *value = Value::Local(el);
                                    if let Some(&n) = known_int.get(&el.0) {
                                        known_int.insert(local.0, n);
                                    }
                                    if let Some(inner) = known_list.get(&el.0).cloned() {
                                        known_list.insert(local.0, inner);
                                    }
                                }
                            } else if let (Some(pairs), Some(&k)) =
                                (known_map.get(&xs.0), known_int.get(&idx.0))
                            {
                                // Map.get → Option: only when every key is a known Int
                                // (same discipline as Contains — avoid false None).
                                let keys: Vec<_> = pairs.chunks_exact(2).map(|kv| kv[0]).collect();
                                if keys.iter().all(|kk| known_int.contains_key(&kk.0)) {
                                    let found = keys.iter().enumerate().find_map(|(i, kk)| {
                                        if known_int.get(&kk.0).copied() == Some(k) {
                                            Some(pairs[i * 2 + 1])
                                        } else {
                                            None
                                        }
                                    });
                                    // Prelude Option: Some = tag 0, None = tag 1.
                                    match found {
                                        Some(v) => {
                                            *value = Value::AllocAdt {
                                                adt_name: "Option".into(),
                                                tag: 0,
                                                fields: vec![v],
                                                repr: AdtRepr::LitAdt,
                                            };
                                            known_adt.insert(local.0, vec![v]);
                                        }
                                        None => {
                                            *value = Value::AllocAdt {
                                                adt_name: "Option".into(),
                                                tag: 1,
                                                fields: vec![],
                                                repr: AdtRepr::LitAdt,
                                            };
                                            known_adt.insert(local.0, vec![]);
                                        }
                                    }
                                }
                            }
                        }
                        (Builtin::Contains, [col, key]) => {
                            // Only fold when every key/elem is a known Int constant.
                            // A non-constant key that happens to equal `k` at runtime
                            // must not be folded to `false` (false negative).
                            if let Some(&k) = known_int.get(&key.0) {
                                if let Some(pairs) = known_map.get(&col.0) {
                                    let keys: Vec<_> =
                                        pairs.chunks_exact(2).map(|kv| kv[0]).collect();
                                    if keys.iter().all(|kk| known_int.contains_key(&kk.0)) {
                                        let found = keys
                                            .iter()
                                            .any(|kk| known_int.get(&kk.0).copied() == Some(k));
                                        *value = Value::Bool(found);
                                        known_int.insert(local.0, if found { 1 } else { 0 });
                                    }
                                } else if let Some(elems) = known_set.get(&col.0) {
                                    if elems.iter().all(|e| known_int.contains_key(&e.0)) {
                                        let found = elems
                                            .iter()
                                            .any(|e| known_int.get(&e.0).copied() == Some(k));
                                        *value = Value::Bool(found);
                                        known_int.insert(local.0, if found { 1 } else { 0 });
                                    }
                                }
                            }
                        }
                        (Builtin::AdtField, [adt, idx, ..]) => {
                            if let (Some(fields), Some(&i)) =
                                (known_adt.get(&adt.0), known_int.get(&idx.0))
                            {
                                if i >= 0 && (i as usize) < fields.len() {
                                    let el = fields[i as usize];
                                    *value = Value::Local(el);
                                    if let Some(&n) = known_int.get(&el.0) {
                                        known_int.insert(local.0, n);
                                    }
                                    if let Some(inner) = known_list.get(&el.0).cloned() {
                                        known_list.insert(local.0, inner);
                                    }
                                    if let Some(inner) = known_adt.get(&el.0).cloned() {
                                        known_adt.insert(local.0, inner);
                                    }
                                    if let Some(&tag) = known_adt_tag.get(&el.0) {
                                        known_adt_tag.insert(local.0, tag);
                                    }
                                }
                            }
                        }
                        (Builtin::AdtTag, [adt]) => {
                            if let Some(&tag) = known_adt_tag.get(&adt.0) {
                                *value = Value::Int(tag);
                                known_int.insert(local.0, tag);
                            }
                        }
                        (Builtin::ListConcat, [a, b]) => {
                            if let (Some(la), Some(lb)) =
                                (known_list.get(&a.0), known_list.get(&b.0))
                            {
                                let mut merged = la.clone();
                                merged.extend_from_slice(lb);
                                *value = Value::AllocList {
                                    elems: merged.clone(),
                                    repr: ListRepr::LitList,
                                };
                                known_list.insert(local.0, merged);
                            }
                        }
                        (Builtin::ListAppend, [xs, x]) => {
                            if let Some(elems) = known_list.get(&xs.0) {
                                let mut merged = elems.clone();
                                merged.push(*x);
                                *value = Value::AllocList {
                                    elems: merged.clone(),
                                    repr: ListRepr::LitList,
                                };
                                known_list.insert(local.0, merged);
                            }
                        }
                        (Builtin::ListTake, [xs, n]) => {
                            if let (Some(elems), Some(&k)) =
                                (known_list.get(&xs.0), known_int.get(&n.0))
                            {
                                if k >= 0 {
                                    let take: Vec<_> =
                                        elems.iter().take(k as usize).copied().collect();
                                    *value = Value::AllocList {
                                        elems: take.clone(),
                                        repr: ListRepr::LitList,
                                    };
                                    known_list.insert(local.0, take);
                                }
                            }
                        }
                        (Builtin::ListSlice, [xs, n]) => {
                            // `slice`/`drop`: drop the first `n` elements.
                            if let (Some(elems), Some(&k)) =
                                (known_list.get(&xs.0), known_int.get(&n.0))
                            {
                                if k >= 0 {
                                    let drop_n = (k as usize).min(elems.len());
                                    let rest: Vec<_> = elems[drop_n..].to_vec();
                                    *value = Value::AllocList {
                                        elems: rest.clone(),
                                        repr: ListRepr::LitList,
                                    };
                                    known_list.insert(local.0, rest);
                                }
                            }
                        }
                        (Builtin::ListReverse, [xs]) => {
                            if let Some(elems) = known_list.get(&xs.0) {
                                let mut rev = elems.clone();
                                rev.reverse();
                                *value = Value::AllocList {
                                    elems: rev.clone(),
                                    repr: ListRepr::LitList,
                                };
                                known_list.insert(local.0, rev);
                            }
                        }
                        _ => {}
                    },
                    Value::If {
                        then_block,
                        else_block,
                        ..
                    } => {
                        const_fold_block(then_block);
                        const_fold_block(else_block);
                    }
                    Value::Loop {
                        header,
                        body,
                        latch,
                    } => {
                        const_fold_block(header);
                        const_fold_block(body);
                        const_fold_block(latch);
                    }
                    _ => {}
                }
            }
            Op::Let { value, .. } => {
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    const_fold_block(then_block);
                    const_fold_block(else_block);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    const_fold_block(header);
                    const_fold_block(body);
                    const_fold_block(latch);
                }
            }
            _ => {}
        }
    }
}

fn fold_bin(op: BinOp, a: i64, b: i64) -> Option<i64> {
    Some(match op {
        BinOp::Add => a.checked_add(b)?,
        BinOp::Sub => a.checked_sub(b)?,
        BinOp::Mul => a.checked_mul(b)?,
        BinOp::Div if b != 0 && !(a == i64::MIN && b == -1) => a / b,
        BinOp::Rem if b != 0 && !(a == i64::MIN && b == -1) => a % b,
        BinOp::Eq => (a == b) as i64,
        BinOp::Ne => (a != b) as i64,
        BinOp::Lt => (a < b) as i64,
        BinOp::Le => (a <= b) as i64,
        BinOp::Gt => (a > b) as i64,
        BinOp::Ge => (a >= b) as i64,
        BinOp::And => ((a != 0) && (b != 0)) as i64,
        BinOp::Or => ((a != 0) || (b != 0)) as i64,
        _ => return None,
    })
}

pub(crate) fn copy_prop_block(block: &mut Block) {
    let mut rewrite: HashMap<u32, u32> = HashMap::default();
    for op in &mut block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                rewrite_value(value, &rewrite);
                if let Value::Local(Local(src)) = value {
                    let root = rewrite.get(src).copied().unwrap_or(*src);
                    rewrite.insert(local.0, root);
                    *value = Value::Local(Local(root));
                }
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    copy_prop_block(then_block);
                    copy_prop_block(else_block);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    copy_prop_block(header);
                    copy_prop_block(body);
                    copy_prop_block(latch);
                }
            }
            Op::Let { value, .. } => {
                rewrite_value(value, &rewrite);
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    copy_prop_block(then_block);
                    copy_prop_block(else_block);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    copy_prop_block(header);
                    copy_prop_block(body);
                    copy_prop_block(latch);
                }
            }
            Op::Effect { value } => rewrite_value(value, &rewrite),
            Op::Assign { value, .. } | Op::Return { value } => {
                if let Some(&r) = rewrite.get(&value.0) {
                    *value = Local(r);
                }
            }
            Op::Break | Op::Continue => {}
        }
    }
    if let Some(r) = block.result {
        if let Some(&nr) = rewrite.get(&r.0) {
            block.result = Some(Local(nr));
        }
    }
}
