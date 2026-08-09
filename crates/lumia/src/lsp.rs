//! LSP over stdio (JSON-RPC + Content-Length).
//!
//! - textDocument/didOpen|didChange → publishDiagnostics (editor overlays)
//! - textDocument/hover → type from TypedModule.type_at / fun_types
//! - textDocument/definition → decls (cross-file via Span.file)
//! - textDocument/completion → in-scope names + common methods
//! - textDocument/formatting → `lumia fmt` pretty-print

use crate::load::{load_program_with_overlays, LoadedProgram, SourceFile};
use anyhow::Result;
use lumia_hir::lower_module;
use lumia_syntax::{
    byte_to_line_col, format_module_src, line_starts, parse_module, stamp_module, BytePos, Span,
};
use lumia_ty::{
    check_effect_boundaries, finalize_auto_parallel, infer_module_with_visibility, Type,
    TypedModule,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct Analysis {
    typed: TypedModule,
    /// Primary document source (for hover/completion cursor).
    src: String,
    files: Vec<SourceFile>,
}

struct State {
    docs: HashMap<String, String>,
    /// uri → last successful analysis
    analysis: HashMap<String, Analysis>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn state_lock() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn run_lsp() -> Result<()> {
    *state_lock() = Some(State {
        docs: HashMap::new(),
        analysis: HashMap::new(),
    });
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut stdout = io::stdout();
    loop {
        let msg = match read_message(&mut stdin)? {
            Some(m) => m,
            None => break,
        };
        if let Some(resp) = handle_message(msg)? {
            write_message(&mut stdout, &resp)?;
        }
    }
    Ok(())
}

/// Cap LSP message bodies so a malicious/buggy client cannot OOM the server.
const MAX_LSP_CONTENT_LENGTH: usize = 16 * 1024 * 1024;

fn read_message(r: &mut impl BufRead) -> Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = Some(rest.trim().parse::<usize>()?);
        }
    }
    let len = match content_length {
        Some(l) => l,
        None => return Ok(None),
    };
    if len > MAX_LSP_CONTENT_LENGTH {
        anyhow::bail!("LSP Content-Length {len} exceeds limit {MAX_LSP_CONTENT_LENGTH}");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

fn write_message(w: &mut impl Write, v: &Value) -> Result<()> {
    let body = serde_json::to_vec(v)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()?;
    Ok(())
}

fn handle_message(msg: Value) -> Result<Option<Value>> {
    let method = msg.get("method").and_then(|m| m.as_str());
    let id = msg.get("id").cloned();
    match method {
        Some("initialize") => Ok(Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "capabilities": {
                    "textDocumentSync": 1,
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "completionProvider": { "triggerCharacters": ["."] },
                    "documentFormattingProvider": true
                },
                "serverInfo": { "name": "lumia-lsp", "version": "0.3.0" }
            }
        }))),
        Some("initialized") | Some("shutdown") => {
            if id.is_some() {
                Ok(Some(json!({ "jsonrpc": "2.0", "id": id, "result": null })))
            } else {
                Ok(None)
            }
        }
        Some("exit") => std::process::exit(0),
        Some("textDocument/didOpen") => {
            if let Some(params) = msg.get("params") {
                on_did_open(params)?;
            }
            Ok(None)
        }
        Some("textDocument/didChange") => {
            if let Some(params) = msg.get("params") {
                on_did_change(params)?;
            }
            Ok(None)
        }
        Some("textDocument/hover") => {
            let result = on_hover(msg.get("params"))?;
            Ok(Some(
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            ))
        }
        Some("textDocument/definition") => {
            let result = on_definition(msg.get("params"))?;
            Ok(Some(
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            ))
        }
        Some("textDocument/completion") => {
            let result = on_completion(msg.get("params"))?;
            Ok(Some(
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            ))
        }
        Some("textDocument/formatting") => {
            let result = on_formatting(msg.get("params"))?;
            Ok(Some(
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            ))
        }
        Some(_) => {
            if id.is_some() {
                Ok(Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": "Method not found" }
                })))
            } else {
                Ok(None)
            }
        }
        None => Ok(None),
    }
}

fn uri_to_path(uri: &str) -> PathBuf {
    let rest = match uri.strip_prefix("file:") {
        Some(r) => r,
        None => return PathBuf::from(uri),
    };
    // Accept `file:///path`, `file://localhost/path`, and `file:/path`.
    let path_part = if let Some(after_slashes) = rest.strip_prefix("//") {
        if let Some(slash) = after_slashes.find('/') {
            let host = &after_slashes[..slash];
            if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
                &after_slashes[slash..]
            } else {
                // Non-local hosts are not supported; still take the path segment.
                &after_slashes[slash..]
            }
        } else {
            after_slashes
        }
    } else {
        rest
    };
    let decoded = percent_decode(path_part);
    // `file:///C:/Users/...` yields `/C:/Users/...`; strip the extra slash so
    // Windows APIs see a drive-letter path.
    let bytes = decoded.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return PathBuf::from(&decoded[1..]);
    }
    PathBuf::from(decoded)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    // RFC 8089: absolute paths use `file:///…`. Windows drive paths need a
    // leading slash (`file:///C:/…`); bare `file://C:/…` treats `C:` as host.
    let path_str: std::borrow::Cow<'_, str> = if s.starts_with('/') {
        s
    } else if s.len() >= 2 && s.as_bytes().get(1) == Some(&b':') {
        std::borrow::Cow::Owned(format!("/{s}"))
    } else {
        std::borrow::Cow::Owned(format!("/{s}"))
    };
    let mut enc = String::from("file://");
    for &b in path_str.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'_' | b'-' | b'.' | b'~' | b':' => {
                enc.push(b as char)
            }
            _ => enc.push_str(&format!("%{b:02X}")),
        }
    }
    enc
}

fn on_did_open(params: &Value) -> Result<()> {
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

fn on_did_change(params: &Value) -> Result<()> {
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

fn publish_diagnostics(uri: &str, text: &str) -> Result<()> {
    let overlays = current_overlays();
    let (diags, analysis) = analyze_buffer(uri, text, &overlays);
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
        return HashMap::new();
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
            Err(diags) => {
                if !diags.is_empty() {
                    return (diags, None);
                }
                // Fall through to single-buffer parse for a located span when possible.
                if let Err((span, msg)) = check_source(text) {
                    return (vec![diag_from_span(text, span, &msg)], None);
                }
                return (vec![diag_json(1, 1, 1, 2, "analysis failed")], None);
            }
        }
    }
    match check_source(text) {
        Ok(typed) => (
            vec![],
            Some(Analysis {
                typed,
                src: text.to_string(),
                files: vec![SourceFile {
                    path: path.clone(),
                    src: text.to_string(),
                }],
            }),
        ),
        Err((span, msg)) => (vec![diag_from_span(text, span, &msg)], None),
    }
}

/// Load + lower + infer, preserving spans in diagnostics (no `anyhow!(message)`).
fn load_and_typecheck(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
) -> Result<(LoadedProgram, TypedModule), Vec<Value>> {
    let loaded = load_program_with_overlays(path, overlays)
        .map_err(|e| vec![diag_json(1, 1, 1, 2, &format!("{e}"))])?;
    let entry_src = loaded.files.first().map(|f| f.src.as_str()).unwrap_or("");
    let hir = lower_module(&loaded.module)
        .map_err(|e| vec![diag_from_span(entry_src, e.span, &e.message)])?;
    let mut typed = infer_module_with_visibility(&hir, loaded.visibility.clone()).map_err(|e| {
        let span = e.span().unwrap_or_default();
        vec![diag_from_span(entry_src, span, e.message())]
    })?;
    finalize_auto_parallel(&mut typed, true);
    check_effect_boundaries(&typed).map_err(|e| {
        let span = e.span().unwrap_or_default();
        vec![diag_from_span(entry_src, span, e.message())]
    })?;
    Ok((loaded, typed))
}

fn check_source(text: &str) -> Result<TypedModule, (Span, String)> {
    let mut m = parse_module(text).map_err(|e| (e.span, e.message))?;
    stamp_module(&mut m, 0);
    let hir = lower_module(&m).map_err(|e| (e.span, e.message))?;
    let mut typed = infer_module_with_visibility(&hir, Default::default())
        .map_err(|e| (e.span().unwrap_or_default(), e.message().to_string()))?;
    finalize_auto_parallel(&mut typed, true);
    check_effect_boundaries(&typed)
        .map_err(|e| (e.span().unwrap_or_default(), e.message().to_string()))?;
    Ok(typed)
}

fn diag_from_span(src: &str, span: Span, msg: &str) -> Value {
    let starts = line_starts(src);
    let (line, col) = byte_to_line_col(&starts, span.start);
    let (eline, ecol) = byte_to_line_col(&starts, span.end);
    diag_json(line, col, eline, ecol.max(col + 1), msg)
}

fn diag_json(line: u32, col: u32, eline: u32, ecol: u32, msg: &str) -> Value {
    json!({
        "range": {
            "start": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            "end": { "line": eline.saturating_sub(1), "character": ecol.saturating_sub(1) }
        },
        "severity": 1,
        "source": "lumia",
        "message": msg
    })
}

fn pos_to_byte(src: &str, line: u32, character: u32) -> u32 {
    let starts = line_starts(src);
    let idx = line as usize;
    let start = starts.get(idx).copied().unwrap_or(0);
    start.saturating_add(character)
}

fn on_hover(params: Option<&Value>) -> Result<Value> {
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
    let byte = pos_to_byte(&a.src, line, character);
    // Prefer tightest spanning type_at entry.
    let mut best: Option<&(Span, Type)> = None;
    for entry in &a.typed.type_at {
        let (sp, _) = entry;
        if sp.file == 0 && sp.start.0 <= byte && byte < sp.end.0.max(sp.start.0 + 1) {
            match best {
                None => best = Some(entry),
                Some((bsp, _)) => {
                    let bw = bsp.end.0.saturating_sub(bsp.start.0);
                    let w = sp.end.0.saturating_sub(sp.start.0);
                    if w < bw {
                        best = Some(entry);
                    }
                }
            }
        }
    }
    if let Some((_, ty)) = best {
        return Ok(json!({
            "contents": {
                "kind": "markdown",
                "value": format!("```lumia\n{ty}\n```")
            }
        }));
    }
    if let Some(name) = ident_at(&a.src, byte) {
        if let Some(ty) = a.typed.fun_types.get(&name) {
            return Ok(json!({
                "contents": {
                    "kind": "markdown",
                    "value": format!("```lumia\n{name}: {ty}\n```")
                }
            }));
        }
    }
    Ok(Value::Null)
}

fn on_definition(params: Option<&Value>) -> Result<Value> {
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
    let byte = pos_to_byte(&a.src, line, character);
    let Some(name) = ident_at(&a.src, byte) else {
        return Ok(Value::Null);
    };
    let Some(span) = a.typed.decls.get(&name) else {
        return Ok(Value::Null);
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
    Ok(json!({
        "uri": target_uri,
        "range": {
            "start": { "line": sl.saturating_sub(1), "character": sc.saturating_sub(1) },
            "end": { "line": el.saturating_sub(1), "character": ec.saturating_sub(1) }
        }
    }))
}

fn on_completion(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(json!([]));
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(json!([]));
    };
    let mut items = Vec::new();
    let methods = [
        "map", "filter", "fold", "flatMap", "len", "get", "set", "contains", "items", "keys",
        "values", "sortBy", "take", "reverse", "concat", "join", "trim", "split", "toLower",
        "toUpper",
    ];
    for m in methods {
        items.push(json!({ "label": m, "kind": 2 })); // Method
    }
    if let Some(a) = state.analysis.get(uri) {
        for name in a.typed.fun_types.keys() {
            items.push(json!({ "label": name, "kind": 3 })); // Function
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
    Ok(Value::Array(items))
}

fn on_formatting(params: Option<&Value>) -> Result<Value> {
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
    let mut m = match parse_module(text) {
        Ok(m) => m,
        Err(_) => return Ok(json!([])),
    };
    stamp_module(&mut m, 0);
    let formatted = format_module_src(&m);
    if formatted == *text {
        return Ok(json!([]));
    }
    let starts = line_starts(text);
    let last_line = starts.len().saturating_sub(1) as u32;
    let last_col = text.lines().last().map(|l| l.len() as u32).unwrap_or(0);
    Ok(json!([{
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": last_line, "character": last_col }
        },
        "newText": formatted
    }]))
}

fn ident_at(src: &str, byte: u32) -> Option<String> {
    let bytes = src.as_bytes();
    let mut i = byte as usize;
    if i >= bytes.len() {
        i = bytes.len().saturating_sub(1);
    }
    while i > 0 && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i -= 1;
    }
    if i < bytes.len() && !(bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    let start = i;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    if start >= i {
        return None;
    }
    std::str::from_utf8(&bytes[start..i])
        .ok()
        .map(|s| s.to_string())
}

#[allow(dead_code)]
fn _byte_pos(p: u32) -> BytePos {
    BytePos(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn uri_to_path_decodes_and_strips_file_prefix() {
        let p = uri_to_path("file:///tmp/hello%20world.lm");
        assert_eq!(p, PathBuf::from("/tmp/hello world.lm"));
        let p = uri_to_path("file://localhost/tmp/x.lm");
        assert_eq!(p, PathBuf::from("/tmp/x.lm"));
        let p = uri_to_path("file:///C:/Users/me/x.lm");
        assert_eq!(p, PathBuf::from("C:/Users/me/x.lm"));
        assert_eq!(
            path_to_uri(Path::new("C:/Users/me/x.lm")),
            "file:///C:/Users/me/x.lm"
        );
    }

    #[test]
    fn read_message_rejects_huge_content_length() {
        let huge = MAX_LSP_CONTENT_LENGTH + 1;
        let raw = format!("Content-Length: {huge}\r\n\r\n");
        let mut cur = Cursor::new(raw.into_bytes());
        let err = read_message(&mut cur).expect_err("must reject oversized body");
        assert!(err.to_string().contains("exceeds limit"), "got {err}");
    }
}
