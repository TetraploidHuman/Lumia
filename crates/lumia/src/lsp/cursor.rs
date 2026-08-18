//! Cursor position ↔ source byte offset helpers (LSP position encodings).

use super::state::position_encoding;
use lumia_syntax::{
    byte_to_line_col_metric, line_starts, measure_str, pos_to_byte_metric, BytePos, Span,
};

pub(super) fn pos_to_byte(src: &str, line: u32, character: u32) -> u32 {
    pos_to_byte_metric(src, line, character, position_encoding())
}

/// 0-based LSP `(line, character)` for a byte offset under the negotiated encoding.
pub(super) fn byte_to_position(src: &str, byte: u32) -> (u32, u32) {
    let starts = line_starts(src);
    let (line, col) = byte_to_line_col_metric(src, &starts, BytePos(byte), position_encoding());
    (line.saturating_sub(1), col.saturating_sub(1))
}

pub(super) fn span_to_range(src: &str, span: Span) -> serde_json::Value {
    let (sl, sc) = byte_to_position(src, span.start.0);
    let (el, mut ec) = byte_to_position(src, span.end.0);
    if el == sl && ec <= sc {
        ec = sc + 1;
    }
    serde_json::json!({
        "start": { "line": sl, "character": sc },
        "end": { "line": el, "character": ec }
    })
}

/// Token length in the negotiated encoding (semanticTokens).
pub(super) fn token_length(src: &str, start: usize, end: usize) -> u32 {
    let end = end.min(src.len());
    let start = start.min(end);
    // Slice must be on char boundaries for UTF-8 validity.
    let mut s = start;
    let mut e = end;
    while s < end && !src.is_char_boundary(s) {
        s += 1;
    }
    while e > s && !src.is_char_boundary(e) {
        e -= 1;
    }
    measure_str(&src[s..e], position_encoding())
}

pub(super) fn ident_at(src: &str, byte: u32) -> Option<String> {
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

/// Incomplete identifier prefix ending at `byte` (for completion filtering).
/// Unlike [`ident_at`], does not extend past the cursor.
pub(super) fn prefix_at(src: &str, byte: u32) -> String {
    let bytes = src.as_bytes();
    let end = (byte as usize).min(bytes.len());
    let mut i = end;
    while i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
        i -= 1;
    }
    std::str::from_utf8(&bytes[i..end])
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{ident_at, pos_to_byte, prefix_at};
    use crate::lsp::state::{state_lock, State};
    use lumia_syntax::ColumnMetric;
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
    fn pos_to_byte_maps_line_and_character_ascii() {
        with_encoding(ColumnMetric::Utf16, || {
            let src = "foo\nbar baz\n";
            assert_eq!(pos_to_byte(src, 0, 0), 0);
            assert_eq!(pos_to_byte(src, 0, 2), 2);
            assert_eq!(pos_to_byte(src, 1, 0), 4);
            assert_eq!(pos_to_byte(src, 1, 4), 8);
        });
    }

    #[test]
    fn pos_to_byte_utf16_skips_cjk_bytes() {
        with_encoding(ColumnMetric::Utf16, || {
            let src = "val 你好 = x\n";
            let x_byte = src.find('x').expect("x") as u32;
            // UTF-16: v a l _ 你 好 _ = _ x → character 9
            assert_eq!(pos_to_byte(src, 0, 9), x_byte);
            // Byte-as-character would land inside 好 (wrong).
            assert_ne!(pos_to_byte(src, 0, x_byte), x_byte);
        });
    }

    #[test]
    fn ident_at_finds_identifier_at_cursor() {
        let src = "val x = 1\nfun foo() = x\n";
        assert_eq!(ident_at(src, 4), Some("x".to_string()));
        assert_eq!(ident_at(src, 14), Some("foo".to_string()));
        assert_eq!(ident_at(src, 22), Some("x".to_string()));
        assert_eq!(ident_at(src, 6), None); // on '='
        assert_eq!(ident_at(src, 0), Some("val".to_string()));
    }

    #[test]
    fn prefix_at_stops_at_cursor() {
        let src = "val print";
        assert_eq!(prefix_at(src, src.len() as u32), "print");
        assert_eq!(prefix_at(src, 5), "p"); // after "val p"
        assert_eq!(prefix_at(src, 4), ""); // after "val "
    }
}
