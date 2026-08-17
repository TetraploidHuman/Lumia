//! textDocument/completion.

use super::cursor::{pos_to_byte, prefix_at};
use super::state::{state_lock, Analysis};
use anyhow::Result;
use lumia_hir::{surface_names, SurfaceRole};
use lumia_syntax::TokenKind;
use lumia_ty::display_type;
use rustc_hash::FxHashSet as HashSet;
use serde_json::{json, Value};

fn matches_prefix(label: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    label.starts_with(prefix)
        || label
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
}

pub(super) fn completion_items(analysis: Option<&Analysis>, prefix: &str) -> Vec<Value> {
    let mut items = Vec::new();
    let mut seen = HashSet::default();

    let mut push = |label: &str, kind: u8, detail: Option<&str>| {
        if !matches_prefix(label, prefix) {
            return;
        }
        if !seen.insert(label.to_string()) {
            return;
        }
        let mut v = json!({ "label": label, "kind": kind });
        if let Some(d) = detail {
            v["detail"] = json!(d);
        }
        items.push(v);
    };

    // Completion offers real keywords + foreign surface soft (`pure`/`fn`).
    for kw in TokenKind::KEYWORDS
        .iter()
        .chain(TokenKind::SURFACE_SOFT.iter())
    {
        push(kw, 14, None); // CompletionItemKind.Keyword
    }
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

    items
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
    let prefix = analysis
        .map(|a| {
            let byte = pos_to_byte(&a.src, line, character);
            prefix_at(&a.src, byte)
        })
        .or_else(|| {
            state.docs.get(uri).map(|src| {
                let byte = pos_to_byte(src, line, character);
                prefix_at(src, byte)
            })
        })
        .unwrap_or_default();
    Ok(Value::Array(completion_items(analysis, &prefix)))
}

#[cfg(test)]
mod tests {
    use super::completion_items;

    #[test]
    fn completion_includes_keywords_without_analysis() {
        let items = completion_items(None, "");
        let labels: Vec<&str> = items.iter().filter_map(|v| v["label"].as_str()).collect();
        assert!(labels.contains(&"val"));
        assert!(labels.contains(&"match"));
        assert!(labels.contains(&"map"));
    }

    #[test]
    fn completion_scans_surface_builtins() {
        let items = completion_items(None, "");
        let labels: Vec<&str> = items.iter().filter_map(|v| v["label"].as_str()).collect();
        assert!(labels.contains(&"listOf"), "{labels:?}");
        assert!(labels.contains(&"setOf"), "{labels:?}");
        assert!(labels.contains(&"mapOf"), "{labels:?}");
        assert!(labels.contains(&"println"), "{labels:?}");
        assert!(labels.contains(&"len"), "{labels:?}");
        assert!(!labels.contains(&"adtTag"), "{labels:?}");
    }

    #[test]
    fn completion_filters_by_prefix() {
        let items = completion_items(None, "print");
        let labels: Vec<&str> = items.iter().filter_map(|v| v["label"].as_str()).collect();
        assert!(labels.contains(&"println"), "{labels:?}");
        assert!(!labels.contains(&"val"), "{labels:?}");
        assert!(!labels.contains(&"map"), "{labels:?}");
    }

    #[test]
    fn completion_imported_alias_via_loader() {
        // Import aliases need loader+std; check_source alone leaves `log` unbound.
        use crate::lsp::analyze::analyze_buffer;
        use rustc_hash::FxHashMap as HashMap;
        let src = r#"
module Main
import std.io.{println as log}
val main = { log(1) }
"#;
        let (_, analysis) = analyze_buffer("untitled:Completion-1", src, &HashMap::default());
        let a = analysis.expect("loader must typecheck untitled std import");
        let items = completion_items(Some(&a), "lo");
        let labels: Vec<&str> = items.iter().filter_map(|v| v["label"].as_str()).collect();
        assert!(
            labels.contains(&"log"),
            "imported alias `log` must appear in completion via loader, got {labels:?}"
        );
    }
}
