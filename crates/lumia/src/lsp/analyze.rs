//! Document analysis: typecheck, diagnostics, publish.

use super::diagnostics::{diag_from_span, diag_json};
use super::protocol::write_message;
use super::state::{state_lock, Analysis};
use super::uri::uri_to_path;
use crate::check::{
    check_program_with_overlays, check_source_recovering, OverlayCheckError, PartialCheck,
};
use crate::load::{LoadedProgram, SourceFile};
use anyhow::Result;
use lumia_ty::TypedModule;
use rustc_hash::FxHashMap as HashMap;
use serde_json::{json, Value};
use std::io;
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
    publish_diagnostics(&uri, &text)?;
    Ok(())
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
    publish_diagnostics(&uri, &text)?;
    Ok(())
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
    let mut stdout = io::stdout();
    write_message(
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": [] }
        }),
    )?;
    Ok(())
}

fn publish_diagnostics(uri: &str, text: &str) -> Result<()> {
    let overlays = current_overlays();
    let (diags, analysis) = analyze_buffer(uri, text, &overlays);
    // Only replace the cache on success. A local parse/type error must not wipe
    // hover / inlay / completion from the last good analysis.
    if let Some(a) = analysis {
        let mut st = state_lock();
        if let Some(s) = st.as_mut() {
            s.analysis.insert(uri.to_string(), a);
        }
    }
    let mut stdout = io::stdout();
    write_message(
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": diags }
        }),
    )?;
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
fn analyze_buffer(
    uri: &str,
    text: &str,
    overlays: &HashMap<PathBuf, String>,
) -> (Vec<Value>, Option<Analysis>) {
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
                    vec![],
                    Some(Analysis {
                        typed,
                        src: entry_src,
                        files: loaded.files,
                    }),
                );
            }
            Err(load_diags) => {
                // Recovering buffer check: keep later items after a local parse error.
                let partial = check_source_recovering(text, true);
                if partial.typed.is_some() || !partial.diagnostics.is_empty() {
                    return partial_to_lsp(text, &path, partial);
                }
                if !load_diags.is_empty() {
                    return (refine_load_diags(text, &path, load_diags), None);
                }
                return (vec![diag_json(1, 1, 1, 2, "analysis failed")], None);
            }
        }
    }
    partial_to_lsp(text, &path, check_source_recovering(text, true))
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
        files: vec![SourceFile {
            path: path.to_path_buf(),
            src: text.to_string(),
        }],
    });
    (diags, analysis)
}

/// Load + typecheck via shared [`check_program_with_overlays`].
fn load_and_typecheck(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
) -> Result<(LoadedProgram, TypedModule), Vec<Value>> {
    match check_program_with_overlays(path, overlays, true, false) {
        Ok(v) => Ok(v),
        Err(OverlayCheckError::Load(msg)) => Err(vec![diag_from_load_message(text_for_path(path, overlays), &msg)]),
        Err(OverlayCheckError::Analyze { loaded, err }) => {
            let span = err.span().unwrap_or_default();
            let src = loaded
                .files
                .get(span.file as usize)
                .map(|f| f.src.as_str())
                .or_else(|| loaded.files.first().map(|f| f.src.as_str()))
                .unwrap_or("");
            Err(vec![diag_from_span(src, span, err.message())])
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

fn refine_load_diags(src: &str, path: &Path, diags: Vec<Value>) -> Vec<Value> {
    let _ = path;
    diags
        .into_iter()
        .map(|d| {
            let msg = d.get("message").and_then(|m| m.as_str()).unwrap_or("");
            // Already has a non-trivial range? keep it.
            let sl = d["range"]["start"]["line"].as_u64().unwrap_or(0);
            let sc = d["range"]["start"]["character"].as_u64().unwrap_or(0);
            let el = d["range"]["end"]["line"].as_u64().unwrap_or(0);
            let ec = d["range"]["end"]["character"].as_u64().unwrap_or(0);
            if sl != 0 || sc != 0 || el != 0 || ec > 1 {
                return d;
            }
            diag_from_load_message(src, msg)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_line_col_prefix;

    #[test]
    fn parse_cli_diag_prefix() {
        let (l, c, rest) = parse_line_col_prefix(
            "err.lm:5:1: parse: expected RBrace, found Eof",
        )
        .expect("prefix");
        assert_eq!((l, c), (5, 1));
        assert!(rest.starts_with("parse:"));
    }
}
