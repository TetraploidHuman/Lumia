//! `lumia doc` — Markdown from `///` comments + public API surface (DESIGN §13).

use anyhow::{Context, Result};
use lumia_syntax::{parse_module, Item, Module, TypeKind, VariantFields};
use std::fs;
use std::path::Path;

/// Render documentation for a `.lm` source file to Markdown.
pub fn render_file(path: &Path) -> Result<String> {
    let src = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let module = parse_module(&src).map_err(|e| {
        let starts = lumia_syntax::line_starts(&src);
        let (line, col) = lumia_syntax::byte_to_line_col(&starts, e.span.start);
        anyhow::anyhow!(
            "{}:{}:{}: parse: {}",
            path.display(),
            line,
            col,
            e.message
        )
    })?;
    Ok(render_module(&src, &module, path))
}

fn render_module(src: &str, module: &Module, path: &Path) -> String {
    let mut out = String::new();
    let title = if module.name.is_empty() {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("module")
            .to_string()
    } else {
        module.name.clone()
    };
    out.push_str(&format!("# Module `{title}`\n\n"));

    let mut mod_docs = preceding_doc_lines(src, module.span.start.0 as usize);
    let exports = crate::exports::take_exports_from_doc_lines(&mut mod_docs);
    if !mod_docs.is_empty() {
        out.push_str(&mod_docs.join("\n"));
        out.push_str("\n\n");
    }
    if let Some(ref ex) = exports {
        out.push_str("**Exports:** ");
        out.push_str(
            &ex.iter()
                .map(|e| format!("`{e}`"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str("\n\n");
    }

    if !module.imports.is_empty() {
        out.push_str("## Imports\n\n");
        for imp in &module.imports {
            let path_s = imp.path.join(".");
            out.push_str(&format!("- `import {path_s}`\n"));
        }
        out.push('\n');
    }

    // When `@exports` is present, document only that public surface (still honor `priv`).
    let exported = |name: &str| -> bool {
        match &exports {
            Some(ex) => ex.iter().any(|e| e == name),
            None => true,
        }
    };

    let mut types = Vec::new();
    let mut vals = Vec::new();
    let mut foreigns = Vec::new();
    for item in &module.items {
        match item {
            Item::Type(t) if !t.is_priv && exported(&t.name) => types.push(t),
            Item::Val(v) if !v.is_priv && exported(&v.name) => vals.push(v),
            Item::Foreign(f) if exported(&f.name) => foreigns.push(f),
            Item::Trait(_) | Item::Instance(_) => {}
            _ => {}
        }
    }

    if !types.is_empty() {
        out.push_str("## Types\n\n");
        for t in types {
            out.push_str(&format!("### `{}`\n\n", t.name));
            let docs = preceding_doc_lines(src, t.span.start.0 as usize);
            if !docs.is_empty() {
                out.push_str(&docs.join("\n"));
                out.push_str("\n\n");
            }
            match &t.kind {
                TypeKind::Product(fields) => {
                    out.push_str(&format!(
                        "Product type with fields: {}.\n\n",
                        fields
                            .iter()
                            .map(|f| format!("`{f}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                TypeKind::Sum(variants) => {
                    out.push_str("Sum type variants:\n\n");
                    for v in variants {
                        match &v.fields {
                            VariantFields::Unit => {
                                out.push_str(&format!("- `{}`\n", v.name));
                            }
                            VariantFields::Positional(names) => {
                                let holes = names.join(", ");
                                out.push_str(&format!("- `{}`({holes})\n", v.name));
                            }
                            VariantFields::Named(fields) => {
                                out.push_str(&format!("- `{}`({})\n", v.name, fields.join(", ")));
                            }
                        }
                    }
                    out.push('\n');
                }
            }
        }
    }

    if !foreigns.is_empty() {
        out.push_str("## Foreign\n\n");
        for f in foreigns {
            let pure = if f.is_pure { " pure" } else { "" };
            let params = f
                .params
                .iter()
                .map(|(n, t)| format!("{n}: {t}"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "### `foreign \"{}\"{pure} fn {}({params}) -> {}`\n\n",
                f.abi, f.name, f.ret
            ));
            let docs = preceding_doc_lines(src, f.span.start.0 as usize);
            if !docs.is_empty() {
                out.push_str(&docs.join("\n"));
                out.push_str("\n\n");
            }
        }
    }

    if !vals.is_empty() {
        out.push_str("## Values\n\n");
        for v in vals {
            let sig = match &v.params {
                Some(ps) if !ps.is_empty() => {
                    let names: Vec<&str> = ps.iter().map(|(n, _)| n.as_str()).collect();
                    format!("{}({})", v.name, names.join(", "))
                }
                _ => v.name.clone(),
            };
            out.push_str(&format!("### `{sig}`\n\n"));
            let docs = preceding_doc_lines(src, v.span.start.0 as usize);
            if !docs.is_empty() {
                out.push_str(&docs.join("\n"));
                out.push_str("\n\n");
            } else {
                out.push_str("_(no doc comment)_\n\n");
            }
        }
    }

    out
}

/// Collect `///` lines immediately above `byte_offset` (skipping blank lines).
fn preceding_doc_lines(src: &str, byte_offset: usize) -> Vec<String> {
    let before = &src[..byte_offset.min(src.len())];
    let mut docs = Vec::new();
    let mut saw_doc = false;
    for line in before.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if saw_doc {
                break;
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("///") {
            saw_doc = true;
            let text = rest.strip_prefix(' ').unwrap_or(rest);
            docs.push(text.to_string());
        } else {
            break;
        }
    }
    docs.reverse();
    docs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_module_and_val_docs() {
        let src = r#"
/// Module blurb.
///
/// @exports inc, main
module Demo

import std.io.{println}

/// Adds one.
val inc(x) = x + 1

priv val hidden = 0

val main = {
    println(inc(1))
}
"#;
        let m = parse_module(src).unwrap();
        let md = render_module(src, &m, Path::new("demo.lm"));
        assert!(md.contains("# Module `Demo`"));
        assert!(md.contains("Module blurb."));
        assert!(md.contains("**Exports:** `inc`, `main`"));
        assert!(md.contains("### `inc(x)`"));
        assert!(md.contains("Adds one."));
        assert!(!md.contains("hidden"));
        assert!(md.contains("### `main`"));
    }

    #[test]
    fn exports_hides_non_exported_public_vals() {
        let src = r#"
/// @exports onlyPublic
module DocVis
val hiddenHelper = 1
val onlyPublic = 2
priv val secret = 3
"#;
        let m = parse_module(src).unwrap();
        let md = render_module(src, &m, Path::new("doc_vis.lm"));
        assert!(md.contains("**Exports:** `onlyPublic`"));
        assert!(md.contains("### `onlyPublic`"));
        assert!(!md.contains("hiddenHelper"));
        assert!(!md.contains("secret"));
    }

    #[test]
    fn foreign_and_type_sections() {
        let src = r#"
module F

/// Option-like.
type Opt {
    Some(value)
    None
}

/// Absolute value.
foreign "C" pure fn llabs(x: Int) -> Int
"#;
        let m = parse_module(src).unwrap();
        let md = render_module(src, &m, Path::new("f.lm"));
        assert!(md.contains("### `Opt`"));
        assert!(md.contains("Option-like."));
        assert!(md.contains("`Some`"));
        assert!(md.contains("foreign \"C\" pure fn llabs"));
        assert!(md.contains("Absolute value."));
    }
}
