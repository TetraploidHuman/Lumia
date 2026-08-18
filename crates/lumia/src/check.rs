//! Shared program typecheck for CLI and LSP.

use crate::diag::{Diagnostic, DiagnosticKind};
use crate::load::{
    load_program, load_program_with_overlays, path_label, LoadedProgram, SourceFile,
};
use anyhow::Result;
use lumia_hir::{lower_module, lower_module_recovering};
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
        trust_foreign_pure: resolve_trust_foreign_pure(
            trust_foreign_pure,
            loaded.trust_foreign_pure,
        ),
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
        kind: DiagnosticKind,
        err: TypeError,
    },
}

/// Load with editor overlays and typecheck (LSP multi-file path).
///
/// `trust_foreign_pure`: `Some` overrides the package setting; `None` uses it
/// (same policy as CLI without `--trust-foreign-pure` / `--no-trust-foreign-pure`).
///
/// Fail-fast: first lower/type error only (CLI-style). Prefer
/// [`check_program_with_overlays_recovering`] for IDE multi-diagnostic.
pub fn check_program_with_overlays(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
    auto_parallel: bool,
    trust_foreign_pure: Option<bool>,
) -> Result<(LoadedProgram, TypedModule), OverlayCheckError> {
    let partial =
        check_program_with_overlays_recovering(path, overlays, auto_parallel, trust_foreign_pure)?;
    if let Some(typed) = partial.typed {
        if !partial.diagnostics.iter().any(|d| d.kind.is_error()) {
            return Ok((partial.loaded, typed));
        }
    }
    match partial.diagnostics.into_iter().find(|d| d.kind.is_error()) {
        Some(d) => Err(OverlayCheckError::Analyze {
            loaded: Box::new(partial.loaded),
            kind: d.kind,
            err: LocatedError {
                message: d.message,
                span: d.span,
            }
            .into(),
        }),
        None => Err(OverlayCheckError::Load("analysis failed".into())),
    }
}

/// Multi-file load + lower/typecheck with recovery (all soft diagnostics).
pub fn check_program_with_overlays_recovering(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
    auto_parallel: bool,
    trust_foreign_pure: Option<bool>,
) -> Result<PartialProgramCheck, OverlayCheckError> {
    let loaded = load_program_with_overlays(path, overlays)
        .map_err(|e| OverlayCheckError::Load(format!("{e}")))?;
    Ok(typecheck_loaded_recovering(
        loaded,
        auto_parallel,
        trust_foreign_pure,
    ))
}

/// Partial multi-file check (IDE): keep TypedModule when some items error.
#[derive(Debug)]
pub struct PartialProgramCheck {
    pub loaded: LoadedProgram,
    pub typed: Option<TypedModule>,
    pub diagnostics: Vec<Diagnostic>,
}

fn typecheck_loaded_recovering(
    loaded: LoadedProgram,
    auto_parallel: bool,
    trust_foreign_pure: Option<bool>,
) -> PartialProgramCheck {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    // Package honor-system: surface as LSP Warning (CLI load still eprintln! Once).
    let effective = resolve_trust_foreign_pure(trust_foreign_pure, loaded.trust_foreign_pure);
    if loaded.trust_foreign_pure && effective {
        diagnostics.push(Diagnostic::new(
            Span::dummy(),
            DiagnosticKind::Warning,
            "package.trust_foreign_pure=true honors unverified `foreign \"C\" pure` \
             (same trust surface as --trust-foreign-pure; override with --no-trust-foreign-pure)",
        ));
    }
    let (hir_opt, lower_errs) = lower_module_recovering(&loaded.module);
    diagnostics.extend(
        lower_errs
            .into_iter()
            .map(|e| Diagnostic::new(e.span, DiagnosticKind::Lower, e.message)),
    );
    let Some(hir) = hir_opt else {
        return PartialProgramCheck {
            loaded,
            typed: None,
            diagnostics,
        };
    };
    let opts = TypecheckOptions {
        auto_parallel,
        trust_foreign_pure: effective,
    };
    let (typed, ty_errs) = typecheck_hir_recovering(&hir, loaded.visibility.clone(), &opts);
    for e in ty_errs {
        diagnostics.push(Diagnostic::new(
            e.span().unwrap_or_default(),
            DiagnosticKind::Type,
            e.message().to_string(),
        ));
    }
    PartialProgramCheck {
        loaded,
        typed,
        diagnostics,
    }
}

/// Single-buffer typecheck (unsaved / no on-disk entry).
pub fn check_source(text: &str, auto_parallel: bool) -> Result<TypedModule, (Span, String)> {
    let partial = check_source_recovering(text, auto_parallel);
    if let Some(typed) = partial.typed {
        if !partial.diagnostics.iter().any(|d| d.kind.is_error()) {
            return Ok(typed);
        }
        // Recovered parse errors: treat as failure for strict callers, but keep
        // the first hard diagnostic (CLI-style).
    }
    match partial.diagnostics.into_iter().find(|d| d.kind.is_error()) {
        Some(d) => Err((d.span, d.display_message())),
        None => Err((Span::dummy(), "analysis failed".into())),
    }
}

/// Partial check for IDE: keep later items after a local parse error.
#[derive(Debug, Default)]
pub struct PartialCheck {
    pub typed: Option<TypedModule>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse with recovery, then lower/typecheck whatever items survived.
pub fn check_source_recovering(text: &str, auto_parallel: bool) -> PartialCheck {
    let outcome = parse_module_recovering(text);
    let mut diagnostics: Vec<Diagnostic> = outcome
        .errors
        .into_iter()
        .map(|e| Diagnostic::new(e.span, DiagnosticKind::Parse, e.message))
        .collect();

    if outcome.module.name.is_empty() && outcome.module.items.is_empty() {
        return PartialCheck {
            typed: None,
            diagnostics,
        };
    }

    let mut m = outcome.module;
    stamp_module(&mut m, 0);
    let (hir_opt, lower_errs) = lower_module_recovering(&m);
    for e in lower_errs {
        diagnostics.push(Diagnostic::new(e.span, DiagnosticKind::Lower, e.message));
    }
    let Some(hir) = hir_opt else {
        return PartialCheck {
            typed: None,
            diagnostics,
        };
    };
    let opts = TypecheckOptions {
        auto_parallel,
        trust_foreign_pure: false,
    };
    let (typed, ty_errs) = typecheck_hir_recovering(&hir, NameVisibility::default(), &opts);
    for e in ty_errs {
        diagnostics.push(Diagnostic::new(
            e.span().unwrap_or_default(),
            DiagnosticKind::Type,
            e.message().to_string(),
        ));
    }
    PartialCheck { typed, diagnostics }
}

fn diag_err(loaded: &LoadedProgram, span: Span, kind: &str, message: &str) -> anyhow::Error {
    let labels: Vec<String> = loaded.files.iter().map(|f| path_label(&f.path)).collect();
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

    #[test]
    fn trust_foreign_pure_override_beats_package() {
        assert!(!resolve_trust_foreign_pure(Some(false), true));
        assert!(resolve_trust_foreign_pure(Some(true), false));
        assert!(resolve_trust_foreign_pure(None, true));
        assert!(!resolve_trust_foreign_pure(None, false));
    }

    #[test]
    fn package_trust_foreign_pure_emits_warning_not_error() {
        let mut loaded = loaded_from_source("Main.lm", "module Main\nval main = 0\n");
        loaded.trust_foreign_pure = true;
        let partial = typecheck_loaded_recovering(loaded, true, None);
        assert!(partial.typed.is_some());
        assert!(
            partial
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::Warning
                    && d.message.contains("trust_foreign_pure")),
            "expected trust Warning, got {:?}",
            partial.diagnostics
        );
        assert!(
            !partial.diagnostics.iter().any(|d| d.kind.is_error()),
            "trust Warning must not be a hard error"
        );
        // CLI force-off: no advisory.
        let mut loaded2 = loaded_from_source("Main.lm", "module Main\nval main = 0\n");
        loaded2.trust_foreign_pure = true;
        let partial2 = typecheck_loaded_recovering(loaded2, true, Some(false));
        assert!(
            !partial2
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::Warning),
            " --no-trust-foreign-pure should suppress Warning"
        );
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
    fn recovering_parse_hole_does_not_false_green_calls() {
        let src = r#"
module Main
val broken = { a, b -> a +
val ok = broken(1, 2)
"#;
        let partial = check_source_recovering(src, true);
        assert!(
            partial
                .diagnostics
                .iter()
                .any(|d| matches!(d.kind, DiagnosticKind::Parse)),
            "expected parse diagnostic, got {:?}",
            partial.diagnostics
        );
        let typed = partial.typed.expect("partial typed module");
        assert!(
            !typed.fun_schemes.contains_key("broken"),
            "broken must not get an identity scheme, got {:?}",
            typed.fun_schemes.get("broken")
        );
        assert!(
            partial.diagnostics.iter().any(|d| {
                matches!(d.kind, DiagnosticKind::Type)
                    && (d.message.contains("broken") || d.message.contains("__parse_hole"))
            }),
            "expected type diagnostic for hole/use, got {:?}",
            partial.diagnostics
        );
    }

    #[test]
    fn recovering_reports_multiple_lower_errors() {
        let src = r#"
module Main
type Opt { Some(v) None }
val main = {
  val BadCtor(x) = None
  val AlsoBad(y) = None
  0
}
"#;
        let partial = check_source_recovering(src, true);
        let lower = partial
            .diagnostics
            .iter()
            .filter(|d| matches!(d.kind, DiagnosticKind::Lower))
            .collect::<Vec<_>>();
        assert_eq!(lower.len(), 2, "diags={:?}", partial.diagnostics);
    }

    #[test]
    fn recovering_reports_multiple_type_errors() {
        let src = r#"
module Main
val a: Int = "x"
val b: Int = "y"
val main = 0
"#;
        let partial = check_source_recovering(src, true);
        let types = partial
            .diagnostics
            .iter()
            .filter(|d| matches!(d.kind, DiagnosticKind::Type))
            .collect::<Vec<_>>();
        assert!(
            types.len() >= 2,
            "expected ≥2 type diags, got {:?}",
            partial.diagnostics
        );
    }

    #[test]
    fn overlays_recovering_collects_multiple_type_errors() {
        let dir = std::env::temp_dir().join(format!("lumia_ov_multi_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let main = dir.join("Main.lm");
        std::fs::write(
            &main,
            r#"
module Main
val a: Int = "x"
val b: Int = "y"
val main = 0
"#,
        )
        .unwrap();
        let partial =
            check_program_with_overlays_recovering(&main, &HashMap::default(), true, None)
                .expect("load");
        let types = partial
            .diagnostics
            .iter()
            .filter(|d| matches!(d.kind, DiagnosticKind::Type))
            .collect::<Vec<_>>();
        assert!(
            types.len() >= 2,
            "expected ≥2 type diags, got {:?}",
            partial.diagnostics
        );
        // Fail-fast wrapper still surfaces a single Analyze error.
        let err = check_program_with_overlays(&main, &HashMap::default(), true, None)
            .expect_err("expected type error");
        match err {
            OverlayCheckError::Analyze { .. } => {}
            OverlayCheckError::Load(m) => panic!("unexpected load: {m}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
