//! Keyword / free-builtin span overlays (shared with TextMate surface names).

use super::token::{push, AbsToken, MOD_DEFAULT_LIB, TY_FUNCTION, TY_KEYWORD};

/// Surface keywords shared with TextMate (also painted by semantic overlay).
pub(super) const KEYWORDS: &[&str] = &[
    "if", "else", "match", "for", "in", "break", "continue", "return", "alt", "module", "import",
    "val", "var", "type", "trait", "instance", "requires", "with", "effect", "foreign", "priv",
    "as", "pure", "fn", "and", "or", "not", "true", "false",
];

pub(super) fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

pub(super) fn find_word(src: &str, word: &str, from: usize, to: usize) -> Option<(usize, usize)> {
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
        let end = abs + word.len();
        let before_ok = abs == 0 || !is_ident_byte(bytes[abs - 1]);
        let after_ok = end >= bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            return Some((abs, end));
        }
        search += rel + 1;
    }
    None
}

fn in_string_or_comment(src: &str, offset: usize) -> bool {
    let offset = offset.min(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    let mut in_line = false;
    let mut in_block = false;
    let mut in_string = false;
    while i < offset {
        let c = bytes[i];
        if in_line {
            if c == b'\n' {
                in_line = false;
            }
            i += 1;
            continue;
        }
        if in_block {
            if c == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                in_block = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if in_string {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'/' => {
                    in_line = true;
                    i += 2;
                    continue;
                }
                b'*' => {
                    in_block = true;
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        if c == b'"' {
            in_string = true;
        }
        i += 1;
    }
    in_line || in_block || in_string
}

pub(super) fn push_keyword_spans(src: &str, kw: &str, out: &mut Vec<AbsToken>) {
    let mut from = 0usize;
    while let Some((s, e)) = find_word(src, kw, from, src.len()) {
        if !in_string_or_comment(src, s) {
            push(out, s, e, TY_KEYWORD, 0);
        }
        from = e;
    }
}

pub(super) fn push_free_builtin_spans(src: &str, name: &str, out: &mut Vec<AbsToken>) {
    let mut from = 0usize;
    while let Some((s, e)) = find_word(src, name, from, src.len()) {
        if !in_string_or_comment(src, s) {
            push(out, s, e, TY_FUNCTION, MOD_DEFAULT_LIB);
        }
        from = e;
    }
}
