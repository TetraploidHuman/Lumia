//! Located diagnostics: path:line:col + source snippet.

use crate::span::{BytePos, Span};
use std::fmt::Write as _;

/// How to count columns within a source line.
///
/// - [`Self::Utf8`]: UTF-8 bytes (LSP `utf-8` position encoding).
/// - [`Self::Utf16`]: UTF-16 code units (LSP default `utf-16`).
/// - [`Self::Scalar`]: Unicode scalars / code points (CLI diagnostics).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColumnMetric {
    Utf8,
    #[default]
    Utf16,
    Scalar,
}

/// 0-based byte offsets of the start of each line (line 0 at 0).
pub fn line_starts(src: &str) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            let next = (i + 1) as u32;
            if next <= src.len() as u32 {
                starts.push(next);
            }
        }
    }
    starts
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Count of `metric` units in `s` (whole string).
pub fn measure_str(s: &str, metric: ColumnMetric) -> u32 {
    match metric {
        ColumnMetric::Utf8 => s.len() as u32,
        ColumnMetric::Utf16 => s.encode_utf16().count() as u32,
        ColumnMetric::Scalar => s.chars().count() as u32,
    }
}

/// `metric` column (0-based) of a byte offset within `line` (no trailing newline).
pub fn metric_col_at_byte(line: &str, byte_in_line: usize, metric: ColumnMetric) -> u32 {
    let end = floor_char_boundary(line, byte_in_line.min(line.len()));
    measure_str(&line[..end], metric)
}

/// Byte offset within `line` for a 0-based `metric` column (clamped to line end).
pub fn byte_at_metric_col(line: &str, col: u32, metric: ColumnMetric) -> usize {
    if col == 0 {
        return 0;
    }
    match metric {
        ColumnMetric::Utf8 => (col as usize).min(line.len()),
        ColumnMetric::Utf16 => {
            let mut units = 0u32;
            for (i, ch) in line.char_indices() {
                if units >= col {
                    return i;
                }
                units += ch.len_utf16() as u32;
            }
            line.len()
        }
        ColumnMetric::Scalar => {
            for (n, (i, _)) in line.char_indices().enumerate() {
                if (n as u32) >= col {
                    return i;
                }
            }
            line.len()
        }
    }
}

fn line_index_and_start(starts: &[u32], pos: BytePos) -> (usize, u32) {
    let p = pos.0;
    let idx = match starts.binary_search(&p) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    };
    let line_start = starts.get(idx).copied().unwrap_or(0);
    (idx, line_start)
}

/// 1-based line and column for a byte position (column in `metric` units).
pub fn byte_to_line_col_metric(
    src: &str,
    starts: &[u32],
    pos: BytePos,
    metric: ColumnMetric,
) -> (u32, u32) {
    let (idx, line_start) = line_index_and_start(starts, pos);
    let line = (idx as u32) + 1;
    let line_begin = line_start as usize;
    let line_end = starts
        .get(idx + 1)
        .map(|s| *s as usize)
        .unwrap_or(src.len());
    let mut line_text = &src[line_begin.min(src.len())..line_end.min(src.len())];
    if let Some(stripped) = line_text.strip_suffix('\n') {
        line_text = stripped;
    }
    if let Some(stripped) = line_text.strip_suffix('\r') {
        line_text = stripped;
    }
    let byte_in_line = pos.0.saturating_sub(line_start) as usize;
    let col = metric_col_at_byte(line_text, byte_in_line, metric) + 1;
    (line, col)
}

/// 1-based line and **UTF-8 byte** column (legacy / internal).
pub fn byte_to_line_col(starts: &[u32], pos: BytePos) -> (u32, u32) {
    let (idx, line_start) = line_index_and_start(starts, pos);
    let line = (idx as u32) + 1;
    let col = pos.0.saturating_sub(line_start) + 1;
    (line, col)
}

/// Map 0-based `(line, character)` in `metric` units to a source byte offset.
pub fn pos_to_byte_metric(
    src: &str,
    line: u32,
    character: u32,
    metric: ColumnMetric,
) -> u32 {
    let starts = line_starts(src);
    let idx = line as usize;
    let start = starts.get(idx).copied().unwrap_or(0) as usize;
    let end = starts
        .get(idx + 1)
        .map(|s| *s as usize)
        .unwrap_or(src.len());
    let mut line_text = &src[start.min(src.len())..end.min(src.len())];
    if let Some(stripped) = line_text.strip_suffix('\n') {
        line_text = stripped;
    }
    if let Some(stripped) = line_text.strip_suffix('\r') {
        line_text = stripped;
    }
    let off = byte_at_metric_col(line_text, character, metric);
    (start + off) as u32
}

/// Format `path:line:col: kind: message` plus the source line and a caret underline.
///
/// Columns and caret offsets use **Unicode scalar** counts (DESIGN §3.3 / user-facing).
///
/// `path` / `src` should match [`Span::file`] after [`crate::stamp_module`] (or an
/// equivalent `with_file`). Prefer [`format_diagnostic_files`] when a file table
/// is available so the span's file id is the path source of truth.
pub fn format_diagnostic(path: &str, src: &str, span: Span, kind: &str, message: &str) -> String {
    let starts = line_starts(src);
    let (line, col) = byte_to_line_col_metric(src, &starts, span.start, ColumnMetric::Scalar);
    let mut out = String::new();
    let _ = writeln!(out, "{path}:{line}:{col}: {kind}: {message}");

    let line_idx = (line as usize).saturating_sub(1);
    let line_start = starts.get(line_idx).copied().unwrap_or(0) as usize;
    let line_end = starts
        .get(line_idx + 1)
        .map(|s| *s as usize)
        .unwrap_or(src.len());
    let mut line_text = &src[line_start.min(src.len())..line_end.min(src.len())];
    if let Some(stripped) = line_text.strip_suffix('\n') {
        line_text = stripped;
    }
    if let Some(stripped) = line_text.strip_suffix('\r') {
        line_text = stripped;
    }
    let _ = writeln!(out, "  {line_text}");

    let start_byte = span.start.0.saturating_sub(line_start as u32) as usize;
    let end_byte = span.end.0.saturating_sub(line_start as u32) as usize;
    let caret_start = metric_col_at_byte(line_text, start_byte, ColumnMetric::Scalar) as usize;
    let mut caret_end = metric_col_at_byte(line_text, end_byte, ColumnMetric::Scalar) as usize;
    if caret_end <= caret_start {
        caret_end = caret_start + 1;
    }
    let line_scalars = measure_str(line_text, ColumnMetric::Scalar) as usize;
    caret_end = caret_end.min(line_scalars.saturating_add(1).max(caret_start + 1));
    let mut mark = String::new();
    mark.push_str("  ");
    for _ in 0..caret_start.min(line_scalars) {
        mark.push(' ');
    }
    let width = (caret_end - caret_start).max(1);
    for _ in 0..width {
        mark.push('^');
    }
    let _ = write!(out, "{mark}");
    out
}

/// Multi-file diagnostic: path/src come from `files[span.file]` (stamp contract).
///
/// Out-of-range `span.file` falls back to file 0 when present, else `"<unknown>"`.
pub fn format_diagnostic_files(
    files: &[(&str, &str)],
    span: Span,
    kind: &str,
    message: &str,
) -> String {
    let (path, src) = files
        .get(span.file as usize)
        .or_else(|| files.first())
        .copied()
        .unwrap_or(("<unknown>", ""));
    format_diagnostic(path, src, span, kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_basic() {
        let src = "ab\ncde\nf";
        let starts = line_starts(src);
        assert_eq!(byte_to_line_col(&starts, BytePos(0)), (1, 1));
        assert_eq!(byte_to_line_col(&starts, BytePos(3)), (2, 1));
        assert_eq!(byte_to_line_col(&starts, BytePos(5)), (2, 3));
    }

    #[test]
    fn line_col_byte_helper_stays_byte_based() {
        // Legacy helper: 1-based *byte* offsets. `你`/`好` are 3 bytes each.
        let src = "val 你好 = x\n";
        let starts = line_starts(src);
        let x_byte = src.find('x').expect("x") as u32;
        let (line, col) = byte_to_line_col(&starts, BytePos(x_byte));
        assert_eq!(line, 1);
        assert_eq!(col, x_byte + 1);
        assert!(col > 10, "multibyte chars must push the byte column past ASCII width");
    }

    #[test]
    fn format_diagnostic_uses_scalar_columns() {
        // `val 你好 = x` → scalars: v a l _ 你 好 _ = _ x → x at column 10.
        let src = "val 你好 = x\n";
        let starts = line_starts(src);
        let x_byte = src.find('x').expect("x") as u32;
        let (line, col) =
            byte_to_line_col_metric(src, &starts, BytePos(x_byte), ColumnMetric::Scalar);
        assert_eq!((line, col), (1, 10));
        let (_, utf16) =
            byte_to_line_col_metric(src, &starts, BytePos(x_byte), ColumnMetric::Utf16);
        assert_eq!(utf16, 10, "BMP CJK: utf-16 units == scalars");
        let s = format_diagnostic(
            "中.lm",
            src,
            Span::new(x_byte, x_byte + 1),
            "type",
            "unbound",
        );
        assert!(s.starts_with("中.lm:1:10:"), "{s}");
        // Caret pads by scalars so it sits under `x`, not mid-CJK bytes.
        let caret_line = s.lines().nth(2).expect("caret line");
        assert!(
            caret_line.trim_start().starts_with('^'),
            "caret should mark x: {s}"
        );
        assert_eq!(
            caret_line.chars().filter(|c| *c == ' ').count(),
            2 + 9, // indent + scalars before x
            "{s}"
        );
    }

    #[test]
    fn utf16_roundtrip_surrogate_pair() {
        // U+1F600 😀 is one scalar, two UTF-16 units, four UTF-8 bytes.
        let src = "a😀b";
        let starts = line_starts(src);
        let b_byte = src.find('b').expect("b") as u32;
        let (_, utf16) =
            byte_to_line_col_metric(src, &starts, BytePos(b_byte), ColumnMetric::Utf16);
        let (_, scalar) =
            byte_to_line_col_metric(src, &starts, BytePos(b_byte), ColumnMetric::Scalar);
        assert_eq!(utf16, 4); // 1-based: a, hi, lo, b
        assert_eq!(scalar, 3); // 1-based: a, emoji, b
        assert_eq!(
            pos_to_byte_metric(src, 0, utf16 - 1, ColumnMetric::Utf16),
            b_byte
        );
        assert_eq!(
            pos_to_byte_metric(src, 0, scalar - 1, ColumnMetric::Scalar),
            b_byte
        );
    }

    #[test]
    fn format_includes_path_and_caret() {
        let src = "val x = 1\nval y = z\n";
        // point at `z`
        let span = Span::new(16, 17);
        let s = format_diagnostic("t.lm", src, span, "type", "unbound variable `z`");
        assert!(s.contains("t.lm:2:7:"), "{s}");
        assert!(s.contains("unbound variable `z`"), "{s}");
        assert!(s.contains('^'), "{s}");
    }

    #[test]
    fn format_diagnostic_files_uses_span_file() {
        let a = "val x = 1\n";
        let b = "val y = z\n";
        let span = Span::new(8, 9).with_file(1); // `z` in file 1
        let s = format_diagnostic_files(
            &[("a.lm", a), ("b.lm", b)],
            span,
            "type",
            "unbound variable `z`",
        );
        assert!(s.starts_with("b.lm:1:9:"), "{s}");
        assert!(s.contains("unbound variable `z`"), "{s}");
    }
}
