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
#[cfg(test)]
mod test_support;
mod cursor;
mod definition;
mod diagnostics;
mod formatting;
mod hover;
mod inlay;
mod signature_help;
mod protocol;
mod code_action;
mod references;
mod semantic;
mod state;
mod symbols;
mod uri;

use analyze::{on_did_change, on_did_change_watched_files, on_did_close, on_did_open};
use anyhow::Result;
use completion::on_completion;
use code_action::on_code_action;
use definition::on_definition;
use formatting::on_formatting;
use hover::on_hover;
use inlay::on_inlay_hint;
use signature_help::on_signature_help;
use protocol::{read_message, write_stdout};
use references::{on_references, on_rename};
use semantic::{on_semantic_tokens, TOKEN_MODIFIERS, TOKEN_TYPES};
use serde_json::{json, Value};
use state::{
    create_session_state, default_state, invalidate_program_cache, set_session_state,
    spawn_analyze_worker, state_lock,
};
use std::io;
use symbols::on_document_symbol;
use uri::uri_to_path;

pub fn run_lsp() -> Result<()> {
    let session_state = create_session_state();
    set_session_state(session_state);
    let analyze_tx = spawn_analyze_worker(session_state);
    *state_lock() = Some(default_state(Some(analyze_tx)));
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

    // After `shutdown`, only `exit` is valid (LSP).
    if method != Some("exit") {
        let shut = state_lock().as_ref().is_some_and(|s| s.shut_down);
        if shut {
            if id.is_some() {
                return Ok(Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32600,
                        "message": "server shut down; only `exit` is allowed"
                    }
                })));
            }
            return Ok(None);
        }
    }

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
            let workspace_folders = parse_workspace_folders(params);
            // Prefer utf-8 when the client offers it; otherwise LSP default utf-16.
            let position_encoding = negotiate_position_encoding(params);
            let position_encoding_str = match position_encoding {
                lumia_syntax::ColumnMetric::Utf8 => "utf-8",
                _ => "utf-16",
            };
            if let Some(s) = state_lock().as_mut() {
                s.auto_parallel = ap;
                s.client_supports_configuration = supports_config;
                s.position_encoding = position_encoding;
                s.workspace_folders = workspace_folders;
            }
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "capabilities": {
                        "positionEncoding": position_encoding_str,
                        "textDocumentSync": {
                            "openClose": true,
                            "change": 2
                        },
                        "hoverProvider": true,
                        "definitionProvider": true,
                        "signatureHelpProvider": {
                            "triggerCharacters": [",", "("]
                        },
                        "referencesProvider": true,
                        "renameProvider": true,
                        "codeActionProvider": {
                            "codeActionKinds": ["quickfix"]
                        },
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
                                "supported": true,
                                "changeNotifications": true
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
            if let Some(s) = state_lock().as_mut() {
                s.shut_down = true;
            }
            if id.is_some() {
                Ok(Some(json!({ "jsonrpc": "2.0", "id": id, "result": null })))
            } else {
                Ok(None)
            }
        }
        Some("exit") => {
            let clean = state_lock().as_ref().is_some_and(|s| s.shut_down);
            std::process::exit(if clean { 0 } else { 1 });
        }
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
        Some("workspace/didChangeWorkspaceFolders") => {
            apply_workspace_folder_change(msg.get("params"))?;
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
        Some("textDocument/signatureHelp") => {
            let result = on_signature_help(msg.get("params"))?;
            Ok(Some(
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            ))
        }
        Some("textDocument/references") => {
            let result = on_references(msg.get("params"))?;
            Ok(Some(
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            ))
        }
        Some("textDocument/rename") => {
            let result = on_rename(msg.get("params"))?;
            Ok(Some(
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            ))
        }
        Some("textDocument/codeAction") => {
            let result = on_code_action(msg.get("params"))?;
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
                let pending = state_lock().as_ref().and_then(|s| s.pending_config_req);
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

fn parse_workspace_folders(params: Option<&Value>) -> Vec<std::path::PathBuf> {
    let mut folders = parse_workspace_folder_list(params.and_then(|p| p.get("workspaceFolders")));
    if folders.is_empty() {
        if let Some(root_uri) = params
            .and_then(|p| p.get("rootUri"))
            .and_then(|v| v.as_str())
        {
            folders.push(uri_to_path(root_uri));
        } else if let Some(root_path) = params
            .and_then(|p| p.get("rootPath"))
            .and_then(|v| v.as_str())
        {
            folders.push(std::path::PathBuf::from(root_path));
        }
    }
    normalize_workspace_folders(folders)
}

fn parse_workspace_folder_list(v: Option<&Value>) -> Vec<std::path::PathBuf> {
    v.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("uri").and_then(|u| u.as_str()))
                .map(uri_to_path)
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_workspace_folders(mut folders: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    folders.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    folders.dedup();
    folders
}

fn apply_workspace_folder_change(params: Option<&Value>) -> Result<()> {
    let added = parse_workspace_folder_list(
        params
            .and_then(|p| p.get("event"))
            .and_then(|e| e.get("added")),
    );
    let removed = parse_workspace_folder_list(
        params
            .and_then(|p| p.get("event"))
            .and_then(|e| e.get("removed")),
    );
    if added.is_empty() && removed.is_empty() {
        return Ok(());
    }
    let docs: Vec<(String, String)> = {
        let mut st = state_lock();
        let Some(s) = st.as_mut() else {
            return Ok(());
        };
        let mut folders = s.workspace_folders.clone();
        folders.retain(|p| !removed.iter().any(|r| r == p));
        for a in added {
            if !folders.iter().any(|p| p == &a) {
                folders.push(a);
            }
        }
        folders = normalize_workspace_folders(folders);
        if folders == s.workspace_folders {
            return Ok(());
        }
        s.workspace_folders = folders;
        invalidate_program_cache(s);
        s.docs.iter().map(|(u, t)| (u.clone(), t.clone())).collect()
    };
    for (uri, text) in docs {
        let _ = analyze::publish_diagnostics_for(&uri, &text);
    }
    Ok(())
}

fn apply_auto_parallel(ap: bool) -> Result<()> {
    let docs: Vec<(String, String)> = {
        let mut st = state_lock();
        if let Some(s) = st.as_mut() {
            if s.auto_parallel != ap {
                invalidate_program_cache(s);
            }
            s.auto_parallel = ap;
            s.docs.iter().map(|(u, t)| (u.clone(), t.clone())).collect()
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

/// LSP 3.17 `general.positionEncodings`: prefer `utf-8`, else default `utf-16`.
fn negotiate_position_encoding(params: Option<&Value>) -> lumia_syntax::ColumnMetric {
    let Some(arr) = params
        .and_then(|p| p.get("capabilities"))
        .and_then(|c| c.get("general"))
        .and_then(|g| g.get("positionEncodings"))
        .and_then(|v| v.as_array())
    else {
        return lumia_syntax::ColumnMetric::Utf16;
    };
    if arr.iter().any(|v| v.as_str() == Some("utf-8")) {
        lumia_syntax::ColumnMetric::Utf8
    } else {
        lumia_syntax::ColumnMetric::Utf16
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

    #[test]
    fn negotiate_prefers_utf8_when_offered() {
        let params = serde_json::json!({
            "capabilities": {
                "general": { "positionEncodings": ["utf-16", "utf-8"] }
            }
        });
        assert_eq!(
            super::negotiate_position_encoding(Some(&params)),
            lumia_syntax::ColumnMetric::Utf8
        );
        assert_eq!(
            super::negotiate_position_encoding(None),
            lumia_syntax::ColumnMetric::Utf16
        );
    }

    #[test]
    fn shutdown_rejects_further_requests() {
        use super::state::{default_state, state_lock};
        crate::lsp::test_support::with_test_lock(|| {
            let prev = state_lock().take();
            *state_lock() = Some(default_state(None));
            let shut = super::handle_message(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "shutdown"
            }))
            .expect("shutdown");
            assert_eq!(shut.unwrap()["result"], serde_json::Value::Null);
            assert!(state_lock().as_ref().unwrap().shut_down);
            let rejected = super::handle_message(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/hover",
                "params": {}
            }))
            .expect("hover after shutdown");
            let err = rejected.unwrap();
            assert_eq!(err["error"]["code"], -32600);
            *state_lock() = prev;
        });
    }

    #[test]
    fn parse_workspace_folders_prefers_workspace_folders_then_root_uri() {
        let from_folders = serde_json::json!({
            "workspaceFolders": [
                { "uri": "file:///tmp/ws-b" },
                { "uri": "file:///tmp/ws-a" }
            ],
            "rootUri": "file:///tmp/root"
        });
        let got = super::parse_workspace_folders(Some(&from_folders));
        assert_eq!(got.len(), 2);
        assert!(got.iter().any(|p| p.to_string_lossy().contains("/tmp/ws-a")));
        assert!(got.iter().any(|p| p.to_string_lossy().contains("/tmp/ws-b")));

        let from_root = serde_json::json!({
            "rootUri": "file:///tmp/root-only"
        });
        let got = super::parse_workspace_folders(Some(&from_root));
        assert_eq!(got.len(), 1);
        assert!(got[0].to_string_lossy().contains("/tmp/root-only"));
    }

    #[test]
    fn workspace_folder_change_updates_state() {
        use super::state::{default_state, state_lock};
        crate::lsp::test_support::with_test_lock(|| {
            let prev = state_lock().take();
            let mut st = default_state(None);
            st.workspace_folders = vec![std::path::PathBuf::from("/tmp/ws-a")];
            *state_lock() = Some(st);
            let _ = super::handle_message(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "workspace/didChangeWorkspaceFolders",
                "params": {
                    "event": {
                        "added": [{ "uri": "file:///tmp/ws-b" }],
                        "removed": [{ "uri": "file:///tmp/ws-a" }]
                    }
                }
            }))
            .expect("workspace folder change");
            let now = state_lock().as_ref().unwrap().workspace_folders.clone();
            assert_eq!(now, vec![std::path::PathBuf::from("/tmp/ws-b")]);
            *state_lock() = prev;
        });
    }
}
