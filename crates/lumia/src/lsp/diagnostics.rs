//! Diagnostic JSON for LSP publishDiagnostics.

use crate::diag::DiagnosticKind;
use lumia_syntax::{byte_to_line_col, line_starts, Span};
use serde_json::{json, Value};

pub(super) fn diag_from_span(src: &str, span: Span, kind: DiagnosticKind, msg: &str) -> Value {
    let starts = line_starts(src);
    let (line, col) = byte_to_line_col(&starts, span.start);
    let (eline, ecol) = byte_to_line_col(&starts, span.end);
    diag_json(line, col, eline, ecol.max(col + 1), kind, msg)
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
    use lumia_syntax::{BytePos, Span};

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
    fn bare_type_message_still_gets_code() {
        let d = diag_json(1, 1, 1, 2, DiagnosticKind::Type, "unbound `x`");
        assert_eq!(d["code"], "type");
        assert_eq!(d["message"], "type: unbound `x`");
    }

    #[test]
    fn diag_from_span_uses_source_lines() {
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
    }
}
