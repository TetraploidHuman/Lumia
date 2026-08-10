//! LSP over stdio (JSON-RPC + Content-Length).
//!
//! - textDocument/didOpen|didChange → publishDiagnostics (editor overlays)
//! - textDocument/hover → type from TypedModule.type_at / fun_types
//! - textDocument/definition → decls (cross-file via Span.file)
//! - textDocument/completion → in-scope names + common methods
//! - textDocument/formatting → `lumia fmt` pretty-print

mod analyze;
mod protocol;

use analyze::{
    on_completion, on_definition, on_did_change, on_did_open, on_formatting, on_hover, state_lock,
    State,
};
use anyhow::Result;
use protocol::{read_message, write_message};
use rustc_hash::FxHashMap as HashMap;
use serde_json::{json, Value};
use std::io;

pub fn run_lsp() -> Result<()> {
    *state_lock() = Some(State {
        docs: HashMap::default(),
        analysis: HashMap::default(),
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

#[cfg(test)]
mod tests {
    use super::analyze::{path_to_uri, uri_to_path};
    use super::protocol::{read_message, MAX_LSP_CONTENT_LENGTH};
    use std::io::Cursor;
    use std::path::{Path, PathBuf};

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
