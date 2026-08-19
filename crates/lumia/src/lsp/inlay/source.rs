//! Source-text heuristics for placing inlay hints.

use lumia_syntax::line_starts;
use lumia_hir::Sym;
use serde_json::Value;

pub(super) fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// First whole-word match of `word` in `src[from..to]`; returns byte offset *after* the word.
pub(super) fn find_word_end(src: &str, word: &str, from: usize, to: usize) -> Option<usize> {
    if word.is_empty() || from >= src.len() {
        return None;
    }
    let to = to.min(src.len());
    if from >= to {
        return None;
    }
    let bytes = src.as_bytes();
    let region = &src[from..to];
    let mut search = 0usize;
    while let Some(rel) = region[search..].find(word) {
        let abs = from + search + rel;
        let before_ok = abs == 0 || !is_ident_byte(bytes[abs - 1]);
        let after = abs + word.len();
        let after_ok = after >= bytes.len() || !is_ident_byte(bytes[after]);
        if before_ok && after_ok {
            return Some(after);
        }
        search += rel + 1;
    }
    None
}

/// Last whole-word match of `word` ending at or before `before`.
pub(super) fn find_word_end_before(src: &str, word: &str, before: usize) -> Option<usize> {
    let before = before.min(src.len());
    let mut best = None;
    let mut from = 0usize;
    while let Some(end) = find_word_end(src, word, from, before) {
        best = Some(end);
        from = end;
        if from >= before {
            break;
        }
    }
    best
}

/// Parse `{ a, b ->` / `{ ->` / `{ a ->` inside `src[start..end]`.
/// Returns (param_name, byte_after_name) list.
pub(super) fn lambda_param_ends(src: &str, start: usize, end: usize) -> Vec<(String, usize)> {
    let end = end.min(src.len());
    if start >= end {
        return Vec::new();
    }
    let slice = &src[start..end];
    let Some(brace_rel) = slice.find('{') else {
        return Vec::new();
    };
    let after_brace = start + brace_rel + 1;
    let rest = &src[after_brace..end];
    let Some(arrow_rel) = rest.find("->") else {
        // Block fun `{ … }` with no params.
        return Vec::new();
    };
    let params_src = rest[..arrow_rel].trim();
    if params_src.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cursor = after_brace;
    for part in params_src.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            cursor += part.len() + 1; // + comma
            continue;
        }
        // Skip leading whitespace in this segment relative to params_src positioning:
        // re-find the ident in the original buffer near cursor.
        if let Some(end) = find_word_end(src, trimmed, cursor, end) {
            out.push((trimmed.to_string(), end));
            cursor = end;
        }
    }
    out
}

/// Ends of `params` names in order within `src[from..to]` (e.g. `(a, b)` or `{ a, b ->`).
pub(super) fn param_ends_in_window(
    src: &str,
    params: &[Sym],
    from: usize,
    to: usize,
) -> Vec<(String, usize)> {
    let mut cursor = from;
    let mut out = Vec::new();
    for p in params {
        if let Some(end) = find_word_end(src, p.as_str(), cursor, to) {
            out.push((p.to_string(), end));
            cursor = end;
        }
    }
    out
}

pub(super) fn in_range(byte: u32, range: Option<(u32, u32)>) -> bool {
    match range {
        None => true,
        Some((start, end)) => byte >= start && byte <= end,
    }
}

pub(super) fn range_from_params(src: &str, params: &Value) -> Option<(u32, u32)> {
    let range = params.get("range")?;
    let sl = range["start"]["line"].as_u64()? as u32;
    let sc = range["start"]["character"].as_u64()? as u32;
    let el = range["end"]["line"].as_u64()? as u32;
    let ec = range["end"]["character"].as_u64()? as u32;
    let starts = line_starts(src);
    // Clients (VS Code) often send an end line past EOF; never map that to byte 0
    // via `unwrap_or(0)`, or every hint is filtered out by `in_range`.
    let byte_at = |line: u32, col: u32| -> u32 {
        if starts.is_empty() {
            return 0;
        }
        let last = (starts.len() - 1) as u32;
        if line > last {
            return src.len() as u32;
        }
        let base = starts[line as usize];
        let line_end = if (line as usize + 1) < starts.len() {
            starts[line as usize + 1]
        } else {
            src.len() as u32
        };
        base.saturating_add(col).min(line_end)
    };
    let start = byte_at(sl, sc);
    let end = byte_at(el, ec);
    Some((start.min(end), start.max(end)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_word_respects_ident_boundaries() {
        let src = "val xx = x";
        assert_eq!(find_word_end(src, "x", 0, src.len()), Some(src.len()));
        assert_eq!(find_word_end_before(src, "x", src.len() - 1), None);
        assert_eq!(find_word_end_before(src, "xx", src.len()), Some(6));
    }

    #[test]
    fn lambda_param_ends_parses_arrow_header() {
        let src = "{ a, b -> a + b }";
        let ends = lambda_param_ends(src, 0, src.len());
        assert_eq!(ends.len(), 2);
        assert_eq!(ends[0].0, "a");
        assert_eq!(ends[1].0, "b");
    }
}
