//! Shared program typecheck + assert annotation for CLI and LSP.

use crate::load::{load_program, load_program_with_overlays, path_label, LoadedProgram, SourceFile};
use anyhow::Result;
use lumia_hir::lower_module;
use lumia_syntax::{parse_module_recovering, stamp_module, LocatedError, Span};
use lumia_ty::{
    typecheck_hir, typecheck_hir_recovering, NameVisibility, TypeError, TypecheckOptions,
    TypedModule,
};
use rustc_hash::FxHashMap as HashMap;
use std::path::{Path, PathBuf};

/// Resolve whether to honor `foreign "C" pure`.
///
/// - `Some(v)` — CLI / caller override (`--trust-foreign-pure` / `--no-trust-foreign-pure`)
/// - `None` — use `Lumia.toml` `package.trust_foreign_pure` (default false)
pub fn resolve_trust_foreign_pure(override_: Option<bool>, package: bool) -> bool {
    override_.unwrap_or(package)
}

enum AnalyzeError {
    Lower(LocatedError),
    Type(TypeError),
}

/// Lower + typecheck an already-loaded program (shared by CLI and overlay paths).
fn typecheck_loaded(
    loaded: &LoadedProgram,
    auto_parallel: bool,
    trust_foreign_pure: Option<bool>,
) -> Result<TypedModule, AnalyzeError> {
    let hir = lower_module(&loaded.module).map_err(AnalyzeError::Lower)?;
    let opts = TypecheckOptions {
        auto_parallel,
        trust_foreign_pure: resolve_trust_foreign_pure(trust_foreign_pure, loaded.trust_foreign_pure),
    };
    typecheck_hir(&hir, loaded.visibility.clone(), &opts).map_err(AnalyzeError::Type)
}

/// Load a program from disk and typecheck it (CLI `check` / `build` path).
///
/// `trust_foreign_pure`: `Some` overrides the package setting; `None` uses it.
pub fn check_program(
    file: &Path,
    auto_parallel: bool,
    trust_foreign_pure: Option<bool>,
) -> Result<(TypedModule, LoadedProgram)> {
    let loaded = load_program(file)?;
    let typed = match typecheck_loaded(&loaded, auto_parallel, trust_foreign_pure) {
        Ok(t) => t,
        Err(AnalyzeError::Lower(e)) => {
            return Err(diag_err(&loaded, e.span, "lower", &e.message));
        }
        Err(AnalyzeError::Type(e)) => return Err(type_err(&loaded, e)),
    };
    Ok((typed, loaded))
}

/// Error from [`check_program_with_overlays`] (preserves loaded sources for spans).
#[derive(Debug)]
pub enum OverlayCheckError {
    Load(String),
    Analyze {
        loaded: Box<LoadedProgram>,
        err: TypeError,
    },
}

/// Load with editor overlays and typecheck (LSP multi-file path).
///
/// `trust_foreign_pure`: `Some` overrides the package setting; `None` uses it
/// (same policy as CLI without `--trust-foreign-pure` / `--no-trust-foreign-pure`).
pub fn check_program_with_overlays(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
    auto_parallel: bool,
    trust_foreign_pure: Option<bool>,
) -> Result<(LoadedProgram, TypedModule), OverlayCheckError> {
    let loaded = load_program_with_overlays(path, overlays)
        .map_err(|e| OverlayCheckError::Load(format!("{e}")))?;
    match typecheck_loaded(&loaded, auto_parallel, trust_foreign_pure) {
        Ok(typed) => Ok((loaded, typed)),
        Err(AnalyzeError::Lower(e)) => Err(OverlayCheckError::Analyze {
            loaded: Box::new(loaded),
            err: e.into(),
        }),
        Err(AnalyzeError::Type(err)) => Err(OverlayCheckError::Analyze {
            loaded: Box::new(loaded),
            err,
        }),
    }
}

/// Single-buffer typecheck (unsaved / no on-disk entry).
pub fn check_source(text: &str, auto_parallel: bool) -> Result<TypedModule, (Span, String)> {
    let partial = check_source_recovering(text, auto_parallel);
    if let Some(typed) = partial.typed {
        if partial.diagnostics.is_empty() {
            return Ok(typed);
        }
        // Recovered parse errors: treat as failure for strict callers, but keep
        // the first diagnostic (CLI-style).
    }
    match partial.diagnostics.into_iter().next() {
        Some(d) => Err(d),
        None => Err((Span::dummy(), "analysis failed".into())),
    }
}

/// Partial check for IDE: keep later items after a local parse error.
#[derive(Debug, Default)]
pub struct PartialCheck {
    pub typed: Option<TypedModule>,
    pub diagnostics: Vec<(Span, String)>,
}

/// Parse with recovery, then lower/typecheck whatever items survived.
pub fn check_source_recovering(text: &str, auto_parallel: bool) -> PartialCheck {
    let outcome = parse_module_recovering(text);
    let mut diagnostics: Vec<(Span, String)> = outcome
        .errors
        .into_iter()
        .map(|e| (e.span, format!("parse: {}", e.message)))
        .collect();

    if outcome.module.name.is_empty() && outcome.module.items.is_empty() {
        return PartialCheck {
            typed: None,
            diagnostics,
        };
    }

    let mut m = outcome.module;
    stamp_module(&mut m, 0);
    let hir = match lower_module(&m) {
        Ok(h) => h,
        Err(e) => {
            diagnostics.push((e.span, format!("lower: {}", e.message)));
            return PartialCheck {
                typed: None,
                diagnostics,
            };
        }
    };
    let opts = TypecheckOptions {
        auto_parallel,
        trust_foreign_pure: false,
    };
    let (typed, ty_errs) = typecheck_hir_recovering(&hir, NameVisibility::default(), &opts);
    for e in ty_errs {
        diagnostics.push((e.span().unwrap_or_default(), e.message().to_string()));
    }
    PartialCheck { typed, diagnostics }
}

/// Inject `assert` failure messages (`file:line: assert failed`) before Core lower.
pub fn annotate_assert_messages(module: &mut lumia_hir::Module, loaded: &LoadedProgram) {
    let labels: Vec<String> = loaded
        .files
        .iter()
        .map(|f| path_label(&f.path))
        .collect();
    let table: Vec<(&str, &str)> = labels
        .iter()
        .zip(loaded.files.iter())
        .map(|(lab, f)| (lab.as_str(), f.src.as_str()))
        .collect();
    lumia_hir::annotate_assert_messages(module, &table);
}

fn diag_err(loaded: &LoadedProgram, span: Span, kind: &str, message: &str) -> anyhow::Error {
    let labels: Vec<String> = loaded
        .files
        .iter()
        .map(|f| path_label(&f.path))
        .collect();
    let table: Vec<(&str, &str)> = labels
        .iter()
        .zip(loaded.files.iter())
        .map(|(lab, f)| (lab.as_str(), f.src.as_str()))
        .collect();
    anyhow::anyhow!(lumia_syntax::format_diagnostic_files(
        &table, span, kind, message,
    ))
}

fn type_err(loaded: &LoadedProgram, e: TypeError) -> anyhow::Error {
    match e.span() {
        Some(span) => diag_err(loaded, span, "type", e.message()),
        None => anyhow::anyhow!("type: {}", e.message()),
    }
}

/// Minimal [`LoadedProgram`] for unit tests (single in-memory file).
pub fn loaded_from_source(path: &str, src: &str) -> LoadedProgram {
    use lumia_syntax::{Module as SynModule, Span};
    LoadedProgram {
        files: vec![SourceFile {
            path: PathBuf::from(path),
            src: src.to_string(),
        }],
        module: SynModule {
            name: "M".into(),
            span: Span::dummy(),
            imports: Vec::new(),
            items: Vec::new(),
        },
        link_args: Vec::new(),
        trust_foreign_pure: false,
        visibility: NameVisibility::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_hir::{Builtin, Expr, Item, Module as HirModule};
    use lumia_syntax::Span;
    use rustc_hash::{FxHashMap, FxHashSet};

    #[test]
    fn trust_foreign_pure_override_beats_package() {
        assert!(!resolve_trust_foreign_pure(Some(false), true));
        assert!(resolve_trust_foreign_pure(Some(true), false));
        assert!(resolve_trust_foreign_pure(None, true));
        assert!(!resolve_trust_foreign_pure(None, false));
    }

    #[test]
    fn recovering_keeps_later_item_types() {
        let src = r#"
module Main
import std.io.{println}
val add = { a, b -> a + b
val main = {
    println(1)
}
"#;
        let partial = check_source_recovering(src, true);
        assert!(!partial.diagnostics.is_empty(), "expected parse diagnostic");
        let typed = partial.typed.expect("typecheck recovered items");
        assert!(
            typed.fun_types.contains_key("main") || typed.fun_schemes.contains_key("main"),
            "main should still be typed, keys={:?}",
            typed.fun_types.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn annotate_assert_adds_file_line_message() {
        let src = "module M\nval main = { assert(false) }\n";
        let loaded = loaded_from_source("t.lm", src);
        let start = src.find("assert").expect("assert") as u32;
        let end = (src.find(')').expect(")") as u32) + 1;
        let assert_span = Span::new(start, end);
        let mut module = HirModule {
            name: "M".into(),
            items: vec![Item::Val {
                name: "main".into(),
                body: Expr::BuiltinCall {
                    name: Builtin::Assert,
                    args: vec![Expr::Bool(false, assert_span)],
                    span: assert_span,
                },
                ty: None,
                span: assert_span,
                is_priv: false,
            }],
            adts: Vec::new(),
            products: Vec::new(),
            instances: FxHashSet::default(),
            show_methods: FxHashMap::default(),
            trait_methods: FxHashMap::default(),
            method_traits: FxHashMap::default(),
        };
        annotate_assert_messages(&mut module, &loaded);
        let Item::Val { body, .. } = &module.items[0] else {
            panic!("expected val");
        };
        let Expr::BuiltinCall { args, .. } = body else {
            panic!("expected builtin");
        };
        assert_eq!(args.len(), 2);
        match &args[1] {
            Expr::String(msg, _) => {
                assert!(
                    msg.contains("t.lm:2: assert failed"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected string message, got {other:?}"),
        }
    }
}
