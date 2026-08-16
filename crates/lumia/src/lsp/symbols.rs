//! textDocument/documentSymbol — outline from typed module items.

use super::state::{state_lock, Analysis};
use anyhow::Result;
use lumia_hir::Item;
use lumia_syntax::{byte_to_line_col, line_starts, BytePos, Span};
use serde_json::{json, Value};

/// LSP SymbolKind constants used by outline views.
mod kind {
    pub const MODULE: i32 = 2;
    pub const FUNCTION: i32 = 12;
    pub const VARIABLE: i32 = 13;
    pub const ENUM: i32 = 10;
    pub const STRUCT: i32 = 23;
    pub const CLASS: i32 = 5;
    pub const INTERFACE: i32 = 11;
}

fn range_json(src: &str, span: Span) -> Value {
    let starts = line_starts(src);
    let (sl, sc) = byte_to_line_col(&starts, span.start);
    let (el, ec) = byte_to_line_col(&starts, span.end);
    json!({
        "start": { "line": sl.saturating_sub(1), "character": sc.saturating_sub(1) },
        "end": { "line": el.saturating_sub(1), "character": ec.saturating_sub(1) }
    })
}

fn symbol(name: &str, kind: i32, src: &str, span: Span) -> Value {
    let range = range_json(src, span);
    json!({
        "name": name,
        "kind": kind,
        "range": range,
        "selectionRange": range
    })
}

fn span_for_name(a: &Analysis, name: &str) -> Span {
    if let Some(span) = a.typed.decls.get(name) {
        return *span;
    }
    // Approximate: first occurrence of the identifier in the primary buffer.
    if let Some(pos) = a.src.find(name) {
        let start = pos as u32;
        return Span {
            file: primary_file_id(a),
            start: BytePos(start),
            end: BytePos(start + name.len() as u32),
        };
    }
    Span {
        file: primary_file_id(a),
        start: BytePos(0),
        end: BytePos(0),
    }
}

fn primary_file_id(a: &Analysis) -> u32 {
    a.buffer_file
}

fn span_in_primary(a: &Analysis, span: Span) -> bool {
    span.file == primary_file_id(a)
}

pub(super) fn symbols_for_analysis(a: &Analysis) -> Vec<Value> {
    let m = &a.typed.module;
    let src = &a.src;
    let mut out = Vec::new();
    out.push(symbol(
        &m.name,
        kind::MODULE,
        src,
        span_for_name(a, &m.name),
    ));
    for item in &m.items {
        match item {
            Item::Fun(f) => {
                if !span_in_primary(a, f.span) {
                    continue;
                }
                out.push(symbol(
                    &f.name,
                    kind::FUNCTION,
                    src,
                    span_for_name(a, &f.name),
                ));
            }
            Item::Val { name, span, .. } => {
                if !span_in_primary(a, *span) {
                    continue;
                }
                out.push(symbol(name, kind::VARIABLE, src, span_for_name(a, name)));
            }
        }
    }
    for p in &m.products {
        out.push(symbol(
            &p.name,
            kind::STRUCT,
            src,
            span_for_name(a, &p.name),
        ));
    }
    for adt in &m.adts {
        out.push(symbol(
            &adt.name,
            kind::ENUM,
            src,
            span_for_name(a, &adt.name),
        ));
    }
    for trait_name in m.method_traits.values() {
        // method_traits maps method → trait; emit each trait once.
        if out.iter().any(|s| s["name"] == *trait_name) {
            continue;
        }
        out.push(symbol(
            trait_name,
            kind::INTERFACE,
            src,
            span_for_name(a, trait_name),
        ));
    }
    let mut instances: Vec<_> = m.instances.iter().collect();
    instances.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    for (tr, ty) in instances {
        let name = format!("{tr} for {ty}");
        out.push(symbol(&name, kind::CLASS, src, span_for_name(a, tr)));
    }
    out
}

pub(super) fn on_document_symbol(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(json!([]));
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(json!([]));
    };
    let Some(a) = state.analysis.get(uri) else {
        return Ok(json!([]));
    };
    Ok(Value::Array(symbols_for_analysis(a)))
}

#[cfg(test)]
mod tests {
    use super::super::state::Analysis;
    use super::symbols_for_analysis;
    use crate::check::check_source;

    #[test]
    fn document_symbols_include_module_and_vals() {
        let src = "module Demo\n\nval main = {\n    1\n}\n";
        let typed = check_source(src, true).expect("typecheck");
        let a = Analysis {
            typed,
            src: src.to_string(),
            files: vec![],
            buffer_file: 0,
        };
        let syms = symbols_for_analysis(&a);
        let names: Vec<&str> = syms.iter().filter_map(|s| s["name"].as_str()).collect();
        assert!(names.contains(&"Demo"));
        assert!(names.contains(&"main"));
    }
}
