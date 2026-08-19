//! textDocument/formatting.
//!
//! **Contract:** format always uses a *strict* `parse_module` tree (pretty-print
//! authority). Do **not** reuse `Analysis.surface` (recovering AST with holes) —
//! semantic tokens may paint from recovering surface; format must fail closed
//! on parse errors (`Err`, never empty edits).

use super::cursor::byte_to_position;
use super::state::{source_fingerprint, state_lock};
use anyhow::{bail, Result};
use lumia_syntax::{format_matches_source, format_module_src, parse_module, stamp_module};
use serde_json::{json, Value};

/// Pretty-print `text`. Returns `Ok([])` when already formatted.
/// Parse failures are `Err` (callers must not treat them as "no edits").
pub(super) fn format_document(text: &str) -> Result<Vec<Value>> {
    let mut m = parse_module(text).map_err(|e| anyhow::anyhow!("parse failed: {e}"))?;
    stamp_module(&mut m, 0);
    let formatted = format_module_src(&m);
    if format_matches_source(text, &formatted) {
        return Ok(vec![]);
    }
    // EOF position — do not use `str::lines().last()` (drops a trailing empty line).
    let (eline, ecol) = byte_to_position(text, text.len() as u32);
    Ok(vec![json!({
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": {
                "line": eline,
                "character": ecol
            }
        },
        "newText": formatted
    })])
}

pub(super) fn on_formatting(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(json!([]));
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    if uri.is_empty() {
        bail!("textDocument/formatting: missing textDocument.uri");
    }
    let st = state_lock();
    let Some(state) = st.as_ref() else {
        bail!("textDocument/formatting: LSP state not initialized");
    };
    let Some(text) = state.docs.get(uri) else {
        bail!("textDocument/formatting: document not open ({uri})");
    };
    let hash = source_fingerprint(text);
    if let Some((cached_hash, edits)) = state.format_cache.get(uri) {
        if *cached_hash == hash {
            return Ok(Value::Array(edits.clone()));
        }
    }
    let text = text.clone();
    drop(st);
    let edits = format_document(&text)?;
    {
        let mut st = state_lock();
        if let Some(s) = st.as_mut() {
            s.format_cache.insert(uri.to_string(), (hash, edits.clone()));
        }
    }
    Ok(Value::Array(edits))
}

#[cfg(test)]
mod tests {
    use super::format_document;

    #[test]
    fn format_document_pretty_prints_snippet() {
        let messy = "module T\nval x=1\n";
        let edits = format_document(messy).expect("format");
        assert_eq!(edits.len(), 1);
        let new_text = edits[0]["newText"].as_str().unwrap();
        assert!(new_text.contains("val x = 1"));
    }

    #[test]
    fn format_document_no_edit_when_already_formatted() {
        let src = "module T\n\nval x = 1\n";
        let edits = format_document(src).expect("format");
        assert!(edits.is_empty());
    }

    #[test]
    fn format_document_edits_trailing_spaces() {
        // Shared with CLI `fmt --check`: trailing spaces are not ignored.
        let dirty = "module T\n\nval x = 1  \n";
        let edits = format_document(dirty).expect("format");
        assert_eq!(edits.len(), 1);
        let new_text = edits[0]["newText"].as_str().unwrap();
        assert!(!new_text.contains("1  \n"), "got {new_text:?}");
    }

    #[test]
    fn format_document_no_edit_when_only_missing_final_newline() {
        let src = "module T\n\nval x = 1";
        let edits = format_document(src).expect("format");
        assert!(
            edits.is_empty(),
            "missing final newline should match: {edits:?}"
        );
    }

    #[test]
    fn format_range_end_accounts_for_trailing_newline() {
        let messy = "module T\nval x=1\n";
        let edits = format_document(messy).expect("format");
        assert_eq!(edits.len(), 1);
        // Two content lines + trailing `\n` → EOF on empty line 2 (0-based), col 0.
        // (`str::lines().last()` would wrongly report col = len("val x=1").)
        assert_eq!(edits[0]["range"]["end"]["line"], 2);
        assert_eq!(edits[0]["range"]["end"]["character"], 0);
    }

    #[test]
    fn format_document_parse_error_is_err_not_empty() {
        let bad = "module T\nval =\n";
        let err = format_document(bad).expect_err("should fail");
        assert!(err.to_string().contains("parse failed"), "got: {err}");
    }

    #[test]
    fn format_imported_alias_via_loader_surface() {
        // Format uses strict parse (not recovering Analysis). Import aliases must
        // still pretty-print without inventing unbound names.
        let src = r#"module Main
import std.io.{println as log}
val main={log(1)}
"#;
        let edits = format_document(src).expect("import alias must parse for format");
        assert_eq!(edits.len(), 1);
        let pretty = edits[0]["newText"].as_str().unwrap();
        assert!(
            pretty.contains("println as log"),
            "expected import alias preserved, got {pretty:?}"
        );
        assert!(
            pretty.contains("log(1)") || pretty.contains("log (1)"),
            "expected alias call, got {pretty:?}"
        );
    }
}
