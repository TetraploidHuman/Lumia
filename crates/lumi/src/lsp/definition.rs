//! textDocument/definition.

use super::cursor::{ident_at, pos_to_byte};
use super::state::{state_lock, Analysis};
use super::uri::path_to_uri;
use anyhow::Result;
use lumi_syntax::{byte_to_line_col, line_starts};
use serde_json::{json, Value};

pub(super) fn definition_for_analysis(a: &Analysis, uri: &str, line: u32, character: u32) -> Value {
    let byte = pos_to_byte(&a.src, line, character);
    let Some(name) = ident_at(&a.src, byte) else {
        return Value::Null;
    };
    let Some(span) = a.typed.decls.get(&name) else {
        return Value::Null;
    };
    let file = a
        .files
        .get(span.file as usize)
        .unwrap_or_else(|| a.files.first().expect("analysis files"));
    let starts = line_starts(&file.src);
    let (sl, sc) = byte_to_line_col(&starts, span.start);
    let (el, ec) = byte_to_line_col(&starts, span.end);
    let target_uri = if file.path.as_os_str().is_empty() {
        uri.to_string()
    } else {
        path_to_uri(&file.path)
    };
    json!({
        "uri": target_uri,
        "range": {
            "start": { "line": sl.saturating_sub(1), "character": sc.saturating_sub(1) },
            "end": { "line": el.saturating_sub(1), "character": ec.saturating_sub(1) }
        }
    })
}

pub(super) fn on_definition(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(Value::Null);
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let line = params["position"]["line"].as_u64().unwrap_or(0) as u32;
    let character = params["position"]["character"].as_u64().unwrap_or(0) as u32;
    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(Value::Null);
    };
    let Some(a) = state.analysis.get(uri) else {
        return Ok(Value::Null);
    };
    Ok(definition_for_analysis(a, uri, line, character))
}

#[cfg(test)]
mod tests {
    use super::definition_for_analysis;
    use crate::check::check_source;
    use crate::load::SourceFile;
    use crate::lsp::state::Analysis;
    use lumi_syntax::line_starts;
    use std::path::PathBuf;

    fn analysis(src: &str) -> Analysis {
        let typed = check_source(src, true).expect("typecheck");
        Analysis {
            typed,
            src: src.to_string(),
            files: vec![SourceFile {
                path: PathBuf::new(),
                src: src.to_string(),
            }],
        }
    }

    fn line_col_of(src: &str, needle: &str) -> (u32, u32) {
        let byte = src.find(needle).expect("needle") as u32;
        let starts = line_starts(src);
        let (line, col) = lumi_syntax::byte_to_line_col(&starts, lumi_syntax::BytePos(byte));
        (line.saturating_sub(1), col.saturating_sub(1))
    }

    #[test]
    fn definition_jumps_to_binding() {
        let src = r#"
module Demo
val add = { x, y -> x + y }
val main = {
    add(1, 2)
}
"#;
        let a = analysis(src);
        // Use site of `add` inside main.
        let use_byte = src.rfind("add").expect("use site");
        let starts = line_starts(src);
        let (line, col) =
            lumi_syntax::byte_to_line_col(&starts, lumi_syntax::BytePos(use_byte as u32));
        let loc = definition_for_analysis(
            &a,
            "file:///demo.lm",
            line.saturating_sub(1),
            col.saturating_sub(1),
        );
        assert!(!loc.is_null(), "expected Location for add use-site");
        let (def_line, _) = line_col_of(src, "val add");
        assert_eq!(
            loc["range"]["start"]["line"].as_u64().unwrap(),
            def_line as u64,
            "definition should point at `val add`, got {loc}"
        );
    }

    #[test]
    fn definition_unknown_ident_is_null() {
        let src = "module Demo\nval main = { 1 }\n";
        let a = analysis(src);
        let loc = definition_for_analysis(&a, "file:///demo.lm", 1, 0);
        assert!(loc.is_null());
    }
}
