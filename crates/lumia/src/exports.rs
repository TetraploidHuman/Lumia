//! Shared `/// @exports …` parsing for loader and `lumia doc`.

/// Split the text after the `@exports` keyword into export names.
pub fn parse_exports_payload(after_keyword: &str) -> Vec<String> {
    let list = after_keyword.trim().trim_start_matches(':').trim();
    if list.is_empty() {
        return vec![];
    }
    list.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn exports_keyword_rest(after: &str) -> bool {
    after.is_empty() || after.starts_with(char::is_whitespace) || after.starts_with(':')
}

/// First `/// @exports …` line in module source, if any.
///
/// Empty `@exports` yields `Some([])` — loader treats that as an error; doc may render it.
pub fn parse_exports_from_source(src: &str) -> Option<Vec<String>> {
    for line in src.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("///") else {
            continue;
        };
        let rest = rest.trim();
        let Some(after) = rest.strip_prefix("@exports") else {
            continue;
        };
        if !exports_keyword_rest(after) {
            continue;
        }
        return Some(parse_exports_payload(after));
    }
    None
}

/// Pull `@exports a, b` out of stripped module doc lines (no `///` prefix).
pub fn take_exports_from_doc_lines(docs: &mut Vec<String>) -> Option<Vec<String>> {
    let idx = docs.iter().position(|l| {
        l.trim_start()
            .strip_prefix("@exports")
            .is_some_and(exports_keyword_rest)
    })?;
    let line = docs.remove(idx);
    let after = line.trim_start().strip_prefix("@exports").unwrap_or("");
    Some(parse_exports_payload(after))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_and_doc_agree_on_list() {
        let src = "/// @exports foo, bar\nmodule M\n";
        assert_eq!(
            parse_exports_from_source(src),
            Some(vec!["foo".into(), "bar".into()])
        );
        let mut docs = vec!["@exports foo, bar".into()];
        assert_eq!(
            take_exports_from_doc_lines(&mut docs),
            Some(vec!["foo".into(), "bar".into()])
        );
        assert!(docs.is_empty());
    }

    #[test]
    fn empty_exports_is_some_empty() {
        assert_eq!(
            parse_exports_from_source("/// @exports\nmodule M\n"),
            Some(vec![])
        );
        let mut docs = vec!["@exports".into()];
        assert_eq!(take_exports_from_doc_lines(&mut docs), Some(vec![]));
    }

    #[test]
    fn missing_exports_is_none() {
        assert_eq!(parse_exports_from_source("module M\n"), None);
    }
}
