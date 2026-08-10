//! textDocument/completion.

use super::state::{state_lock, Analysis};
use anyhow::Result;
use lumia_ty::display_type;
use serde_json::{json, Value};

pub(super) fn completion_items(analysis: Option<&Analysis>) -> Vec<Value> {
    let mut items = Vec::new();
    let methods = [
        "map", "filter", "fold", "flatMap", "len", "get", "set", "contains", "items", "keys",
        "values", "sortBy", "take", "reverse", "concat", "join", "trim", "split", "toLower",
        "toUpper",
    ];
    for m in methods {
        items.push(json!({ "label": m, "kind": 2 })); // Method
    }
    if let Some(a) = analysis {
        for (name, ty) in &a.typed.fun_types {
            let num_vars = a
                .typed
                .fun_schemes
                .get(name)
                .map(|s| s.num_vars.as_slice())
                .unwrap_or(&[]);
            items.push(json!({
                "label": name,
                "kind": 3, // Function
                "detail": display_type(ty, num_vars),
            }));
        }
        for name in a.typed.decls.keys() {
            if !a.typed.fun_types.contains_key(name) {
                items.push(json!({ "label": name, "kind": 6 })); // Variable
            }
        }
    }
    for kw in [
        "val", "var", "match", "if", "else", "for", "in", "type", "import", "foreign", "pure",
    ] {
        items.push(json!({ "label": kw, "kind": 14 })); // Keyword
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
}
