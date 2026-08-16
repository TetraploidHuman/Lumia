//! Cursor position ↔ source byte offset helpers.

use lumia_syntax::line_starts;

pub(super) fn pos_to_byte(src: &str, line: u32, character: u32) -> u32 {
    let starts = line_starts(src);
    let idx = line as usize;
    let start = starts.get(idx).copied().unwrap_or(0);
    start.saturating_add(character)
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

    #[test]
    fn pos_to_byte_maps_line_and_character() {
        let src = "foo\nbar baz\n";
        assert_eq!(pos_to_byte(src, 0, 0), 0);
        assert_eq!(pos_to_byte(src, 0, 2), 2);
        assert_eq!(pos_to_byte(src, 1, 0), 4);
        assert_eq!(pos_to_byte(src, 1, 4), 8);
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
