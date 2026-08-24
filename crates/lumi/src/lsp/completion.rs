//! textDocument/completion.

use super::state::{state_lock, Analysis};
use anyhow::Result;
use lumi_hir::{surface_names, SurfaceRole};
use lumi_ty::display_type;
use rustc_hash::FxHashSet as HashSet;
use serde_json::{json, Value};

/// True keywords (lexer / grammar) — not scanned from builtins.
const KEYWORDS: &[&str] = &[
    "val", "var", "match", "if", "else", "for", "in", "type", "import", "foreign", "pure", "trait",
    "instance", "alt", "module",
];

pub(super) fn completion_items(analysis: Option<&Analysis>) -> Vec<Value> {
    let mut items = Vec::new();
    let mut seen = HashSet::default();

    let mut push = |label: &str, kind: u8, detail: Option<&str>| {
        if !seen.insert(label.to_string()) {
            return;
        }
        let mut v = json!({ "label": label, "kind": kind });
        if let Some(d) = detail {
            v["detail"] = json!(d);
        }
        items.push(v);
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

pub(super) fn on_completion(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(json!([]));
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(json!([]));
    };
    let analysis = state.analysis.get(uri);
    Ok(Value::Array(completion_items(analysis)))
}

#[cfg(test)]
mod tests {
    use super::completion_items;

    #[test]
    fn completion_includes_keywords_without_analysis() {
        let items = completion_items(None);
        let labels: Vec<&str> = items.iter().filter_map(|v| v["label"].as_str()).collect();
        assert!(labels.contains(&"val"));
        assert!(labels.contains(&"match"));
        assert!(labels.contains(&"map"));
    }

    #[test]
    fn completion_scans_surface_builtins() {
        let items = completion_items(None);
        let labels: Vec<&str> = items.iter().filter_map(|v| v["label"].as_str()).collect();
        assert!(labels.contains(&"listOf"), "{labels:?}");
        assert!(labels.contains(&"setOf"), "{labels:?}");
        assert!(labels.contains(&"mapOf"), "{labels:?}");
        assert!(labels.contains(&"println"), "{labels:?}");
        assert!(labels.contains(&"len"), "{labels:?}");
        assert!(!labels.contains(&"adtTag"), "{labels:?}");
    }
}
