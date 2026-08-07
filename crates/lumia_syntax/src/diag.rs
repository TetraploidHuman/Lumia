//! Located diagnostics: path:line:col + source snippet.

use crate::span::{BytePos, Span};
use std::fmt::Write as _;

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

/// 1-based line and column for a byte position.
pub fn byte_to_line_col(starts: &[u32], pos: BytePos) -> (u32, u32) {
    let p = pos.0;
    let idx = match starts.binary_search(&p) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    };
    let line_start = starts.get(idx).copied().unwrap_or(0);
    let line = (idx as u32) + 1;
    let col = p.saturating_sub(line_start) + 1;
    (line, col)
}

/// Format `path:line:col: kind: message` plus the source line and a caret underline.
pub fn format_diagnostic(path: &str, src: &str, span: Span, kind: &str, message: &str) -> String {
    let starts = line_starts(src);
    let (line, col) = byte_to_line_col(&starts, span.start);
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

    let caret_start = span.start.0.saturating_sub(line_start as u32) as usize;
    let mut caret_end = span.end.0.saturating_sub(line_start as u32) as usize;
    if caret_end <= caret_start {
        caret_end = caret_start + 1;
    }
    caret_end = caret_end.min(line_text.len().saturating_add(1).max(caret_start + 1));
    let mut mark = String::new();
    mark.push_str("  ");
    for _ in 0..caret_start.min(line_text.len()) {
        mark.push(' ');
    }
    let width = (caret_end - caret_start).max(1);
    for _ in 0..width {
        mark.push('^');
    }
    let _ = write!(out, "{mark}");
    out
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
    fn format_includes_path_and_caret() {
        let src = "val x = 1\nval y = z\n";
        // point at `z`
        let span = Span::new(16, 17);
        let s = format_diagnostic("t.lumia", src, span, "type", "unbound variable `z`");
        assert!(s.contains("t.lumia:2:7:"), "{s}");
        assert!(s.contains("unbound variable `z`"), "{s}");
        assert!(s.contains('^'), "{s}");
    }
}
