//! textDocument/definition.

use super::cursor::{ident_at, pos_to_byte};
use super::state::{state_lock, Analysis};
use super::uri::path_to_uri;
use anyhow::Result;
use lumia_syntax::{byte_to_line_col, line_starts};
use serde_json::{json, Value};

pub(super) fn definition_for_analysis(a: &Analysis, uri: &str, line: u32, character: u32) -> Value {
    let byte = pos_to_byte(&a.src, line, character);
    let Some(name) = ident_at(&a.src, byte) else {
        return Value::Null;
    };
    let Some(span) = a.typed.decls.get(&name) else {
        return Value::Null;
    };
    let file = a
        .files
        .get(span.file as usize)
        .unwrap_or_else(|| a.files.first().expect("analysis files"));
    let starts = line_starts(&file.src);
    let (sl, sc) = byte_to_line_col(&starts, span.start);
    let (el, ec) = byte_to_line_col(&starts, span.end);
    let target_uri = if file.path.as_os_str().is_empty() {
        uri.to_string()
    } else {
        path_to_uri(&file.path)
    };
    json!({
        "uri": target_uri,
        "range": {
            "start": { "line": sl.saturating_sub(1), "character": sc.saturating_sub(1) },
            "end": { "line": el.saturating_sub(1), "character": ec.saturating_sub(1) }
        }
    })
}

pub(super) fn on_definition(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(Value::Null);
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let line = params["position"]["line"].as_u64().unwrap_or(0) as u32;
    let character = params["position"]["character"].as_u64().unwrap_or(0) as u32;
    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(Value::Null);
    };
    let Some(a) = state.analysis.get(uri) else {
        return Ok(Value::Null);
    };
    Ok(definition_for_analysis(a, uri, line, character))
}
