//! textDocument/references and textDocument/rename.

use super::cursor::ident_at;
use super::state::{state_lock, Analysis};
use super::uri::path_to_uri;
use anyhow::Result;
use lumia_syntax::{byte_to_line_col_metric, line_starts, BytePos, ColumnMetric};
use serde_json::{json, Map, Value};

#[derive(Clone)]
struct RefLoc {
    uri: String,
    start: u32,
    end: u32,
    is_decl: bool,
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_valid_identifier(name: &str) -> bool {
    let mut it = name.bytes();
    let Some(first) = it.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    it.all(is_ident_byte)
}

fn byte_to_position_with_metric(src: &str, byte: u32, metric: ColumnMetric) -> (u32, u32) {
    let starts = line_starts(src);
    let (line, col) = byte_to_line_col_metric(src, &starts, BytePos(byte), metric);
    (line.saturating_sub(1), col.saturating_sub(1))
}

fn range_json(src: &str, start: u32, end: u32, metric: ColumnMetric) -> Value {
    let (sl, sc) = byte_to_position_with_metric(src, start, metric);
    let (el, mut ec) = byte_to_position_with_metric(src, end, metric);
    if sl == el && ec <= sc {
        ec = sc + 1;
    }
    json!({
        "start": { "line": sl, "character": sc },
        "end": { "line": el, "character": ec }
    })
}

fn find_ident_occurrences(src: &str, name: &str) -> Vec<(u32, u32)> {
    let bytes = src.as_bytes();
    let needle = name.as_bytes();
    let n = needle.len();
    if n == 0 || bytes.len() < n {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + n <= bytes.len() {
        if &bytes[i..i + n] == needle {
            let left_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let right_ok = i + n == bytes.len() || !is_ident_byte(bytes[i + n]);
            if left_ok && right_ok {
                out.push((i as u32, (i + n) as u32));
                i += n;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn pos_to_byte_with_metric(src: &str, line: u32, character: u32, metric: ColumnMetric) -> u32 {
    lumia_syntax::pos_to_byte_metric(src, line, character, metric)
}

fn collect_refs(
    a: &Analysis,
    request_uri: &str,
    line: u32,
    character: u32,
    metric: ColumnMetric,
) -> Vec<RefLoc> {
    let byte = pos_to_byte_with_metric(&a.src, line, character, metric);
    let Some(name) = ident_at(&a.src, byte) else {
        return Vec::new();
    };
    let Some(decl) = a.typed.decls.get(&name) else {
        return Vec::new();
    };
    let decl_ident = a
        .files
        .get(decl.file as usize)
        .and_then(|file| {
            find_ident_occurrences(&file.src, &name)
                .into_iter()
                .find(|(start, end)| *start >= decl.start.0 && *end <= decl.end.0)
        });
    let mut out = Vec::new();
    let mut decl_exact_idx: Option<usize> = None;
    let mut decl_fallback_idx: Option<usize> = None;
    for (fi, file) in a.files.iter().enumerate() {
        let uri = if fi as u32 == a.buffer_file || file.path.as_os_str().is_empty() {
            request_uri.to_string()
        } else {
            path_to_uri(&file.path)
        };
        for (start, end) in find_ident_occurrences(&file.src, &name) {
            out.push(RefLoc {
                uri: uri.clone(),
                start,
                end,
                is_decl: false,
            });
            let idx = out.len() - 1;
            if decl.file as usize != fi {
                continue;
            }
            if decl_ident == Some((start, end)) {
                decl_exact_idx = Some(idx);
            }
            if start >= decl.start.0 && end <= decl.end.0 {
                let better = decl_fallback_idx
                    .map(|best| start < out[best].start)
                    .unwrap_or(true);
                if better {
                    decl_fallback_idx = Some(idx);
                }
            }
        }
    }
    if let Some(idx) = decl_exact_idx.or(decl_fallback_idx) {
        out[idx].is_decl = true;
    }
    out.sort_by(|a, b| {
        a.uri
            .cmp(&b.uri)
            .then(a.start.cmp(&b.start))
            .then(a.end.cmp(&b.end))
    });
    out
}

pub(super) fn on_references(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(json!([]));
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let line = params["position"]["line"].as_u64().unwrap_or(0) as u32;
    let character = params["position"]["character"].as_u64().unwrap_or(0) as u32;
    let include_decl = params["context"]["includeDeclaration"]
        .as_bool()
        .unwrap_or(true);

    let (refs, metric, sources): (Vec<RefLoc>, ColumnMetric, std::collections::HashMap<String, String>) = {
        let st = state_lock();
        let Some(state) = st.as_ref() else {
            return Ok(json!([]));
        };
        let Some(a) = state.analysis.get(uri) else {
            return Ok(json!([]));
        };
        let refs = collect_refs(a, uri, line, character, state.position_encoding);
        let mut sources = std::collections::HashMap::new();
        sources.insert(uri.to_string(), a.src.clone());
        for file in &a.files {
            let fu = if file.path.as_os_str().is_empty() {
                uri.to_string()
            } else {
                path_to_uri(&file.path)
            };
            sources.entry(fu).or_insert_with(|| file.src.clone());
        }
        (refs, state.position_encoding, sources)
    };
    let mut out = Vec::new();
    for r in refs {
        if !include_decl && r.is_decl {
            continue;
        }
        let src = sources.get(&r.uri).map(|s| s.as_str()).unwrap_or("");
        out.push(json!({
            "uri": r.uri,
            "range": range_json(src, r.start, r.end, metric)
        }));
    }
    Ok(Value::Array(out))
}

pub(super) fn on_rename(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(Value::Null);
    };
    let new_name = params["newName"].as_str().unwrap_or("");
    if !is_valid_identifier(new_name) {
        return Ok(Value::Null);
    }
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let line = params["position"]["line"].as_u64().unwrap_or(0) as u32;
    let character = params["position"]["character"].as_u64().unwrap_or(0) as u32;

    let (refs, metric, sources): (Vec<RefLoc>, ColumnMetric, std::collections::HashMap<String, String>) = {
        let st = state_lock();
        let Some(state) = st.as_ref() else {
            return Ok(Value::Null);
        };
        let Some(a) = state.analysis.get(uri) else {
            return Ok(Value::Null);
        };
        let refs = collect_refs(a, uri, line, character, state.position_encoding);
        let mut sources = std::collections::HashMap::new();
        sources.insert(uri.to_string(), a.src.clone());
        for file in &a.files {
            let fu = if file.path.as_os_str().is_empty() {
                uri.to_string()
            } else {
                path_to_uri(&file.path)
            };
            sources.entry(fu).or_insert_with(|| file.src.clone());
        }
        (refs, state.position_encoding, sources)
    };
    if refs.is_empty() {
        return Ok(Value::Null);
    }
    let mut changes: Map<String, Value> = Map::new();
    for r in refs {
        let src = sources.get(&r.uri).map(|s| s.as_str()).unwrap_or("");
        let edit = json!({
            "range": range_json(src, r.start, r.end, metric),
            "newText": new_name
        });
        changes
            .entry(r.uri)
            .and_modify(|v| {
                if let Value::Array(arr) = v {
                    arr.push(edit.clone());
                }
            })
            .or_insert_with(|| Value::Array(vec![edit]));
    }
    Ok(json!({ "changes": changes }))
}

#[cfg(test)]
mod tests {
    use super::{on_references, on_rename};
    use crate::check::check_source;
    use crate::load::SourceFile;
    use crate::lsp::cursor::byte_to_position;
    use crate::lsp::state::{default_state, state_lock, Analysis};
    use crate::lsp::test_support::{
        imported_alias_analysis, with_analysis_state, with_encoding, IMPORTED_ALIAS_SRC,
    };
    use lumia_syntax::ColumnMetric;
    use serde_json::json;
    use std::path::PathBuf;

    fn with_state<R>(src: &str, f: impl FnOnce() -> R) -> R {
        let prev = state_lock().take();
        let typed = check_source(src, true).expect("typed");
        let analysis = Analysis::from_typed(
            typed,
            src.to_string(),
            vec![SourceFile {
                path: PathBuf::new(),
                src: src.to_string(),
            }],
            0,
        );
        let mut st = default_state(None);
        st.analysis.insert("file:///demo.lm".to_string(), analysis);
        *state_lock() = Some(st);
        let out = f();
        *state_lock() = prev;
        out
    }

    #[test]
    fn references_include_and_exclude_declaration() {
        let src = r#"
module Demo
val add = { x, y -> x + y }
val main = {
  add(1, 2)
  add(3, 4)
}
"#;
        with_encoding(ColumnMetric::Utf16, || with_state(src, || {
            let byte = src.find("add(1").expect("use site") as u32;
            let (line, character) = byte_to_position(src, byte);
            let with_decl = on_references(Some(&json!({
                "textDocument": { "uri": "file:///demo.lm" },
                "position": { "line": line, "character": character },
                "context": { "includeDeclaration": true }
            })))
            .expect("references");
            let without_decl = on_references(Some(&json!({
                "textDocument": { "uri": "file:///demo.lm" },
                "position": { "line": line, "character": character },
                "context": { "includeDeclaration": false }
            })))
            .expect("references");
            assert_eq!(with_decl.as_array().map(|a| a.len()), Some(3));
            assert_eq!(without_decl.as_array().map(|a| a.len()), Some(2));
        }));
    }

    #[test]
    fn rename_returns_workspace_edit_changes() {
        let src = r#"
module Demo
val add = { x, y -> x + y }
val main = { add(1, 2) }
"#;
        with_encoding(ColumnMetric::Utf16, || with_state(src, || {
            let edit = on_rename(Some(&json!({
                "textDocument": { "uri": "file:///demo.lm" },
                "position": { "line": 3, "character": 13 },
                "newName": "sum"
            })))
            .expect("rename");
            let changes = &edit["changes"]["file:///demo.lm"];
            assert!(changes.is_array(), "rename must return edits: {edit}");
            assert_eq!(changes.as_array().map(|a| a.len()), Some(2));
        }));
    }

    #[test]
    fn references_imported_alias_via_loader() {
        let uri = "untitled:Refs-1";
        let analysis = imported_alias_analysis(uri);
        with_encoding(ColumnMetric::Utf16, || with_analysis_state(uri, analysis, || {
            let byte = IMPORTED_ALIAS_SRC.find("log(1)").expect("log call") as u32;
            let (line, character) = byte_to_position(IMPORTED_ALIAS_SRC, byte);
            let refs = on_references(Some(&json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
                "context": { "includeDeclaration": true }
            })))
            .expect("references");
            let arr = refs.as_array().expect("array");
            assert_eq!(arr.len(), 2, "alias decl + call use expected, got {refs}");
            assert!(
                arr.iter().all(|loc| loc["uri"] == uri),
                "untitled loader refs should map back to client URI, got {refs}"
            );
        }));
    }

    #[test]
    fn rename_imported_alias_via_loader() {
        let uri = "untitled:Rename-1";
        let analysis = imported_alias_analysis(uri);
        with_encoding(ColumnMetric::Utf16, || with_analysis_state(uri, analysis, || {
            let byte = IMPORTED_ALIAS_SRC.find("log(1)").expect("log call") as u32;
            let (line, character) = byte_to_position(IMPORTED_ALIAS_SRC, byte);
            let edit = on_rename(Some(&json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
                "newName": "printLn"
            })))
            .expect("rename");
            let changes = edit["changes"][uri].as_array().expect("rename edits");
            assert_eq!(changes.len(), 2, "alias decl + call use expected, got {edit}");
            assert!(
                changes.iter().all(|c| c["newText"] == "printLn"),
                "all edits should rewrite to requested alias, got {edit}"
            );
        }));
    }
}
