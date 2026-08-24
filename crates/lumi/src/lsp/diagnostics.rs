//! Diagnostic JSON for LSP publishDiagnostics.

use lumi_syntax::{byte_to_line_col, line_starts, Span};
use serde_json::{json, Value};

pub(super) fn diag_from_span(src: &str, span: Span, msg: &str) -> Value {
    let starts = line_starts(src);
    let (line, col) = byte_to_line_col(&starts, span.start);
    let (eline, ecol) = byte_to_line_col(&starts, span.end);
    diag_json(line, col, eline, ecol.max(col + 1), msg)
}

pub(super) fn diag_json(line: u32, col: u32, eline: u32, ecol: u32, msg: &str) -> Value {
    json!({
        "range": {
            "start": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            "end": { "line": eline.saturating_sub(1), "character": ecol.saturating_sub(1) }
        },
        "severity": 1,
        "source": "lumi",
        "message": msg
    })
}

#[cfg(test)]
mod tests {
    use super::{diag_from_span, diag_json};
    use lumi_syntax::{BytePos, Span};

    #[test]
    fn diag_json_shape() {
        let d = diag_json(2, 3, 2, 8, "test error");
        assert_eq!(d["range"]["start"]["line"], 1);
        assert_eq!(d["range"]["start"]["character"], 2);
        assert_eq!(d["range"]["end"]["line"], 1);
        assert_eq!(d["range"]["end"]["character"], 7);
        assert_eq!(d["severity"], 1);
        assert_eq!(d["source"], "lumi");
        assert_eq!(d["message"], "test error");
    }

    #[test]
    fn diag_from_span_uses_source_lines() {
        let src = "line one\nline two\n";
        let span = Span {
            file: 0,
            start: BytePos(9),
            end: BytePos(12),
        };
        let d = diag_from_span(src, span, "bad");
        assert_eq!(d["range"]["start"]["line"], 1);
        assert_eq!(d["range"]["start"]["character"], 0);
        assert_eq!(d["message"], "bad");
    }
}
