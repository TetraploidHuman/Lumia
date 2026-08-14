//! Match exhaustiveness / coverage.

use crate::ast::{AdtDef, CtorInfo};
use crate::lower::LowerError;
use lumia_syntax::Pattern;
use rustc_hash::FxHashMap as HashMap;

pub(crate) fn check_match_exhaustiveness(
    arms: &[lumia_syntax::MatchArm],
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
    products: &HashMap<String, Vec<String>>,
) -> Result<(), LowerError> {
    let pats: Vec<&Pattern> = arms
        .iter()
        // Guards refine payloads; a guarded arm does not exhaust a constructor.
        .filter(|a| a.guard.is_none())
        .map(|a| &a.pattern)
        .collect();
    check_pats_cover(&pats, ctors, adts, products, "")
}

fn flatten_or<'a>(pat: &'a Pattern, out: &mut Vec<&'a Pattern>) {
    match pat {
        Pattern::Or(ps, _) => {
            for p in ps {
                flatten_or(p, out);
            }
        }
        other => out.push(other),
    }
}

/// Irrefutable at this level for coverage: `_`, binders, or products/tuples whose
/// fields are all catch-alls. Nullary ctor names (`None`) are refutable.
pub(crate) fn coverage_catch_all(pat: &Pattern, ctors: &HashMap<String, CtorInfo>) -> bool {
    match pat {
        Pattern::Wildcard(_) => true,
        Pattern::Ident(name, _) => ctors.get(name).is_none_or(|c| c.arity != 0),
        Pattern::Or(ps, _) => ps.iter().any(|p| coverage_catch_all(p, ctors)),
        Pattern::Struct { fields, .. } => {
            fields.iter().all(|(_, sub)| coverage_catch_all(sub, ctors))
        }
        Pattern::Tuple { elems, .. } => elems.iter().all(|e| coverage_catch_all(e, ctors)),
        Pattern::Variant { .. }
        | Pattern::List { .. }
        | Pattern::Int(_, _)
        | Pattern::Float(_, _)
        | Pattern::Bool(_, _)
        | Pattern::Char(_, _)
        | Pattern::String(_, _) => false,
    }
}

/// Whether `pats` (alternatives) cover all values at this pattern depth.
/// Recurses into variant payloads, product fields, and tuple elements.
pub(crate) fn check_pats_cover(
    pats: &[&Pattern],
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
    products: &HashMap<String, Vec<String>>,
    path: &str,
) -> Result<(), LowerError> {
    use rustc_hash::FxHashSet as HashSet;

    let mut flat = Vec::new();
    for p in pats {
        flatten_or(p, &mut flat);
    }
    // Empty after filtering (no arms, or only guarded arms) is never exhaustive.
    if flat.is_empty() {
        let where_ = if path.is_empty() {
            "scrutinee".into()
        } else {
            path.to_string()
        };
        return Err(LowerError::message_only(format!(
            "non-exhaustive match on {where_}: no covering arm (empty match or only guarded arms)"
        )));
    }
    if flat.iter().any(|p| coverage_catch_all(p, ctors)) {
        return Ok(());
    }

    let mut covered: HashMap<String, HashSet<i64>> = HashMap::default();
    let mut ctor_args: HashMap<String, Vec<Vec<&Pattern>>> = HashMap::default();
    let mut product_fields: HashMap<String, HashMap<String, Vec<&Pattern>>> = HashMap::default();
    let mut tuple_rows: Vec<Vec<&Pattern>> = Vec::new();
    let mut list_pats: Vec<&Pattern> = Vec::new();
    let mut saw_sum = false;
    let mut saw_product = false;
    let mut saw_list = false;
    let mut saw_int = false;
    let mut saw_float = false;
    let mut saw_bool = false;
    let mut bool_true = false;
    let mut bool_false = false;
    let mut saw_open_lit = false; // Char / String: open domains need `_`

    for p in &flat {
        match *p {
            Pattern::Ident(name, _) => {
                if let Some(c) = ctors.get(name) {
                    if c.arity == 0 {
                        saw_sum = true;
                        covered.entry(c.adt_name.clone()).or_default().insert(c.tag);
                    }
                }
            }
            Pattern::Variant { name, args, .. } => {
                if let Some(c) = ctors.get(name) {
                    saw_sum = true;
                    covered.entry(c.adt_name.clone()).or_default().insert(c.tag);
                    ctor_args
                        .entry(name.clone())
                        .or_default()
                        .push(args.iter().collect());
                }
            }
            Pattern::Struct { name, fields, .. } => {
                saw_product = true;
                let entry = product_fields.entry(name.clone()).or_default();
                for (fname, sub) in fields {
                    entry.entry(fname.clone()).or_default().push(sub);
                }
            }
            Pattern::Tuple { elems, .. } => {
                saw_product = true;
                tuple_rows.push(elems.iter().collect());
            }
            Pattern::List { .. } => {
                saw_list = true;
                list_pats.push(*p);
            }
            Pattern::Int(_, _) => {
                saw_int = true;
            }
            Pattern::Float(_, _) => {
                saw_float = true;
            }
            Pattern::Bool(b, _) => {
                saw_bool = true;
                if *b {
                    bool_true = true;
                } else {
                    bool_false = true;
                }
            }
            Pattern::Char(_, _) | Pattern::String(_, _) => {
                saw_open_lit = true;
            }
            Pattern::Wildcard(_) | Pattern::Or(_, _) => {}
        }
    }

    if saw_sum {
        for (adt_name, tags) in &covered {
            let Some(def) = adts.iter().find(|a| a.name == *adt_name) else {
                continue;
            };
            let missing: Vec<&str> = def
                .variants
                .iter()
                .filter(|v| !tags.contains(&v.tag))
                .map(|v| v.name.as_str())
                .collect();
            if !missing.is_empty() {
                let where_ = if path.is_empty() {
                    format!("`{adt_name}`")
                } else {
                    format!("`{adt_name}` (in {path})")
                };
                return Err(LowerError::message_only(format!(
                    "non-exhaustive match on {where_}: missing variant(s) {}",
                    missing.join(", ")
                )));
            }
            for v in &def.variants {
                if v.arity == 0 {
                    continue;
                }
                let Some(rows) = ctor_args.get(&v.name) else {
                    continue;
                };
                for slot in 0..v.arity {
                    let col: Vec<&Pattern> =
                        rows.iter().filter_map(|r| r.get(slot).copied()).collect();
                    if col.len() != rows.len() {
                        continue;
                    }
                    let nested = if path.is_empty() {
                        v.name.clone()
                    } else {
                        format!("{path}.{}", v.name)
                    };
                    check_pats_cover(&col, ctors, adts, products, &nested)?;
                }
            }
        }
    }

    if saw_product {
        for (pname, fields) in &product_fields {
            let order = products.get(pname).cloned().unwrap_or_default();
            for fname in &order {
                let Some(subs) = fields.get(fname) else {
                    continue;
                };
                let nested = if path.is_empty() {
                    format!("{pname}.{fname}")
                } else {
                    format!("{path}.{pname}.{fname}")
                };
                check_pats_cover(subs, ctors, adts, products, &nested)?;
            }
        }
        if !tuple_rows.is_empty() {
            let arity = tuple_rows[0].len();
            if tuple_rows.iter().all(|r| r.len() == arity) {
                for slot in 0..arity {
                    let col: Vec<&Pattern> = tuple_rows
                        .iter()
                        .filter_map(|r| r.get(slot).copied())
                        .collect();
                    let nested = if path.is_empty() {
                        format!(".{}", slot)
                    } else {
                        format!("{path}.{}", slot)
                    };
                    check_pats_cover(&col, ctors, adts, products, &nested)?;
                }
            }
        }
    }

    // Bool is a closed 2-value domain: both `true` and `false` cover it.
    if !saw_sum && !saw_product && saw_bool && !saw_int && !saw_float && !saw_list && !saw_open_lit
    {
        if bool_true && bool_false {
            return Ok(());
        }
        let where_ = if path.is_empty() {
            "Bool".into()
        } else {
            format!("Bool (in {path})")
        };
        let missing = match (bool_true, bool_false) {
            (false, false) => "true, false",
            (true, false) => "false",
            (false, true) => "true",
            (true, true) => unreachable!(),
        };
        return Err(LowerError::message_only(format!(
            "non-exhaustive match on {where_}: missing {missing} (or `_`)"
        )));
    }

    // Int / Float / Char / String / List have infinite (or open) domains: without
    // a catch-all binder/`_`, finite literal arms are never enough. List is
    // exhaustive only when every length is covered (`[]` + `[…, ..rest]` style).
    if !saw_sum && !saw_product && (saw_int || saw_float || saw_list || saw_open_lit) {
        let where_ = if path.is_empty() {
            "scrutinee".into()
        } else {
            path.to_string()
        };
        if saw_list {
            if !list_patterns_exhaustive(&list_pats) {
                return Err(LowerError::message_only(format!(
                    "non-exhaustive match on List (in {where_}): add `[]` / `[..rest]` arms or `_`"
                )));
            }
            // Nested element columns (fixed prefix + rest wildcards).
            // Arms that do not constrain a slot (shorter fixed `[]`, or rest
            // covering longer lengths) contribute `_` so we still check nested
            // coverage — skipping when `col.len() != list_pats.len()` missed
            // `[None, ..r]` vs `Some(_)` after a `[]` arm.
            let max_fixed = list_pats
                .iter()
                .filter_map(|p| match p {
                    Pattern::List { elems, .. } => Some(elems.len()),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            for slot in 0..max_fixed {
                let mut owned: Vec<Pattern> = Vec::new();
                let mut from_arm: Vec<&Pattern> = Vec::new();
                for p in &list_pats {
                    match p {
                        Pattern::List { elems, rest, span } => {
                            if let Some(e) = elems.get(slot) {
                                from_arm.push(e);
                            } else if rest.is_some() {
                                owned.push(Pattern::Wildcard(*span));
                            }
                            // else: fixed shorter list — does not match len > slot
                        }
                        _ => {}
                    }
                }
                let mut col: Vec<&Pattern> = from_arm;
                col.extend(owned.iter());
                if col.is_empty() {
                    continue;
                }
                let nested = if path.is_empty() {
                    format!("[{slot}]")
                } else {
                    format!("{path}[{slot}]")
                };
                check_pats_cover(&col, ctors, adts, products, &nested)?;
            }
        } else if saw_int {
            return Err(LowerError::message_only(format!(
                "non-exhaustive match on Int (in {where_}): integer literals need a `_` arm"
            )));
        } else if saw_float {
            return Err(LowerError::message_only(format!(
                "non-exhaustive match on Float (in {where_}): float literals need a `_` arm"
            )));
        } else if saw_open_lit {
            return Err(LowerError::message_only(format!(
                "non-exhaustive match on Char/String (in {where_}): literal arms need a `_` arm"
            )));
        }
    }

    Ok(())
}

/// `[]` covers length 0; `[e0,…,ek-1, ..rest]` covers all lengths `>= k`.
/// Together they must cover `0..`.
pub(crate) fn list_patterns_exhaustive(pats: &[&Pattern]) -> bool {
    use rustc_hash::FxHashSet as HashSet;
    let mut exact: HashSet<usize> = HashSet::default();
    let mut rest_mins: Vec<usize> = Vec::new();
    for p in pats {
        match p {
            Pattern::List { elems, rest, .. } => {
                if rest.is_some() {
                    rest_mins.push(elems.len());
                } else {
                    exact.insert(elems.len());
                }
            }
            _ => return false,
        }
    }
    let Some(min_rest) = rest_mins.into_iter().min() else {
        // Only fixed-length arms — infinitely many lengths remain.
        return false;
    };
    (0..min_rest).all(|len| exact.contains(&len))
}
