//! textDocument/semanticTokens/full — type-aware highlighting (Lumi-style).

mod overlay;
mod token;
mod walk;

pub use token::{TOKEN_MODIFIERS, TOKEN_TYPES};

use super::state::{state_lock, Analysis};
use anyhow::Result;
use lumia_hir::{surface_names, SurfaceRole};
use overlay::{highlight_keywords, push_free_builtin_spans, push_keyword_spans};
use serde_json::{json, Value};
use token::encode_deltas;
use walk::collect_module;

pub(super) fn tokens_for_analysis(a: &Analysis) -> Vec<u32> {
    let mut abs = Vec::new();
    // Use cached surface parse from analysis (avoid a second parse_module_*).
    collect_module(a, &a.surface.module, &a.src, &mut abs);
    for kw in highlight_keywords() {
        push_keyword_spans(&a.src, kw, &mut abs);
    }
    for sn in surface_names() {
        if sn.role == SurfaceRole::Free {
            push_free_builtin_spans(&a.src, sn.name, &mut abs);
        }
    }
    abs.sort_by_key(|t| (t.start, t.end));
    abs.dedup_by(|a, b| a.start == b.start && a.end == b.end);
    encode_deltas(&a.src, &abs)
}

pub(super) fn on_semantic_tokens(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(json!({ "data": [] }));
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(json!({ "data": [] }));
    };
    let Some(a) = state.analysis.get(uri) else {
        return Ok(json!({ "data": [] }));
    };
    Ok(json!({ "data": tokens_for_analysis(a) }))
}

#[cfg(test)]
mod tests {
    use super::tokens_for_analysis;
    use crate::check::check_source;
    use crate::lsp::state::Analysis;

    #[test]
    fn semantic_tokens_nonempty_for_simple_program() {
        let src = r#"
module Demo
import std.io.{println}
type Color {
    Red,
    Green
}
val main = {
    val xs = listOf(1, 2)
    println(xs.len())
    Red
}
"#;
        let typed = check_source(src, true).expect("typecheck");
        let a = Analysis::from_typed(typed, src.to_string(), vec![], 0);
        let data = tokens_for_analysis(&a);
        assert!(
            !data.is_empty() && data.len().is_multiple_of(5),
            "expected encoded token quintuples, got len={}",
            data.len()
        );
        // At least one keyword / function / method / enum member / type (module+import path).
        let types: Vec<u32> = data.chunks(5).map(|c| c[3]).collect();
        assert!(types.contains(&0), "function token missing: {types:?}");
        assert!(types.contains(&1), "method token missing: {types:?}");
        assert!(types.contains(&6), "enumMember missing: {types:?}");
        assert!(types.contains(&9), "keyword missing: {types:?}");
        assert!(
            types.contains(&8),
            "type token (module/import) missing: {types:?}"
        );
    }

    #[test]
    fn semantic_tokens_paint_import_alias() {
        // `check_source` does not load packages, so aliased imports are not
        // bound as callables — only paint the import AST (path + local name).
        let src = r#"
module Demo
import std.io.{println as log}
val main = { 1 }
"#;
        let typed = check_source(src, true).expect("typecheck");
        let a = Analysis::from_typed(typed, src.to_string(), vec![], 0);
        let data = tokens_for_analysis(&a);
        assert!(!data.is_empty() && data.len().is_multiple_of(5));
        let types: Vec<u32> = data.chunks(5).map(|c| c[3]).collect();
        assert!(
            types.contains(&8),
            "import path type tokens missing: {types:?}"
        );
        assert!(
            types.contains(&0),
            "imported local name should paint as function: {types:?}"
        );
    }

    #[test]
    fn semantic_imported_alias_call_via_loader() {
        // With loader, `log` is a typed callable; call-site paint must see fun_types.
        use crate::lsp::test_support::imported_alias_analysis;
        let a = imported_alias_analysis("untitled:Semantic-1");
        assert!(
            a.typed.fun_types.contains_key("log"),
            "imported alias must bind under loader"
        );
        let data = tokens_for_analysis(&a);
        assert!(!data.is_empty() && data.len().is_multiple_of(5));
        let types: Vec<u32> = data.chunks(5).map(|c| c[3]).collect();
        assert!(
            types.contains(&0),
            "typed import alias / call should paint as function: {types:?}"
        );
    }
}
