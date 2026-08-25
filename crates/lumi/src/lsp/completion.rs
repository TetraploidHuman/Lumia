//! textDocument/completion.

use super::cursor::pos_to_byte;
use super::import_complete::{detect_import_complete, import_completion_items};
use super::state::{state_lock, Analysis};
use super::uri::uri_to_path;
use anyhow::Result;
use lumi_hir::{surface_names, SurfaceRole};
use lumi_ty::display_type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use serde_json::{json, Value};
use std::path::PathBuf;

/// True keywords (lexer / grammar) — not scanned from builtins.
const KEYWORDS: &[&str] = &[
    "val", "var", "match", "if", "else", "for", "in", "type", "import", "foreign", "pure", "trait",
    "instance", "alt", "module",
];

fn push_item(items: &mut Vec<Value>, seen: &mut HashSet<String>, label: &str, kind: u8, detail: Option<&str>) {
    if !seen.insert(label.to_string()) {
        return;
    }
    let mut v = json!({ "label": label, "kind": kind });
    if let Some(d) = detail {
        v["detail"] = json!(d);
    }
    items.push(v);
}

/// Line text from the start of the current line through the cursor (exclusive of EOL).
fn line_prefix_at(src: &str, byte: u32) -> &str {
    let byte = (byte as usize).min(src.len());
    // Avoid splitting a UTF-8 codepoint.
    let mut end = byte;
    while end > 0 && !src.is_char_boundary(end) {
        end -= 1;
    }
    let start = src[..end].rfind('\n').map(|i| i + 1).unwrap_or(0);
    &src[start..end]
}

pub(super) fn completion_items(analysis: Option<&Analysis>) -> Vec<Value> {
    let mut items = Vec::new();
    let mut seen = HashSet::default();

    let mut push = |label: &str, kind: u8, detail: Option<&str>| {
        push_item(&mut items, &mut seen, label, kind, detail);
    };

    // Scan the shared language surface (prelude ctors + Builtin + HOF desugars).
    for sn in surface_names() {
        let kind = match sn.role {
            SurfaceRole::Free => 3,   // Function
            SurfaceRole::Method => 2, // Method
        };
        push(sn.name, kind, None);
    }

    if let Some(a) = analysis {
        for (name, ty) in &a.typed.fun_types {
            if name.starts_with("__") {
                continue;
            }
            let num_vars = a
                .typed
                .fun_schemes
                .get(name)
                .map(|s| s.num_vars.as_slice())
                .unwrap_or(&[]);
            let detail = display_type(ty, num_vars);
            push(name, 3, Some(detail.as_str()));
        }
        for name in a.typed.decls.keys() {
            if name.starts_with("__") || a.typed.fun_types.contains_key(name) {
                continue;
            }
            push(name, 6, None); // Variable
        }
        for adt in &a.typed.module.adts {
            for v in &adt.variants {
                let detail = format!(
                    "{} · {}",
                    adt.name,
                    if v.arity == 0 { "unit" } else { "ctor" }
                );
                push(&v.name, 4, Some(&detail)); // Constructor
            }
        }
        for prod in &a.typed.module.products {
            push(&prod.name, 22, Some("product type")); // Struct
        }
    }

    for kw in KEYWORDS {
        push(kw, 14, None); // Keyword
    }
    items
}

fn doc_overlays(docs: &HashMap<String, String>) -> HashMap<PathBuf, String> {
    let mut out = HashMap::default();
    for (uri, text) in docs {
        let path = uri_to_path(uri);
        let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
        out.insert(canon, text.clone());
        out.insert(path, text.clone());
    }
    out
}

pub(super) fn on_completion(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(json!([]));
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let line = params["position"]["line"].as_u64().unwrap_or(0) as u32;
    let character = params["position"]["character"].as_u64().unwrap_or(0) as u32;

    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(json!([]));
    };
    let analysis = state.analysis.get(uri);
    let src = state
        .docs
        .get(uri)
        .map(String::as_str)
        .or_else(|| analysis.map(|a| a.src.as_str()))
        .unwrap_or("");

    let byte = pos_to_byte(src, line, character);
    let line_text = line_prefix_at(src, byte);
    if let Some(ctx) = detect_import_complete(line_text) {
        let importer = {
            let p = uri_to_path(uri);
            if p.as_os_str().is_empty() {
                None
            } else {
                Some(p)
            }
        };
        let overlays = doc_overlays(&state.docs);
        return Ok(Value::Array(import_completion_items(
            &ctx,
            importer.as_deref(),
            &overlays,
        )));
    }

    Ok(Value::Array(completion_items(analysis)))
}

#[cfg(test)]
mod tests {
    use super::{completion_items, line_prefix_at};

    fn labels(items: &[serde_json::Value]) -> Vec<&str> {
        items.iter().filter_map(|v| v["label"].as_str()).collect()
    }

    #[test]
    fn completion_includes_keywords_without_analysis() {
        let items = completion_items(None);
        let labels = labels(&items);
        assert!(labels.contains(&"val"));
        assert!(labels.contains(&"match"));
        assert!(labels.contains(&"map"));
    }

    #[test]
    fn completion_scans_surface_builtins() {
        let items = completion_items(None);
        let labels = labels(&items);
        assert!(labels.contains(&"listOf"), "{labels:?}");
        assert!(labels.contains(&"setOf"), "{labels:?}");
        assert!(labels.contains(&"mapOf"), "{labels:?}");
        assert!(labels.contains(&"len"), "{labels:?}");
        assert!(!labels.contains(&"println"), "{labels:?}");
        assert!(!labels.contains(&"__println"), "{labels:?}");
        assert!(!labels.contains(&"adtTag"), "{labels:?}");
    }

    #[test]
    fn line_prefix_at_stops_at_cursor() {
        let src = "module M\nimport lumi.\nval x = 1\n";
        // Cursor after `import lumi.` on line 1.
        let byte = super::super::cursor::pos_to_byte(src, 1, "import lumi.".len() as u32);
        assert_eq!(line_prefix_at(src, byte), "import lumi.");
    }
}
