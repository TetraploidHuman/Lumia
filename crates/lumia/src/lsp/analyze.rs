//! Document analysis: typecheck, diagnostics, publish.

use super::diagnostics::{diag_from_span, diag_json};
use super::protocol::write_stdout;
use super::state::{next_analyze_gen, state_lock, Analysis, AnalyzeReq, auto_parallel};
use super::uri::{path_to_uri, uri_to_path};
use crate::check::{
    check_program_with_overlays, check_source_recovering, OverlayCheckError, PartialCheck,
};
use crate::load::{LoadedProgram, SourceFile};
use anyhow::Result;
use lumia_ty::TypedModule;
use rustc_hash::FxHashMap as HashMap;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(super) fn on_did_open(params: &Value) -> Result<()> {
    let doc = &params["textDocument"];
    let uri = doc["uri"].as_str().unwrap_or("").to_string();
    let text = doc["text"].as_str().unwrap_or("").to_string();
    {
        let mut st = state_lock();
        if let Some(s) = st.as_mut() {
            s.docs.insert(uri.clone(), text.clone());
        }
    }
    let _ = next_analyze_gen(&uri);
    publish_diagnostics_for(&uri, &text)
}

pub(super) fn on_did_change(params: &Value) -> Result<()> {
    let uri = params["textDocument"]["uri"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let text = params["contentChanges"]
        .as_array()
        .and_then(|a| a.last())
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    {
        let mut st = state_lock();
        if let Some(s) = st.as_mut() {
            s.docs.insert(uri.clone(), text.clone());
        }
    }
    let gen = next_analyze_gen(&uri);
    let tx = {
        let st = state_lock();
        st.as_ref().and_then(|s| s.analyze_tx.clone())
    };
    if let Some(tx) = tx {
        let _ = tx.send(AnalyzeReq { uri, text, gen });
        return Ok(());
    }
    publish_diagnostics_for(&uri, &text)
}

pub(super) fn on_did_close(params: &Value) -> Result<()> {
    let uri = params["textDocument"]["uri"]
        .as_str()
        .unwrap_or("")
        .to_string();
    {
        let mut st = state_lock();
        if let Some(s) = st.as_mut() {
            s.docs.remove(&uri);
            s.analysis.remove(&uri);
        }
    }
    write_stdout(&json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "diagnostics": [] }
    }))
}

/// Disk change to a `.lm` file: re-analyze every open buffer (importers stay fresh).
pub(super) fn on_did_change_watched_files(params: &Value) -> Result<()> {
    let Some(changes) = params.get("changes").and_then(|c| c.as_array()) else {
        return Ok(());
    };
    let mut touched_lm = false;
    for ch in changes {
        let uri = ch.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        if uri.ends_with(".lm") {
            touched_lm = true;
            break;
        }
        let path = uri_to_path(uri);
        if path.extension().and_then(|e| e.to_str()) == Some("lm") {
            touched_lm = true;
            break;
        }
    }
    if !touched_lm {
        return Ok(());
    }
    let open: Vec<(String, String)> = {
        let st = state_lock();
        st.as_ref()
            .map(|s| s.docs.iter().map(|(u, t)| (u.clone(), t.clone())).collect())
            .unwrap_or_default()
    };
    for (uri, text) in open {
        let gen = next_analyze_gen(&uri);
        let tx = {
            let st = state_lock();
            st.as_ref().and_then(|s| s.analyze_tx.clone())
        };
        if let Some(tx) = tx {
            let _ = tx.send(AnalyzeReq {
                uri: uri.clone(),
                text: text.clone(),
                gen,
            });
        } else {
            let _ = publish_diagnostics_for(&uri, &text);
        }
    }
    Ok(())
}

/// Analyze and publish diagnostics (sync; also used by the debounce worker).
pub(super) fn publish_diagnostics_for(uri: &str, text: &str) -> Result<()> {
    let overlays = current_overlays();
    let (batches, analysis) = analyze_buffer(uri, text, &overlays);
    // Only replace the cache on success. A local parse/type error must not wipe
    // hover / inlay / completion from the last good analysis.
    if let Some(a) = analysis {
        let mut st = state_lock();
        if let Some(s) = st.as_mut() {
            s.analysis.insert(uri.to_string(), a);
        }
    }
    let mut published = false;
    for (diag_uri, diags) in &batches {
        write_stdout(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": diag_uri, "diagnostics": diags }
        }))?;
        if diag_uri == uri {
            published = true;
        }
    }
    // Clear stale underlines on the edited buffer when the only errors live elsewhere.
    if !published {
        write_stdout(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": [] }
        }))?;
    }
    Ok(())
}

fn current_overlays() -> HashMap<PathBuf, String> {
    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return HashMap::default();
    };
    state
        .docs
        .iter()
        .map(|(uri, text)| (uri_to_path(uri), text.clone()))
        .collect()
}

/// Prefer multi-file load with overlays when the path exists; else buffer-only.
///
/// Returns `(uri → diagnostics)` batches so errors in imported files publish to
/// the correct document URI (BUILD: multi-file diagnostics follow `Span.file`).
fn analyze_buffer(
    uri: &str,
    text: &str,
    overlays: &HashMap<PathBuf, String>,
) -> (Vec<(String, Vec<Value>)>, Option<Analysis>) {
    let path = uri_to_path(uri);
    if path.is_file() || overlays.contains_key(&path) {
        let mut ov = overlays.clone();
        ov.insert(path.clone(), text.to_string());
        match load_and_typecheck(&path, &ov) {
            Ok((loaded, typed)) => {
                let entry_src = loaded
                    .files
                    .first()
                    .map(|f| f.src.clone())
                    .unwrap_or_else(|| text.to_string());
                return (
                    vec![(uri.to_string(), vec![])],
                    Some(Analysis {
                        typed,
                        src: entry_src,
                        buffer_file: buffer_file_id(&loaded.files, &path),
                        files: loaded.files,
                    }),
                );
            }
            Err(load_diags) => {
                // Prefer real multi-file / load diagnostics. Recovering single-buffer
                // analysis must not hide import/dependency failures (Todo).
                if !load_diags.is_empty() {
                    return (load_diags, None);
                }
                let partial = check_source_recovering(text, auto_parallel());
                if partial.typed.is_some() || !partial.diagnostics.is_empty() {
                    let (diags, analysis) = partial_to_lsp(text, &path, partial);
                    return (vec![(uri.to_string(), diags)], analysis);
                }
                return (
                    vec![(
                        uri.to_string(),
                        vec![diag_json(1, 1, 1, 2, "analysis failed")],
                    )],
                    None,
                );
            }
        }
    }
    let (diags, analysis) = partial_to_lsp(text, &path, check_source_recovering(text, auto_parallel()));
    (vec![(uri.to_string(), diags)], analysis)
}

fn partial_to_lsp(
    text: &str,
    path: &Path,
    partial: PartialCheck,
) -> (Vec<Value>, Option<Analysis>) {
    let diags: Vec<Value> = partial
        .diagnostics
        .iter()
        .map(|(span, msg)| diag_from_span(text, *span, msg))
        .collect();
    let analysis = partial.typed.map(|typed| Analysis {
        typed,
        src: text.to_string(),
        buffer_file: 0,
        files: vec![SourceFile {
            path: path.to_path_buf(),
            src: text.to_string(),
        }],
    });
    (diags, analysis)
}

/// Index of `path` in `files`, preferring exact then canonical match (entry is usually 0).
fn buffer_file_id(files: &[SourceFile], path: &Path) -> u32 {
    if let Some(i) = files.iter().position(|f| f.path == path) {
        return i as u32;
    }
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    files
        .iter()
        .position(|f| f.path == canon || f.path.canonicalize().ok().as_ref() == Some(&canon))
        .unwrap_or(0) as u32
}

/// Load + typecheck via shared [`check_program_with_overlays`].
///
/// On failure, each diagnostic is tagged with the URI of `Span.file` (not the
/// entry buffer), so `publishDiagnostics` lands on the correct document.
fn load_and_typecheck(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
) -> Result<(LoadedProgram, TypedModule), Vec<(String, Vec<Value>)>> {
    match check_program_with_overlays(path, overlays, auto_parallel(), None) {
        Ok(v) => Ok(v),
        Err(OverlayCheckError::Load(msg)) => {
            let entry_uri = path_to_uri(path);
            Err(vec![(
                entry_uri,
                vec![diag_from_load_message(text_for_path(path, overlays), &msg)],
            )])
        }
        Err(OverlayCheckError::Analyze { loaded, err }) => {
            let span = err.span().unwrap_or_default();
            let file = loaded
                .files
                .get(span.file as usize)
                .or_else(|| loaded.files.first());
            let (diag_uri, src) = match file {
                Some(f) => (path_to_uri(&f.path), f.src.as_str()),
                None => (path_to_uri(path), ""),
            };
            Err(vec![(
                diag_uri,
                vec![diag_from_span(src, span, err.message())],
            )])
        }
    }
}

fn text_for_path<'a>(path: &Path, overlays: &'a HashMap<PathBuf, String>) -> &'a str {
    overlays.get(path).map(String::as_str).unwrap_or("")
}

/// Pull `line:col` out of `path:line:col: …` load messages so the editor
/// underline matches the CLI caret instead of defaulting to (1,1).
fn diag_from_load_message(src: &str, msg: &str) -> Value {
    let first = msg.lines().next().unwrap_or(msg);
    // `file:line:col: kind: text` — file may contain drive letters / colons on Windows.
    if let Some((line, col, rest)) = parse_line_col_prefix(first) {
        let end_col = col.saturating_add(1);
        return diag_json(line, col, line, end_col, rest);
    }
    if !src.is_empty() {
        // Last resort: keep message, mark start of buffer.
        return diag_json(1, 1, 1, 2, msg);
    }
    diag_json(1, 1, 1, 2, msg)
}

fn parse_line_col_prefix(line: &str) -> Option<(u32, u32, &str)> {
    // Scan for `:digits:digits:` from the rightmost plausible positions.
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b':' {
            i += 1;
            continue;
        }
        let after_path = i + 1;
        let mut j = after_path;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j == after_path || j >= bytes.len() || bytes[j] != b':' {
            i += 1;
            continue;
        }
        let after_line = j + 1;
        let mut k = after_line;
        while k < bytes.len() && bytes[k].is_ascii_digit() {
            k += 1;
        }
        if k == after_line || k >= bytes.len() || bytes[k] != b':' {
            i += 1;
            continue;
        }
        let line_no: u32 = line.get(after_path..j)?.parse().ok()?;
        let col_no: u32 = line.get(after_line..k)?.parse().ok()?;
        let rest = line.get(k + 1..)?.trim_start();
        if line_no >= 1 && col_no >= 1 {
            return Some((line_no, col_no, rest));
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_line_col_prefix;

    #[test]
    fn parse_cli_diag_prefix() {
        let (l, c, rest) =
            parse_line_col_prefix("err.lm:5:1: parse: expected RBrace, found Eof").expect("prefix");
        assert_eq!((l, c), (5, 1));
        assert!(rest.starts_with("parse:"));
    }
}
