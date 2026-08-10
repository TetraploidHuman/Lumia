//! textDocument/formatting.

use super::state::state_lock;
use anyhow::Result;
use lumia_syntax::{format_module_src, line_starts, parse_module, stamp_module};
use serde_json::{json, Value};

pub(super) fn format_document(text: &str) -> Vec<Value> {
    let mut m = match parse_module(text) {
        Ok(m) => m,
        Err(_) => return vec![],
    };
    stamp_module(&mut m, 0);
    let formatted = format_module_src(&m);
    if formatted == text {
        return vec![];
    }
    let starts = line_starts(text);
    let last_line = starts.len().saturating_sub(1) as u32;
    let last_col = text.lines().last().map(|l| l.len() as u32).unwrap_or(0);
    vec![json!({
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": last_line, "character": last_col }
        },
        "newText": formatted
    })]
}

pub(super) fn on_formatting(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(json!([]));
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(json!([]));
    };
    let Some(text) = state.docs.get(uri) else {
        return Ok(json!([]));
    };
    Ok(Value::Array(format_document(text)))
}

#[cfg(test)]
mod tests {
    use super::format_document;

    #[test]
    fn format_document_pretty_prints_snippet() {
        let messy = "module T\nval x=1\n";
        let edits = format_document(messy);
        assert_eq!(edits.len(), 1);
        let new_text = edits[0]["newText"].as_str().unwrap();
        assert!(new_text.contains("val x = 1"));
    }

    #[test]
    fn format_document_no_edit_when_already_formatted() {
        let src = "module T\n\nval x = 1\n";
        let edits = format_document(src);
        assert!(edits.is_empty());
    }
}
