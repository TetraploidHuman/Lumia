//! Import-path / selective-export completion helpers (stdlib + local modules).

use crate::load::{
    lumi_exports, path_candidates, search_roots_for, std_is_auto_imported, KNOWN_LUMI_MODULES,
};
use crate::vis::{item_is_priv, item_name};
use lumi_syntax::parse_module;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use super::completion_item::push_item;

/// Completion inside an incomplete `import` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ImportComplete {
    /// Path segments before `{` / `*` — e.g. `import lumi.|` or `import lu|`.
    Path {
        /// Completed segments (`["lumi"]` after `import lumi.`).
        segments: Vec<String>,
        /// Incomplete token at the cursor (may be empty after a trailing `.`).
        prefix: String,
    },
    /// Inside `{ … }` selective list — e.g. `import lumi.io.{prin|`.
    Selective { path: Vec<String>, prefix: String },
    /// Inside `{ name as | }` — suppress export suggestions.
    SelectiveAlias,
}

/// Detect import-path / selective-export completion from the line up to the cursor.
pub(super) fn detect_import_complete(line: &str) -> Option<ImportComplete> {
    let line = line.trim_start();
    let rest = line.strip_prefix("import")?;
    // Require a boundary so `imported` does not match.
    let rest = match rest.chars().next() {
        None => "",
        Some(c) if c.is_whitespace() => rest.trim_start(),
        _ => return None,
    };

    if let Some(brace_at) = rest.find('{') {
        let path_str = rest[..brace_at].trim().trim_end_matches('.');
        let path: Vec<String> = if path_str.is_empty() {
            Vec::new()
        } else {
            path_str
                .split('.')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        };
        let inside = &rest[brace_at + 1..];
        // Stop at closing `}` if the user already typed it before the cursor.
        let inside = inside.split('}').next().unwrap_or(inside);
        let last = inside.rsplit(',').next().unwrap_or("").trim();
        // Alias position (`name as |` or bare `as`) — do not suggest exports.
        if last == "as" || last.contains(" as") {
            return Some(ImportComplete::SelectiveAlias);
        }
        return Some(ImportComplete::Selective {
            path,
            prefix: last.to_string(),
        });
    }

    if rest.is_empty() {
        return Some(ImportComplete::Path {
            segments: Vec::new(),
            prefix: String::new(),
        });
    }

    let ends_dot = rest.ends_with('.');
    let parts: Vec<&str> = rest.split('.').collect();
    if ends_dot {
        let segments = parts
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| (*s).to_string())
            .collect();
        Some(ImportComplete::Path {
            segments,
            prefix: String::new(),
        })
    } else {
        let mut parts: Vec<String> = parts.iter().map(|s| (*s).to_string()).collect();
        let prefix = parts.pop().unwrap_or_default();
        Some(ImportComplete::Path {
            segments: parts,
            prefix,
        })
    }
}

fn matches_prefix(name: &str, prefix: &str) -> bool {
    prefix.is_empty() || name.starts_with(prefix)
}

fn dir_looks_like_package(dir: &Path) -> bool {
    let Ok(rd) = fs::read_dir(dir) else {
        return false;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == "mod.lm" || name.ends_with(".lm") {
            return true;
        }
        if p.is_dir() && !name.starts_with('.') {
            // One level of nesting is enough to treat as a package root segment.
            return true;
        }
    }
    false
}

/// Child path segment names under `segments` in any search root.
fn local_child_names(roots: &[PathBuf], segments: &[String]) -> Vec<String> {
    let mut names = HashSet::default();
    for root in roots {
        let dir = if segments.is_empty() {
            root.clone()
        } else {
            root.join(segments.join("/"))
        };
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let path = e.path();
            let name = e.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if path.is_file() {
                if let Some(stem) = name.strip_suffix(".lm") {
                    // Prefer `foo.lm` over dotted `a.b.lm` as multi-segment paths.
                    if !stem.contains('.') {
                        names.insert(stem.to_string());
                    }
                }
            } else if path.is_dir() && dir_looks_like_package(&path) {
                names.insert(name.to_string());
            }
        }
    }
    let mut out: Vec<String> = names.into_iter().collect();
    out.sort();
    out
}

fn find_local_module_file(roots: &[PathBuf], path: &[String]) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let rel: Vec<&str> = path.iter().map(String::as_str).collect();
    for base in roots {
        for cand in path_candidates(base, &rel) {
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

fn read_src(path: &Path, overlays: &HashMap<PathBuf, String>) -> Option<String> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(s) = overlays.get(&canon).or_else(|| overlays.get(path)) {
        return Some(s.clone());
    }
    fs::read_to_string(path).ok()
}

fn public_export_names(src: &str) -> Vec<String> {
    let Ok(m) = parse_module(src) else {
        return Vec::new();
    };
    let mut names: Vec<String> = m
        .items
        .iter()
        .filter(|it| !item_is_priv(it))
        .filter_map(item_name)
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Build completion items for an import context.
///
/// `importer` is the current document path (for search roots / self-skip).
/// `overlays` maps canonical (or raw) paths → unsaved buffer text.
pub(super) fn import_completion_items(
    ctx: &ImportComplete,
    importer: Option<&Path>,
    overlays: &HashMap<PathBuf, String>,
) -> Vec<Value> {
    let mut items = Vec::new();
    let mut seen = HashSet::default();
    let roots = importer
        .map(search_roots_for)
        .unwrap_or_else(|| vec![PathBuf::from(".")]);
    let self_stem = importer
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map(str::to_string);

    match ctx {
        ImportComplete::Path { segments, prefix } => {
            if segments.first().map(String::as_str) == Some("lumi") {
                if segments.len() == 1 {
                    for m in KNOWN_LUMI_MODULES {
                        if matches_prefix(m, prefix) {
                            let detail = if std_is_auto_imported(m) {
                                "stdlib (auto-imported)"
                            } else {
                                "stdlib (explicit import)"
                            };
                            push_item(&mut items, &mut seen, m, 9, Some(detail));
                        }
                    }
                }
                return items;
            }

            if segments.is_empty() && matches_prefix("lumi", prefix) {
                push_item(
                    &mut items,
                    &mut seen,
                    "lumi",
                    9,
                    Some("standard library root"),
                );
            }

            for name in local_child_names(&roots, segments) {
                if Some(name.as_str()) == self_stem.as_deref() && segments.is_empty() {
                    continue;
                }
                if matches_prefix(&name, prefix) {
                    let detail = if segments.is_empty() {
                        "local module"
                    } else {
                        "local package"
                    };
                    push_item(&mut items, &mut seen, &name, 9, Some(detail));
                }
            }
        }
        ImportComplete::SelectiveAlias => {}
        ImportComplete::Selective { path, prefix } => {
            if path.first().map(String::as_str) == Some("lumi") {
                if path.len() < 2 {
                    return items;
                }
                let Ok(exports) = lumi_exports(path) else {
                    return items;
                };
                let mod_name = path[1].as_str();
                for name in exports {
                    if matches_prefix(&name, prefix) {
                        push_item(
                            &mut items,
                            &mut seen,
                            &name,
                            3,
                            Some(&format!("lumi.{mod_name}")),
                        );
                    }
                }
                return items;
            }

            let Some(file) = find_local_module_file(&roots, path) else {
                return items;
            };
            let Some(src) = read_src(&file, overlays) else {
                return items;
            };
            let detail = path.join(".");
            for name in public_export_names(&src) {
                if matches_prefix(&name, prefix) {
                    push_item(&mut items, &mut seen, &name, 3, Some(&detail));
                }
            }
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn labels(items: &[Value]) -> Vec<&str> {
        items.iter().filter_map(|v| v["label"].as_str()).collect()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("lumi_imp_cmp_{tag}_{nanos}"));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn detect_import_path_after_lumi_dot() {
        assert_eq!(
            detect_import_complete("import lumi."),
            Some(ImportComplete::Path {
                segments: vec!["lumi".into()],
                prefix: String::new(),
            })
        );
        assert_eq!(
            detect_import_complete("import lumi.i"),
            Some(ImportComplete::Path {
                segments: vec!["lumi".into()],
                prefix: "i".into(),
            })
        );
        assert_eq!(
            detect_import_complete("import lu"),
            Some(ImportComplete::Path {
                segments: vec![],
                prefix: "lu".into(),
            })
        );
    }

    #[test]
    fn detect_import_selective_exports() {
        assert_eq!(
            detect_import_complete("import lumi.io.{"),
            Some(ImportComplete::Selective {
                path: vec!["lumi".into(), "io".into()],
                prefix: String::new(),
            })
        );
        assert_eq!(
            detect_import_complete("import lumi.io.{prin"),
            Some(ImportComplete::Selective {
                path: vec!["lumi".into(), "io".into()],
                prefix: "prin".into(),
            })
        );
        assert_eq!(
            detect_import_complete("import lumi.io.{println, as"),
            Some(ImportComplete::SelectiveAlias)
        );
        assert_eq!(
            detect_import_complete("import math.{ad"),
            Some(ImportComplete::Selective {
                path: vec!["math".into()],
                prefix: "ad".into(),
            })
        );
    }

    #[test]
    fn detect_import_ignores_imported_ident() {
        assert_eq!(detect_import_complete("imported"), None);
        assert_eq!(detect_import_complete("val x = 1"), None);
    }

    #[test]
    fn import_completion_lists_lumi_modules() {
        let ctx = ImportComplete::Path {
            segments: vec!["lumi".into()],
            prefix: String::new(),
        };
        let items = import_completion_items(&ctx, None, &HashMap::default());
        let got = labels(&items);
        assert!(got.contains(&"io"), "{got:?}");
        assert!(got.contains(&"linalg"), "{got:?}");
    }

    #[test]
    fn import_completion_lists_local_modules() {
        let dir = temp_dir("local");
        fs::write(dir.join("math.lm"), "module Math\nval add(a, b) = { a + b }\n").unwrap();
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::write(
            dir.join("pkg/helpers.lm"),
            "module Helpers\nval tip = { 1 }\n",
        )
        .unwrap();
        let entry = dir.join("main.lm");
        fs::write(&entry, "module Main\n").unwrap();

        let ctx = ImportComplete::Path {
            segments: vec![],
            prefix: String::new(),
        };
        let items = import_completion_items(&ctx, Some(&entry), &HashMap::default());
        let got = labels(&items);
        assert!(got.contains(&"lumi"), "{got:?}");
        assert!(got.contains(&"math"), "{got:?}");
        assert!(got.contains(&"pkg"), "{got:?}");
        assert!(!got.contains(&"main"), "should skip self: {got:?}");

        let ctx = ImportComplete::Path {
            segments: vec!["pkg".into()],
            prefix: String::new(),
        };
        let items = import_completion_items(&ctx, Some(&entry), &HashMap::default());
        let got = labels(&items);
        assert_eq!(got, vec!["helpers"]);

        let ctx = ImportComplete::Selective {
            path: vec!["math".into()],
            prefix: String::new(),
        };
        let items = import_completion_items(&ctx, Some(&entry), &HashMap::default());
        let got = labels(&items);
        assert!(got.contains(&"add"), "{got:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_completion_uses_overlay_for_exports() {
        let dir = temp_dir("overlay");
        let math = dir.join("math.lm");
        fs::write(&math, "module Math\nval old = { 0 }\n").unwrap();
        let entry = dir.join("main.lm");
        fs::write(&entry, "module Main\n").unwrap();

        let mut overlays = HashMap::default();
        overlays.insert(
            math.clone(),
            "module Math\nval fresh = { 1 }\npriv val hidden = { 2 }\n".into(),
        );

        let ctx = ImportComplete::Selective {
            path: vec!["math".into()],
            prefix: String::new(),
        };
        let items = import_completion_items(&ctx, Some(&entry), &overlays);
        let got = labels(&items);
        assert!(got.contains(&"fresh"), "{got:?}");
        assert!(!got.contains(&"old"), "{got:?}");
        assert!(!got.contains(&"hidden"), "{got:?}");

        let _ = fs::remove_dir_all(&dir);
    }
}
