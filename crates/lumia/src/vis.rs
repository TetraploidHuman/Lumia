//! Cross-file `priv` / import visibility helpers.
//!
//! Dependency modules are fully inlined so private callees of public APIs remain
//! linkable; [`lumia_ty::NameVisibility`] then blocks entry code from naming
//! non-imported / `priv` symbols.

use lumia_syntax::{ImportNames, Item};
use lumia_ty::NameVisibility;
use std::collections::HashSet;

pub fn item_name(it: &Item) -> Option<&str> {
    match it {
        Item::Val(v) => Some(v.name.as_str()),
        Item::Type(t) => Some(t.name.as_str()),
        Item::Foreign(f) => Some(f.name.as_str()),
    }
}

pub fn item_is_priv(it: &Item) -> bool {
    match it {
        Item::Val(v) => v.is_priv,
        Item::Type(t) => t.is_priv,
        Item::Foreign(_) => false,
    }
}

fn item_file(it: &Item) -> u32 {
    match it {
        Item::Val(v) => v.span.file,
        Item::Type(t) => t.span.file,
        Item::Foreign(f) => f.span.file,
    }
}

/// Names from `items` that an import clause makes visible to the importer.
pub fn import_visible_names(items: &[Item], names: &ImportNames) -> HashSet<String> {
    let pubs: HashSet<String> = items
        .iter()
        .filter(|it| !item_is_priv(it))
        .filter_map(item_name)
        .map(|s| s.to_string())
        .collect();
    match names {
        ImportNames::All => pubs,
        ImportNames::Single(name) => {
            if pubs.contains(name) {
                HashSet::from([name.clone()])
            } else {
                HashSet::new()
            }
        }
        ImportNames::Selective(ns) => ns
            .iter()
            .filter(|n| pubs.contains(n.as_str()))
            .cloned()
            .collect(),
    }
}

/// Record declaring file for each name; optionally expand entry-visible set.
pub fn extend_visibility(
    vis: &mut NameVisibility,
    items: &[Item],
    newly_visible: &HashSet<String>,
) {
    for it in items {
        if let Some(name) = item_name(it) {
            vis.name_origin
                .entry(name.to_string())
                .or_insert_with(|| item_file(it));
        }
    }
    for n in newly_visible {
        vis.cross_file_visible.insert(n.clone());
    }
}
