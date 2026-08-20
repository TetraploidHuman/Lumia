//! textDocument/codeAction.
//!
//! Minimal, healthy implementation:
//! - If strict formatting would change the document, offer a single quickfix:
//!   "Format document" → replace the document with `lumia fmt` edits.

use super::formatting::format_document;
use super::state::state_lock;
use anyhow::Result;
use serde_json::{json, Value};

fn code_action_for_formatting(text: &str, uri: &str) -> Result<Vec<Value>> {
    let edits = format_document(text)?;
    if edits.is_empty() {
        return Ok(vec![]);
    }
    let mut changes = serde_json::Map::new();
    changes.insert(uri.to_string(), Value::Array(edits));
    Ok(vec![json!({
        "title": "Format document",
        "kind": "quickfix",
        "edit": { "changes": Value::Object(changes) }
    })])
}

pub(super) fn on_code_action(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(json!([]));
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    if uri.is_empty() {
        return Ok(json!([]));
    }

    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(json!([]));
    };
    let Some(text) = state.docs.get(uri) else {
        return Ok(json!([]));
    };
    // `format_document` is strict parse + pretty-print; do it outside the lock.
    let text = text.clone();
    drop(st);

    let actions = code_action_for_formatting(&text, uri)?;
    Ok(json!(actions))
}

#[cfg(test)]
mod tests {
    use super::on_code_action;
    use crate::lsp::test_support::{with_open_doc_state, IMPORTED_ALIAS_SRC};
    use serde_json::json;

    #[test]
    fn code_action_offers_format_document_edits() {
        let uri = "untitled:CodeAction-1";
        let src = "module T\nval x=1\n";
        with_open_doc_state(uri, src, || {
            let out = on_code_action(Some(&json!({
                "textDocument": { "uri": uri },
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                "context": { "diagnostics": [] }
            })))
            .expect("code action");

            let arr = out.as_array().expect("array");
            assert_eq!(arr.len(), 1);
            assert_eq!(arr[0]["title"], "Format document");
            let changes = &arr[0]["edit"]["changes"];
            let edits = changes[uri].as_array().expect("edits array");
            assert_eq!(edits.len(), 1);
            let new_text = edits[0]["newText"].as_str().unwrap_or("");
            assert!(new_text.contains("val x = 1"), "got newText={new_text:?}");
        });
    }

    #[test]
    fn code_action_imported_alias_via_loader_surface() {
        let uri = "untitled:CodeAction-Loader-1";
        with_open_doc_state(uri, IMPORTED_ALIAS_SRC, || {
            let out = on_code_action(Some(&json!({
                "textDocument": { "uri": uri },
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                "context": { "diagnostics": [] }
            })))
            .expect("code action");

            let arr = out.as_array().expect("array");
            assert_eq!(arr.len(), 1, "expected format quickfix, got {out}");
            let edits = arr[0]["edit"]["changes"][uri]
                .as_array()
                .expect("edits array");
            let new_text = edits[0]["newText"].as_str().unwrap_or("");
            assert!(
                new_text.contains("println as log"),
                "format quickfix must preserve imported alias under strict parse, got {new_text:?}"
            );
        });
    }
}

