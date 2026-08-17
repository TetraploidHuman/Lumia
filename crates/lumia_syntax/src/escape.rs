//! Shared string/char escape tables for lexer unescape and pretty escape.
//!
//! Keep lexer decode and `pretty::escape_str` in lockstep via these helpers.

/// Decode one string-literal escape byte (`n`/`t`/`r`/`\`/`"`/`$`).
/// Unknown escapes yield `None` (caller may keep the raw byte as a char).
#[inline]
pub(crate) fn unescape_string_byte(esc: u8) -> Option<char> {
    match esc {
        b'n' => Some('\n'),
        b't' => Some('\t'),
        b'r' => Some('\r'),
        b'\\' => Some('\\'),
        b'"' => Some('"'),
        b'$' => Some('$'),
        _ => None,
    }
}

/// Decode one character-literal escape byte (string set plus `'` / `0`).
#[inline]
pub(crate) fn unescape_char_byte(esc: u8) -> Option<char> {
    match esc {
        b'\'' => Some('\''),
        b'0' => Some('\0'),
        other => unescape_string_byte(other),
    }
}

/// Append the Lumia source escape for `c` into `out` (string / interp lit).
#[inline]
pub(crate) fn escape_char_into(out: &mut String, c: char) {
    match c {
        '\\' => out.push_str("\\\\"),
        '"' => out.push_str("\\\""),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '$' => out.push_str("\\$"),
        c => out.push(c),
    }
}

/// Escape a whole string for pretty-printing string literals.
pub(crate) fn escape_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        escape_char_into(&mut o, c);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_unescape_roundtrip_common() {
        assert_eq!(unescape_string_byte(b'n'), Some('\n'));
        assert_eq!(unescape_string_byte(b't'), Some('\t'));
        assert_eq!(unescape_string_byte(b'r'), Some('\r'));
        assert_eq!(unescape_string_byte(b'\\'), Some('\\'));
        assert_eq!(unescape_string_byte(b'"'), Some('"'));
        assert_eq!(unescape_string_byte(b'$'), Some('$'));
        assert_eq!(unescape_string_byte(b'x'), None);
    }

    #[test]
    fn escape_str_covers_table() {
        assert_eq!(escape_str("a\\b\"c\nd\re\tf$g"), "a\\\\b\\\"c\\nd\\re\\tf\\$g");
    }
}
