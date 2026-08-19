//! Document analysis: typecheck, diagnostics, publish.

use super::diagnostics::{diag_from_span, diag_json};
use super::protocol::write_stdout;
use super::state::{
    auto_parallel, invalidate_program_cache, next_analyze_gen, overlay_fingerprint,
    program_cache_get, program_cache_put, state_lock, Analysis, AnalyzeReq,
};
use super::uri::{path_to_uri, uri_to_path};
use crate::check::{
    check_program_with_overlays_recovering, check_source_recovering, OverlayCheckError,
    PartialCheck,
};
use crate::diag::DiagnosticKind;
use crate::load::{path_in_loaded_files, resolve_ide_entry, LoadedProgram, SourceFile};
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
    let stale = {
        let mut st = state_lock();
        if let Some(s) = st.as_mut() {
            s.docs.remove(&uri);
            s.analysis.remove(&uri);
            s.last_diag_uris.remove(&uri).unwrap_or_default()
        } else {
            Vec::new()
        }
    };
    for diag_uri in stale {
        if diag_uri == uri {
            continue;
        }
        write_stdout(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": diag_uri, "diagnostics": [] }
        }))?;
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
    {
        let mut st = state_lock();
        if let Some(s) = st.as_mut() {
            invalidate_program_cache(s);
        }
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
    let prev = {
        let mut st = state_lock();
        st.as_mut()
            .and_then(|s| s.last_diag_uris.remove(uri))
            .unwrap_or_default()
    };
    let merged = merge_diag_batches(uri, batches, &prev);
    for (diag_uri, diags) in &merged {
        write_stdout(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": diag_uri, "diagnostics": diags }
        }))?;
    }
    {
        let mut st = state_lock();
        if let Some(s) = st.as_mut() {
            s.last_diag_uris.insert(
                uri.to_string(),
                merged.iter().map(|(u, _)| u.clone()).collect(),
            );
        }
    }
    Ok(())
}

/// Ensure the entry URI is published, and clear URIs from the previous analyze
/// of this buffer that are absent from `batches` (stale import underlines).
fn merge_diag_batches(
    entry_uri: &str,
    batches: Vec<(String, Vec<Value>)>,
    prev: &[String],
) -> Vec<(String, Vec<Value>)> {
    let mut by_uri: HashMap<String, Vec<Value>> = HashMap::default();
    for (u, diags) in batches {
        by_uri.insert(u, diags);
    }
    by_uri.entry(entry_uri.to_string()).or_default();
    for u in prev {
        by_uri.entry(u.clone()).or_default();
    }
    // Stable-ish order: entry first, then the rest sorted for determinism.
    let mut rest: Vec<String> = by_uri
        .keys()
        .filter(|u| u.as_str() != entry_uri)
        .cloned()
        .collect();
    rest.sort();
    let mut out = Vec::with_capacity(rest.len() + 1);
    if let Some(diags) = by_uri.remove(entry_uri) {
        out.push((entry_uri.to_string(), diags));
    }
    for u in rest {
        if let Some(diags) = by_uri.remove(&u) {
            out.push((u, diags));
        }
    }
    out
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

/// Analyze via loader + overlays so `import std.*` / package graphs work for
/// unsaved and untitled buffers (not only on-disk `file://` paths).
///
/// Returns `(uri → diagnostics)` batches so errors in imported files publish to
/// the correct document URI (BUILD: multi-file diagnostics follow `Span.file`).
pub(crate) fn analyze_buffer(
    uri: &str,
    text: &str,
    overlays: &HashMap<PathBuf, String>,
) -> (Vec<(String, Vec<Value>)>, Option<Analysis>) {
    // Match `load_program_with_overlays` entry identity so overlay keys hit.
    let path = absolutize_load_entry(&uri_to_path(uri));
    let mut ov = absolutize_overlay_keys(overlays);
    ov.insert(path.clone(), text.to_string());
    // Prefer package Main/main so editing an imported lib sees the full graph.
    let ide_entry = resolve_ide_entry(&path);
    let result = {
        let primary = load_and_typecheck_recovering(&ide_entry, &ov);
        let fall_back = match &primary {
            Ok((loaded, _, _)) => ide_entry != path && !path_in_loaded_files(&loaded.files, &path),
            Err(_) => false,
        };
        if fall_back {
            load_and_typecheck_recovering(&path, &ov)
        } else {
            primary
        }
    };
    match result {
        Ok((loaded, typed, diags)) if diags.is_empty() => {
            let buffer_file = buffer_file_id(&loaded.files, &path);
            let src = loaded
                .files
                .get(buffer_file as usize)
                .map(|f| f.src.clone())
                .unwrap_or_else(|| text.to_string());
            let clears =
                remap_buffer_uri(clear_batches_for_program(uri, &loaded.files), &path, uri);
            (
                clears,
                typed.map(|typed| Analysis::from_typed(typed, src, loaded.files, buffer_file)),
            )
        }
        Ok((loaded, typed, diags)) => {
            let buffer_file = buffer_file_id(&loaded.files, &path);
            let batches = remap_buffer_uri(diagnostics_to_uri_batches(&loaded, &diags), &path, uri);
            let analysis = typed.map(|typed| {
                let src = loaded
                    .files
                    .get(buffer_file as usize)
                    .map(|f| f.src.clone())
                    .unwrap_or_else(|| text.to_string());
                Analysis::from_typed(typed, src, loaded.files, buffer_file)
            });
            (batches, analysis)
        }
        Err(load_diags) => {
            // Prefer real multi-file / load diagnostics. Recovering single-buffer
            // analysis must not hide import/dependency failures (Todo).
            let load_diags = remap_buffer_uri(load_diags, &path, uri);
            if !load_diags.is_empty() {
                return (load_diags, None);
            }
            let partial = check_source_recovering(text, auto_parallel());
            if partial.typed.is_some() || !partial.diagnostics.is_empty() {
                let (diags, analysis) = partial_to_lsp(text, &path, partial);
                return (vec![(uri.to_string(), diags)], analysis);
            }
            (
                vec![(
                    uri.to_string(),
                    vec![diag_json(
                        1,
                        1,
                        1,
                        2,
                        DiagnosticKind::Other,
                        "analysis failed",
                    )],
                )],
                None,
            )
        }
    }
}

/// Same entry absolutization as [`crate::load::load_program_with_overlays`].
fn absolutize_load_entry(path: &Path) -> PathBuf {
    if path.exists() {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn absolutize_overlay_keys(overlays: &HashMap<PathBuf, String>) -> HashMap<PathBuf, String> {
    let mut out = HashMap::default();
    for (p, src) in overlays {
        out.insert(absolutize_load_entry(p), src.clone());
    }
    out
}

/// Loader stamps files with filesystem-style paths; map the open buffer back to
/// the client document URI (`untitled:…` / unsaved `file://`).
fn remap_buffer_uri(
    batches: Vec<(String, Vec<Value>)>,
    buffer_path: &Path,
    client_uri: &str,
) -> Vec<(String, Vec<Value>)> {
    let loaded_uri = path_to_uri(buffer_path);
    if loaded_uri == client_uri {
        return batches;
    }
    batches
        .into_iter()
        .map(|(u, d)| {
            if u == loaded_uri {
                (client_uri.to_string(), d)
            } else {
                (u, d)
            }
        })
        .collect()
}

fn partial_to_lsp(
    text: &str,
    path: &Path,
    partial: PartialCheck,
) -> (Vec<Value>, Option<Analysis>) {
    let diags: Vec<Value> = partial
        .diagnostics
        .iter()
        .map(|d| diag_from_span(text, d.span, d.kind, &d.message))
        .collect();
    let analysis = partial.typed.map(|typed| {
        Analysis::from_typed(
            typed,
            text.to_string(),
            vec![SourceFile {
                path: path.to_path_buf(),
                src: text.to_string(),
            }],
            0,
        )
    });
    (diags, analysis)
}

/// Empty diagnostic batches for the entry URI and every loaded source file.
fn clear_batches_for_program(entry_uri: &str, files: &[SourceFile]) -> Vec<(String, Vec<Value>)> {
    let mut seen = HashMap::<String, ()>::default();
    let mut out = Vec::new();
    let mut push = |u: String| {
        if seen.insert(u.clone(), ()).is_none() {
            out.push((u, Vec::new()));
        }
    };
    push(entry_uri.to_string());
    for f in files {
        push(path_to_uri(&f.path));
    }
    out
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

/// Load + recovering typecheck via [`check_program_with_overlays_recovering`].
///
/// On load failure, each diagnostic is tagged with the entry URI. Soft lower/type
/// diagnostics are grouped by `Span.file` so `publishDiagnostics` lands correctly.
fn load_and_typecheck_recovering(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
) -> Result<
    (
        LoadedProgram,
        Option<TypedModule>,
        Vec<crate::diag::Diagnostic>,
    ),
    Vec<(String, Vec<Value>)>,
> {
    let ap = auto_parallel();
    let overlay_fp = overlay_fingerprint(overlays);
    if let Some(partial) = {
        let st = state_lock();
        st.as_ref()
            .and_then(|s| program_cache_get(s, path, overlay_fp, ap))
            .cloned()
    } {
        return Ok((partial.loaded, partial.typed, partial.diagnostics));
    }
    match check_program_with_overlays_recovering(path, overlays, ap, None) {
        Ok(partial) => {
            if let Some(s) = state_lock().as_mut() {
                program_cache_put(
                    s,
                    path.to_path_buf(),
                    overlay_fp,
                    ap,
                    partial.clone(),
                );
            }
            Ok((partial.loaded, partial.typed, partial.diagnostics))
        }
        Err(OverlayCheckError::Load(msg)) => {
            let entry_uri = path_to_uri(path);
            Err(vec![(
                entry_uri,
                vec![diag_from_load_message(text_for_path(path, overlays), &msg)],
            )])
        }
        Err(OverlayCheckError::Analyze { .. }) => {
            // Recovering API returns Ok(PartialProgramCheck); Analyze is only for
            // the fail-fast wrapper. Treat as opaque failure.
            Err(vec![(
                path_to_uri(path),
                vec![diag_json(
                    1,
                    1,
                    1,
                    2,
                    DiagnosticKind::Other,
                    "analysis failed",
                )],
            )])
        }
    }
}

fn diagnostics_to_uri_batches(
    loaded: &LoadedProgram,
    diags: &[crate::diag::Diagnostic],
) -> Vec<(String, Vec<Value>)> {
    let mut by_uri: HashMap<String, Vec<Value>> = HashMap::default();
    // Clear every file in the load graph so prior underlines do not linger on
    // clean imports when only some files still error.
    for f in &loaded.files {
        by_uri.entry(path_to_uri(&f.path)).or_default();
    }
    for d in diags {
        let file = loaded
            .files
            .get(d.span.file as usize)
            .or_else(|| loaded.files.first());
        let (uri, src) = match file {
            Some(f) => (path_to_uri(&f.path), f.src.as_str()),
            None => continue,
        };
        by_uri
            .entry(uri)
            .or_default()
            .push(diag_from_span(src, d.span, d.kind, &d.message));
    }
    let entry_uri = loaded
        .files
        .first()
        .map(|f| path_to_uri(&f.path))
        .unwrap_or_default();
    let mut rest: Vec<String> = by_uri
        .keys()
        .filter(|u| u.as_str() != entry_uri)
        .cloned()
        .collect();
    rest.sort();
    let mut out = Vec::with_capacity(rest.len() + 1);
    if let Some(diags) = by_uri.remove(&entry_uri) {
        out.push((entry_uri, diags));
    }
    for u in rest {
        if let Some(diags) = by_uri.remove(&u) {
            out.push((u, diags));
        }
    }
    out
}

fn text_for_path<'a>(path: &Path, overlays: &'a HashMap<PathBuf, String>) -> &'a str {
    if let Some(s) = overlays.get(path) {
        return s.as_str();
    }
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(s) = overlays.get(&canon) {
        return s.as_str();
    }
    for (k, v) in overlays {
        if k.canonicalize().ok().as_ref() == Some(&canon) {
            return v.as_str();
        }
    }
    ""
}

/// Pull `line:col` out of `path:line:col: …` load messages so the editor
/// underline matches the CLI caret instead of defaulting to (1,1).
fn diag_from_load_message(src: &str, msg: &str) -> Value {
    let first = msg.lines().next().unwrap_or(msg);
    // `file:line:col: kind: text` — file may contain drive letters / colons on Windows.
    if let Some((line, col, rest)) = parse_line_col_prefix(first) {
        let end_col = col.saturating_add(1);
        let kind = DiagnosticKind::from_message_prefix(rest);
        return diag_json(line, col, line, end_col, kind, rest);
    }
    if !src.is_empty() {
        // Last resort: keep message, mark start of buffer.
        return diag_json(1, 1, 1, 2, DiagnosticKind::Other, msg);
    }
    diag_json(1, 1, 1, 2, DiagnosticKind::Other, msg)
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
    use super::{
        analyze_buffer, clear_batches_for_program, merge_diag_batches, parse_line_col_prefix,
    };
    use crate::check::check_source_recovering;
    use crate::load::SourceFile;
    use rustc_hash::FxHashMap as HashMap;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn untitled_buffer_resolves_std_import() {
        // Import aliases require loader injection; builtins alone do not bind `log`.
        let src = r#"
module Main
import std.io.{println as log}
val main = { log(1) }
"#;
        // Baseline: single-buffer check has no loader / std graph.
        let partial = check_source_recovering(src, true);
        let unbound = partial
            .diagnostics
            .iter()
            .any(|d| d.message.contains("unbound") && d.message.contains("log"));
        assert!(
            unbound,
            "check_source_recovering must leave `log` unbound; diags={:?}",
            partial
                .diagnostics
                .iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
        );
        let (batches, analysis) = analyze_buffer("untitled:Untitled-1", src, &HashMap::default());
        assert!(
            analysis.is_some(),
            "untitled buffer should typecheck via loader+std"
        );
        let diags: Vec<&serde_json::Value> = batches.iter().flat_map(|(_, d)| d).collect();
        assert!(
            diags.is_empty(),
            "unexpected diagnostics on clean untitled std import: {diags:?}"
        );
        assert!(
            batches.iter().any(|(u, _)| u == "untitled:Untitled-1"),
            "diagnostics must publish under the client untitled URI, got {batches:?}"
        );
    }

    #[test]
    fn untitled_buffer_type_error_on_client_uri() {
        let src = r#"
module Main
import std.io.{println as log}
val main: Int = log(1)
"#;
        let (batches, _) = analyze_buffer("untitled:Untitled-2", src, &HashMap::default());
        let client = batches
            .iter()
            .find(|(u, d)| u == "untitled:Untitled-2" && !d.is_empty())
            .expect("type error should land on client untitled URI");
        assert!(
            !client.1.is_empty(),
            "expected at least one diagnostic on untitled URI"
        );
    }

    #[test]
    fn parse_cli_diag_prefix() {
        let (l, c, rest) =
            parse_line_col_prefix("err.lm:5:1: parse: expected RBrace, found Eof").expect("prefix");
        assert_eq!((l, c), (5, 1));
        assert!(rest.starts_with("parse:"));
    }

    #[test]
    fn success_clears_all_loaded_files() {
        let files = vec![
            SourceFile {
                path: PathBuf::from("/tmp/a.lm"),
                src: String::new(),
            },
            SourceFile {
                path: PathBuf::from("/tmp/b.lm"),
                src: String::new(),
            },
        ];
        let batches = clear_batches_for_program("file:///tmp/a.lm", &files);
        assert!(batches.iter().all(|(_, d)| d.is_empty()));
        let uris: Vec<&str> = batches.iter().map(|(u, _)| u.as_str()).collect();
        assert!(uris.contains(&"file:///tmp/a.lm"));
        assert!(uris.contains(&"file:///tmp/b.lm"));
    }

    #[test]
    fn merge_clears_stale_import_uri() {
        let err = json!({"message": "boom"});
        let batches = vec![("file:///entry.lm".into(), vec![])];
        let prev = vec!["file:///entry.lm".into(), "file:///import.lm".into()];
        let merged = merge_diag_batches("file:///entry.lm", batches, &prev);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].0, "file:///entry.lm");
        assert!(merged[0].1.is_empty());
        assert_eq!(merged[1].0, "file:///import.lm");
        assert!(merged[1].1.is_empty());
        let _ = err;
    }
}
