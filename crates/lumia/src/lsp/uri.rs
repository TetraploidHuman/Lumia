//! file:// URI ↔ path conversion.

use std::path::{Path, PathBuf};

pub(super) fn uri_to_path(uri: &str) -> PathBuf {
    let rest = match uri.strip_prefix("file:") {
        Some(r) => r,
        None => return PathBuf::from(uri),
    };
    // Accept `file:///path`, `file://localhost/path`, and `file:/path`.
    let path_part = if let Some(after_slashes) = rest.strip_prefix("//") {
        if let Some(slash) = after_slashes.find('/') {
            let host = &after_slashes[..slash];
            if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
                &after_slashes[slash..]
            } else {
                // Non-local hosts are not supported; still take the path segment.
                &after_slashes[slash..]
            }
        } else {
            after_slashes
        }
    } else {
        rest
    };
    let decoded = percent_decode(path_part);
    // `file:///C:/Users/...` yields `/C:/Users/...`; strip the extra slash so
    // Windows APIs see a drive-letter path.
    let bytes = decoded.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return PathBuf::from(&decoded[1..]);
    }
    PathBuf::from(decoded)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub(super) fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    // RFC 8089: absolute paths use `file:///…`. Windows drive paths need a
    // leading slash (`file:///C:/…`); bare `file://C:/…` treats `C:` as host.
    // Absolute POSIX paths keep leading `/`; Windows `C:/…` and other relatives
    // get a leading slash so the URI is `file:///…` (RFC 8089).
    let path_str: std::borrow::Cow<'_, str> = if s.starts_with('/') {
        s
    } else {
        std::borrow::Cow::Owned(format!("/{s}"))
    };
    let mut enc = String::from("file://");
    for &b in path_str.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'_' | b'-' | b'.' | b'~' | b':' => {
                enc.push(b as char)
            }
            _ => enc.push_str(&format!("%{b:02X}")),
        }
    }
    enc
}

#[cfg(test)]
mod tests {
    use super::{path_to_uri, uri_to_path};
    use std::path::{Path, PathBuf};

    #[test]
    fn uri_to_path_decodes_and_strips_file_prefix() {
        let p = uri_to_path("file:///tmp/hello%20world.lm");
        assert_eq!(p, PathBuf::from("/tmp/hello world.lm"));
        let p = uri_to_path("file://localhost/tmp/x.lm");
        assert_eq!(p, PathBuf::from("/tmp/x.lm"));
        let p = uri_to_path("file:///C:/Users/me/x.lm");
        assert_eq!(p, PathBuf::from("C:/Users/me/x.lm"));
        assert_eq!(
            path_to_uri(Path::new("C:/Users/me/x.lm")),
            "file:///C:/Users/me/x.lm"
        );
    }
}
