//! Diagnostic JSON for LSP publishDiagnostics.

use super::cursor::span_to_range;
use crate::diag::DiagnosticKind;
use lumia_syntax::Span;
use serde_json::{json, Value};

pub(super) fn diag_from_span(src: &str, span: Span, kind: DiagnosticKind, msg: &str) -> Value {
    let mut d = json!({
        "range": span_to_range(src, span),
        "severity": kind.lsp_severity(),
        "source": "lumia",
        "message": kind.format_message(msg)
    });
    if let Some(c) = kind.lsp_code() {
        d["code"] = json!(c);
    }
    d
}

pub(super) fn diag_json(
    line: u32,
    col: u32,
    eline: u32,
    ecol: u32,
    kind: DiagnosticKind,
    msg: &str,
) -> Value {
    let message = kind.format_message(msg);
    let mut d = json!({
        "range": {
            "start": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            "end": { "line": eline.saturating_sub(1), "character": ecol.saturating_sub(1) }
        },
        "severity": kind.lsp_severity(),
        "source": "lumia",
        "message": message
    });
    if let Some(c) = kind.lsp_code() {
        d["code"] = json!(c);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::{diag_from_span, diag_json};
    use crate::diag::DiagnosticKind;
    use crate::lsp::state::{state_lock, State};
    use lumia_syntax::{BytePos, ColumnMetric, Span};
    use rustc_hash::FxHashMap as HashMap;

    fn with_encoding<R>(enc: ColumnMetric, f: impl FnOnce() -> R) -> R {
        let mut guard = state_lock();
        let prev = guard.take();
        *guard = Some(State {
            docs: HashMap::default(),
            analysis: HashMap::default(),
            analyze_tx: None,
            auto_parallel: true,
            client_supports_configuration: false,
            next_req_id: 1,
            pending_config_req: None,
            last_diag_uris: HashMap::default(),
            analyze_gen: HashMap::default(),
            position_encoding: enc,
            shut_down: false,
        });
        drop(guard);
        let out = f();
        *state_lock() = prev;
        out
    }

    #[test]
    fn diag_json_sets_code_from_kind() {
        let d = diag_json(2, 3, 2, 8, DiagnosticKind::Type, "test error");
        assert_eq!(d["range"]["start"]["line"], 1);
        assert_eq!(d["range"]["start"]["character"], 2);
        assert_eq!(d["range"]["end"]["line"], 1);
        assert_eq!(d["range"]["end"]["character"], 7);
        assert_eq!(d["severity"], 1);
        assert_eq!(d["code"], "type");
        assert_eq!(d["source"], "lumia");
        assert_eq!(d["message"], "type: test error");
    }

    #[test]
    fn warning_kind_maps_to_lsp_severity_two() {
        let d = diag_json(1, 1, 1, 2, DiagnosticKind::Warning, "trust advisory");
        assert_eq!(d["severity"], 2);
        assert_eq!(d["code"], "warning");
        assert_eq!(d["message"], "warning: trust advisory");
    }

    #[test]
    fn bare_type_message_still_gets_code() {
        let d = diag_json(1, 1, 1, 2, DiagnosticKind::Type, "unbound `x`");
        assert_eq!(d["code"], "type");
        assert_eq!(d["message"], "type: unbound `x`");
    }

    #[test]
    fn diag_from_span_uses_source_lines() {
        with_encoding(ColumnMetric::Utf16, || {
            let src = "line one\nline two\n";
            let span = Span {
                file: 0,
                start: BytePos(9),
                end: BytePos(12),
            };
            let d = diag_from_span(src, span, DiagnosticKind::Other, "bad");
            assert_eq!(d["range"]["start"]["line"], 1);
            assert_eq!(d["range"]["start"]["character"], 0);
            assert_eq!(d["message"], "bad");
            assert!(d.get("code").is_none());
        });
    }

    #[test]
    fn diag_from_span_utf16_cjk() {
        with_encoding(ColumnMetric::Utf16, || {
            let src = "val 你好 = x\n";
            let x = src.find('x').expect("x") as u32;
            let d = diag_from_span(src, Span::new(x, x + 1), DiagnosticKind::Type, "unbound");
            assert_eq!(d["range"]["start"]["line"], 0);
            assert_eq!(d["range"]["start"]["character"], 9);
            assert_eq!(d["code"], "type");
        });
    }

    #[test]
    fn diagnostics_imported_alias_type_error_via_loader() {
        // Type errors on loader-resolved imports need analyze_buffer (not check_source).
        use crate::lsp::analyze::analyze_buffer;
        let src = r#"
module Main
import std.io.{println as log}
val main: Int = log(1)
"#;
        let (batches, analysis) = analyze_buffer("untitled:Diag-1", src, &HashMap::default());
        assert!(
            analysis.is_some(),
            "loader must typecheck untitled std import enough to emit typed diags"
        );
        let client = batches
            .iter()
            .find(|(u, d)| u == "untitled:Diag-1" && !d.is_empty())
            .expect("type error should publish on client untitled URI");
        let d = &client.1[0];
        assert_eq!(
            d["severity"], 1,
            "type error must be Error severity, got {d:?}"
        );
        assert_eq!(d["source"], "lumia");
        assert!(
            d["code"] == "type" || d["message"].as_str().unwrap_or("").contains("type"),
            "expected type diagnostic, got {d:?}"
        );
    }
}
