//! LSP over stdio (JSON-RPC + Content-Length).
//!
//! - textDocument/didOpen|didChange|didClose → publishDiagnostics (editor overlays)
//! - textDocument/hover → type from TypedModule.type_at / fun_types
//! - textDocument/definition → decls (cross-file via Span.file)
//! - textDocument/completion → in-scope names + common methods
//! - textDocument/formatting → `lumia fmt` pretty-print
//! - textDocument/documentSymbol → outline from module items
//! - textDocument/inlayHint → binding / param / call-return types
//! - textDocument/semanticTokens/full → type-aware highlighting

mod analyze;
mod completion;
mod cursor;
mod definition;
mod diagnostics;
mod formatting;
mod hover;
mod inlay;
mod protocol;
mod semantic;
mod state;
mod symbols;
mod uri;

use analyze::{on_did_change, on_did_close, on_did_open};
use anyhow::Result;
use completion::on_completion;
use definition::on_definition;
use formatting::on_formatting;
use hover::on_hover;
use inlay::on_inlay_hint;
use protocol::{read_message, write_stdout};
use rustc_hash::FxHashMap as HashMap;
use semantic::{on_semantic_tokens, TOKEN_MODIFIERS, TOKEN_TYPES};
use serde_json::{json, Value};
use state::{spawn_analyze_worker, state_lock, State};
use std::io;
use symbols::on_document_symbol;

pub fn run_lsp() -> Result<()> {
    let analyze_tx = spawn_analyze_worker();
    *state_lock() = Some(State {
        docs: HashMap::default(),
        analysis: HashMap::default(),
        analyze_tx: Some(analyze_tx),
    });
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    loop {
        let msg = match read_message(&mut stdin)? {
            Some(m) => m,
            None => break,
        };
        if let Some(resp) = handle_message(msg)? {
            write_stdout(&resp)?;
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
                    "completionProvider": {
                        "triggerCharacters": [".", "("],
                        "resolveProvider": false
                    },
                    "documentFormattingProvider": true,
                    "documentSymbolProvider": true,
                    "inlayHintProvider": true,
                    "semanticTokensProvider": {
                        "legend": {
                            "tokenTypes": TOKEN_TYPES,
                            "tokenModifiers": TOKEN_MODIFIERS
                        },
                        "full": true,
                        "range": false
                    }
                },
                "serverInfo": { "name": "lumia-lsp", "version": env!("CARGO_PKG_VERSION") }
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
        Some("textDocument/didClose") => {
            if let Some(params) = msg.get("params") {
                on_did_close(params)?;
            }
            Ok(None)
        }
        Some("textDocument/documentSymbol") => {
            let result = on_document_symbol(msg.get("params"))?;
            Ok(Some(
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            ))
        }
        Some("textDocument/inlayHint") => {
            let result = on_inlay_hint(msg.get("params"))?;
            Ok(Some(
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            ))
        }
        Some("textDocument/semanticTokens/full") => {
            let result = on_semantic_tokens(msg.get("params"))?;
            Ok(Some(
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            ))
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
    use super::protocol::{read_message, MAX_LSP_CONTENT_LENGTH};
    use std::io::Cursor;

    #[test]
    fn read_message_rejects_huge_content_length() {
        let huge = MAX_LSP_CONTENT_LENGTH + 1;
        let raw = format!("Content-Length: {huge}\r\n\r\n");
        let mut cur = Cursor::new(raw.into_bytes());
        let err = read_message(&mut cur).expect_err("must reject oversized body");
        assert!(err.to_string().contains("exceeds limit"), "got {err}");
    }
}
