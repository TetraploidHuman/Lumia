//! Cross-file `priv` / import visibility helpers.
//!
//! Dependency modules are fully inlined so private callees of public APIs remain
//! linkable; [`lumia_ty::NameVisibility`] then allows each file only its own
//! declarations plus names it imported (entry and deps alike).

use lumia_syntax::{ImportNames, Item};
use lumia_ty::NameVisibility;
use rustc_hash::FxHashSet as HashSet;

pub fn item_name(it: &Item) -> Option<&str> {
    match it {
        Item::Val(v) => Some(v.name.as_str()),
        Item::Type(t) => Some(t.name.as_str()),
        Item::Foreign(f) => Some(f.name.as_str()),
        Item::Trait(t) => Some(t.name.as_str()),
        Item::Instance(_) => None,
    }
}

pub fn item_is_priv(it: &Item) -> bool {
    match it {
        Item::Val(v) => v.is_priv,
        Item::Type(t) => t.is_priv,
        Item::Foreign(_) | Item::Trait(_) | Item::Instance(_) => false,
    }
}

pub fn item_file(it: &Item) -> u32 {
    match it {
        Item::Val(v) => v.span.file,
        Item::Type(t) => t.span.file,
        Item::Foreign(f) => f.span.file,
        Item::Trait(t) => t.span.file,
        Item::Instance(i) => i.span.file,
    }
}

/// Local names from `items` that an import clause makes visible to the importer.
/// When the clause uses `as`, the **alias** is visible (not the export name).
pub fn import_visible_names(items: &[Item], names: &ImportNames) -> HashSet<String> {
    let pubs: HashSet<String> = items
        .iter()
        .filter(|it| !item_is_priv(it))
        .filter_map(item_name)
        .map(|s| s.to_string())
        .collect();
    match names {
        ImportNames::All => pubs,
        ImportNames::Single(n) => {
            if pubs.contains(n.name.as_str()) {
                [n.local().to_string()].into_iter().collect()
            } else {
                HashSet::default()
            }
        }
        ImportNames::Selective(ns) => ns
            .iter()
            .filter(|n| pubs.contains(n.name.as_str()))
            .map(|n| n.local().to_string())
            .collect(),
    }
}

fn set_item_name(it: &mut Item, name: &str) {
    match it {
        Item::Val(v) => v.name = name.to_string().into(),
        Item::Type(t) => t.name = name.to_string().into(),
        Item::Foreign(f) => f.name = name.to_string().into(),
        Item::Trait(t) => t.name = name.to_string().into(),
        Item::Instance(_) => {}
    }
}

fn set_item_priv(it: &mut Item, is_priv: bool) {
    match it {
        Item::Val(v) => v.is_priv = is_priv,
        Item::Type(t) => t.is_priv = is_priv,
        Item::Foreign(_) | Item::Trait(_) | Item::Instance(_) => {}
    }
}

/// Rename public exports to their import aliases; keep a `priv` copy under the
/// original name so sibling/private callees inside the inlined module still resolve.
pub fn apply_import_aliases(mut items: Vec<Item>, names: &ImportNames) -> Vec<Item> {
    let renames: Vec<(String, String)> = match names {
        ImportNames::All => return items,
        ImportNames::Single(n) => match &n.alias {
            Some(a) if a != &n.name => vec![(n.name.to_string(), a.to_string())],
            _ => return items,
        },
        ImportNames::Selective(ns) => ns
            .iter()
            .filter_map(|n| {
                n.alias
                    .as_ref()
                    .filter(|a| *a != &n.name)
                    .map(|a| (n.name.to_string(), a.to_string()))
            })
            .collect(),
    };
    for (orig, alias) in renames {
        let Some(i) = items
            .iter()
            .position(|it| !item_is_priv(it) && item_name(it) == Some(orig.as_str()))
        else {
            continue;
        };
        let mut priv_copy = items[i].clone();
        set_item_priv(&mut priv_copy, true);
        set_item_name(&mut items[i], &alias);
        // Prefer the renamed public; keep `orig` only if nothing else owns it.
        if !items.iter().any(|it| item_name(it) == Some(orig.as_str())) {
            items.push(priv_copy);
        }
    }
    items
}

/// Record declaring file for each name; record `newly_visible` as imports of `importer_file`.
pub fn extend_visibility(
    vis: &mut NameVisibility,
    items: &[Item],
    newly_visible: &HashSet<String>,
    importer_file: u32,
) {
    for it in items {
        if let Some(name) = item_name(it) {
            vis.name_origin
                .entry(name.to_string())
                .or_insert_with(|| item_file(it));
        }
    }
    if newly_visible.is_empty() {
        return;
    }
    vis.imports_by_file
        .entry(importer_file)
        .or_default()
        .extend(newly_visible.iter().cloned());
}
