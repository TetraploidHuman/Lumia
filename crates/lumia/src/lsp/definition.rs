//! textDocument/definition.

use super::cursor::{ident_at, pos_to_byte, span_to_range};
use super::state::{state_lock, Analysis};
use super::uri::path_to_uri;
use anyhow::Result;
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
    let target_uri = if file.path.as_os_str().is_empty() {
        uri.to_string()
    } else {
        path_to_uri(&file.path)
    };
    json!({
        "uri": target_uri,
        "range": span_to_range(&file.src, *span)
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
    use crate::lsp::cursor::byte_to_position;
    use crate::lsp::state::Analysis;
    use crate::lsp::test_support::{imported_alias_analysis, with_encoding, IMPORTED_ALIAS_SRC};
    use lumia_syntax::ColumnMetric;
    use std::path::PathBuf;

    fn analysis(src: &str) -> Analysis {
        let typed = check_source(src, true).expect("typecheck");
        Analysis::from_typed(
            typed,
            src.to_string(),
            vec![SourceFile {
                path: PathBuf::new(),
                src: src.to_string(),
            }],
            0,
        )
    }

    #[test]
    fn definition_jumps_to_binding() {
        with_encoding(ColumnMetric::Utf16, || {
            let src = r#"
module Demo
val add = { x, y -> x + y }
val main = {
    add(1, 2)
}
"#;
            let a = analysis(src);
            let use_byte = src.rfind("add").expect("use site") as u32;
            let (line, col) = byte_to_position(src, use_byte);
            let loc = definition_for_analysis(&a, "file:///demo.lm", line, col);
            assert!(!loc.is_null(), "expected Location for add use-site");
            let def_byte = src.find("val add").expect("def") as u32;
            let (def_line, _) = byte_to_position(src, def_byte);
            assert_eq!(
                loc["range"]["start"]["line"].as_u64().unwrap(),
                def_line as u64,
                "definition should point at `val add`, got {loc}"
            );
        });
    }

    #[test]
    fn definition_unknown_ident_is_null() {
        with_encoding(ColumnMetric::Utf16, || {
            let src = "module Demo\nval main = { 1 }\n";
            let a = analysis(src);
            let loc = definition_for_analysis(&a, "file:///demo.lm", 1, 0);
            assert!(loc.is_null());
        });
    }

    #[test]
    fn definition_imported_alias_via_loader() {
        with_encoding(ColumnMetric::Utf16, || {
            let src = IMPORTED_ALIAS_SRC;
            let uri = "untitled:Def-1";
            let a = imported_alias_analysis(uri);
            let use_byte = src.find("log(1)").expect("use") as u32;
            let (line, col) = byte_to_position(src, use_byte);
            let loc = definition_for_analysis(&a, uri, line, col);
            assert!(
                !loc.is_null(),
                "definition on imported alias must resolve via loader, got {loc}"
            );
        });
    }
}
