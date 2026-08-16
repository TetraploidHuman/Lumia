//! LSP over stdio (JSON-RPC + Content-Length).
//!
//! - textDocument/didOpen|didChange|didClose → publishDiagnostics (editor overlays)
//! - workspace/didChangeWatchedFiles → re-analyze open buffers (import dependents)
//! - workspace/didChangeConfiguration + workspace/configuration → `lumia.autoParallel`
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

use analyze::{on_did_change, on_did_change_watched_files, on_did_close, on_did_open};
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
        auto_parallel: true,
        client_supports_configuration: false,
        next_req_id: 1,
        pending_config_req: None,
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
        Some("initialize") => {
            let params = msg.get("params");
            let opts = params.and_then(|p| p.get("initializationOptions"));
            let ap = opts
                .and_then(|o| {
                    o.get("autoParallel")
                        .or_else(|| o.get("auto_parallel"))
                        .and_then(|v| v.as_bool())
                })
                .unwrap_or(true);
            let supports_config = params
                .and_then(|p| p.get("capabilities"))
                .and_then(|c| c.get("workspace"))
                .and_then(|w| w.get("configuration"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if let Some(s) = state_lock().as_mut() {
                s.auto_parallel = ap;
                s.client_supports_configuration = supports_config;
            }
            Ok(Some(json!({
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
                    },
                    "workspace": {
                        // Client may push `workspace/didChangeConfiguration`; we also
                        // pull via `workspace/configuration` after `initialized`.
                        "workspaceFolders": {
                            "supported": false,
                            "changeNotifications": false
                        }
                    }
                },
                "serverInfo": { "name": "lumia-lsp", "version": env!("CARGO_PKG_VERSION") }
            }
        })))
        }
        Some("initialized") => {
            request_workspace_configuration()?;
            if id.is_some() {
                Ok(Some(json!({ "jsonrpc": "2.0", "id": id, "result": null })))
            } else {
                Ok(None)
            }
        }
        Some("shutdown") => {
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
        Some("workspace/didChangeWatchedFiles") => {
            if let Some(params) = msg.get("params") {
                on_did_change_watched_files(params)?;
            }
            Ok(None)
        }
        Some("workspace/didChangeConfiguration") => {
            let settings = msg.get("params").and_then(|p| p.get("settings"));
            let ap = parse_auto_parallel_settings(settings);
            if let Some(ap) = ap {
                apply_auto_parallel(ap)?;
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
        Some("textDocument/formatting") => match on_formatting(msg.get("params")) {
            Ok(result) => Ok(Some(
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            )),
            Err(e) => Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32603,
                    "message": format!("{e:#}")
                }
            }))),
        },
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
        None => {
            // Response to a server→client request (e.g. workspace/configuration).
            if let Some(rid) = json_rpc_id_i64(msg.get("id")) {
                let pending = state_lock()
                    .as_ref()
                    .and_then(|s| s.pending_config_req);
                if pending == Some(rid) {
                    if let Some(s) = state_lock().as_mut() {
                        s.pending_config_req = None;
                    }
                    // `workspace/configuration` returns an array parallel to `items`.
                    let ap = msg
                        .get("result")
                        .and_then(|r| r.as_array())
                        .and_then(|a| a.first())
                        .and_then(|v| {
                            v.get("autoParallel")
                                .or_else(|| v.get("auto_parallel"))
                                .and_then(|b| b.as_bool())
                                .or_else(|| parse_auto_parallel_settings(Some(v)))
                        });
                    if let Some(ap) = ap {
                        apply_auto_parallel(ap)?;
                    }
                }
            }
            Ok(None)
        }
    }
}

fn json_rpc_id_i64(id: Option<&Value>) -> Option<i64> {
    let id = id?;
    id.as_i64()
        .or_else(|| id.as_u64().and_then(|u| i64::try_from(u).ok()))
}

fn parse_auto_parallel_settings(settings: Option<&Value>) -> Option<bool> {
    settings.and_then(|s| {
        s.get("lumia")
            .and_then(|l| l.get("autoParallel"))
            .or_else(|| s.get("autoParallel"))
            .or_else(|| s.get("auto_parallel"))
            .and_then(|v| v.as_bool())
    })
}

fn apply_auto_parallel(ap: bool) -> Result<()> {
    let docs: Vec<(String, String)> = {
        let mut st = state_lock();
        if let Some(s) = st.as_mut() {
            s.auto_parallel = ap;
            s.docs
                .iter()
                .map(|(u, t)| (u.clone(), t.clone()))
                .collect()
        } else {
            Vec::new()
        }
    };
    for (uri, text) in docs {
        let _ = analyze::publish_diagnostics_for(&uri, &text);
    }
    Ok(())
}

/// Pull `lumia.*` settings when the client supports `workspace/configuration`.
fn request_workspace_configuration() -> Result<()> {
    let req_id = {
        let mut st = state_lock();
        let Some(s) = st.as_mut() else {
            return Ok(());
        };
        if !s.client_supports_configuration {
            return Ok(());
        }
        let id = s.next_req_id;
        s.next_req_id += 1;
        s.pending_config_req = Some(id);
        id
    };
    write_stdout(&json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "workspace/configuration",
        "params": {
            "items": [{ "section": "lumia" }]
        }
    }))
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
