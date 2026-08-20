//! textDocument/hover.

use super::cursor::{ident_at, pos_to_byte};
use super::state::{state_lock, Analysis};
use anyhow::Result;
use lumia_syntax::Span;
use lumia_ty::{display_type, Type};
use serde_json::{json, Value};

fn scheme_num_vars(a: &Analysis, name: &str) -> Vec<u32> {
    a.typed
        .fun_schemes
        .get(name)
        .map(|s| s.num_vars.clone())
        .unwrap_or_default()
}

fn hover_markdown(ty: &Type, num_vars: &[u32]) -> Value {
    markdown_code(&display_type(ty, num_vars))
}

fn hover_binding_markdown(name: &str, ty: &Type, num_vars: &[u32]) -> Value {
    markdown_code(&format!("{name}: {}", display_type(ty, num_vars)))
}

fn markdown_code(text: &str) -> Value {
    json!({
        "contents": {
            "kind": "markdown",
            "value": format!("```lumia\n{text}\n```")
        }
    })
}

pub(crate) fn hover_for_analysis(a: &Analysis, line: u32, character: u32) -> Value {
    let byte = pos_to_byte(&a.src, line, character);
    // Prefer tightest spanning type_at entry.
    let mut best: Option<&(Span, Type)> = None;
    for entry in &a.typed.type_at {
        let (sp, _) = entry;
        if sp.file == a.buffer_file && sp.start.0 <= byte && byte < sp.end.0.max(sp.start.0 + 1) {
            match best {
                None => best = Some(entry),
                Some((bsp, _)) => {
                    let bw = bsp.end.0.saturating_sub(bsp.start.0);
                    let w = sp.end.0.saturating_sub(sp.start.0);
                    if w < bw {
                        best = Some(entry);
                    }
                }
            }
        }
    }
    if let Some((_, ty)) = best {
        // Ground Num vars when hovering a known top-level binding name under the cursor.
        let num_vars = ident_at(&a.src, byte)
            .map(|n| scheme_num_vars(a, &n))
            .unwrap_or_default();
        return hover_markdown(ty, &num_vars);
    }
    if let Some(name) = ident_at(&a.src, byte) {
        if let Some(ty) = a.typed.fun_types.get(&name) {
            let num_vars = scheme_num_vars(a, &name);
            return hover_binding_markdown(&name, ty, &num_vars);
        }
    }
    Value::Null
}

pub(super) fn on_hover(params: Option<&Value>) -> Result<Value> {
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
    Ok(hover_for_analysis(a, line, character))
}

#[cfg(test)]
mod tests {
    use super::hover_for_analysis;
    use crate::check::check_source;
    use crate::lsp::cursor::byte_to_position;
    use crate::lsp::state::Analysis;
    use crate::lsp::test_support::{imported_alias_analysis, with_encoding, IMPORTED_ALIAS_SRC};
    use lumia_syntax::ColumnMetric;

    #[test]
    fn hover_matches_inlay_num_defaulting() {
        with_encoding(ColumnMetric::Utf16, || {
            let src = r#"
module Demo
val add = { x, y -> x + y }
"#;
            let typed = check_source(src, true).expect("typecheck");
            let a = Analysis::from_typed(typed, src.to_string(), vec![], 0);
            let byte = src.find("add").expect("add") as u32;
            let (line, character) = byte_to_position(src, byte);
            let hover = hover_for_analysis(&a, line, character);
            let md = hover["contents"]["value"].as_str().unwrap_or("");
            assert!(
                md.contains("(Int, Int) -> Int"),
                "hover should ground Num like inlay, got {md:?}"
            );
            assert!(
                !md.contains('?'),
                "hover must not show raw ?N vars, got {md:?}"
            );
        });
    }

    #[test]
    fn hover_imported_alias_via_loader() {
        with_encoding(ColumnMetric::Utf16, || {
            let src = IMPORTED_ALIAS_SRC;
            let a = imported_alias_analysis("untitled:Hover-1");
            let byte = src.find("log(1)").expect("log call") as u32;
            let (line, character) = byte_to_position(src, byte);
            let hover = hover_for_analysis(&a, line, character);
            assert!(!hover.is_null(), "hover on imported `log` must not be null");
            let md = hover["contents"]["value"].as_str().unwrap_or("");
            assert!(
                !md.is_empty(),
                "hover markdown for imported alias must be non-empty, got {hover:?}"
            );
        });
    }
}
