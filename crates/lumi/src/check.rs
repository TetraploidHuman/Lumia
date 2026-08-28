//! Shared program typecheck + assert annotation for CLI and LSP.

use crate::caps::CapabilitySet;
use crate::profile::CompileProfile;
use crate::load::{
    load_program, load_program_with_overlays, path_label, LoadedProgram, SourceFile,
};
use anyhow::Result;
use lumi_hir::lower_module_with_options;
use lumi_syntax::{parse_module_recovering, stamp_module, Span};
use lumi_ty::{typecheck_hir, typecheck_hir_recovering, NameVisibility, TypeError, TypedModule};
use rustc_hash::FxHashMap as HashMap;
use std::path::{Path, PathBuf};

/// Load a program from disk and typecheck it (CLI `check` / `build` path).
pub fn check_program(
    file: &Path,
    auto_parallel: bool,
    trust_foreign_pure: bool,
) -> Result<(TypedModule, LoadedProgram)> {
    check_program_with_caps(
        file,
        &CapabilitySet::stock().with_auto_parallel(auto_parallel),
        trust_foreign_pure,
    )
}

/// Same as [`check_program`] with an explicit [`CapabilitySet`] (Phase C).
pub fn check_program_with_caps(
    file: &Path,
    caps: &CapabilitySet,
    trust_foreign_pure: bool,
) -> Result<(TypedModule, LoadedProgram)> {
    check_program_with_profile(
        file,
        &CompileProfile::stock(false)
            .with_caps(caps.clone())
            .with_trust_foreign_pure(trust_foreign_pure),
    )
}

/// Typecheck with a full [`CompileProfile`] (Phase E).
pub fn check_program_with_profile(
    file: &Path,
    profile: &CompileProfile,
) -> Result<(TypedModule, LoadedProgram)> {
    let loaded = load_program(file)?;
    let hir = lower_module_with_options(&loaded.module, &profile.caps.to_lower_options())
        .map_err(|e| diag_err(&loaded, e.span, "lower", &e.message))?;
    let opts = profile
        .caps
        .to_typecheck_options(profile.trust_foreign_pure || loaded.trust_foreign_pure);
    let typed =
        typecheck_hir(&hir, loaded.visibility.clone(), &opts).map_err(|e| type_err(&loaded, e))?;
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
pub fn check_program_with_overlays(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
    profile: &CompileProfile,
) -> Result<(LoadedProgram, TypedModule), OverlayCheckError> {
    let loaded = load_program_with_overlays(path, overlays)
        .map_err(|e| OverlayCheckError::Load(format!("{e}")))?;
    let hir = lower_module_with_options(&loaded.module, &profile.caps.to_lower_options()).map_err(
        |e| OverlayCheckError::Analyze {
            loaded: Box::new(loaded.clone()),
            err: e.into(),
        },
    )?;
    let opts = profile.caps.to_typecheck_options(
        profile.trust_foreign_pure || loaded.trust_foreign_pure,
    );
    match typecheck_hir(&hir, loaded.visibility.clone(), &opts) {
        Ok(typed) => Ok((loaded, typed)),
        Err(err) => Err(OverlayCheckError::Analyze {
            loaded: Box::new(loaded),
            err,
        }),
    }
}

/// Single-buffer typecheck (unsaved / no on-disk entry).
pub fn check_source(text: &str, auto_parallel: bool) -> Result<TypedModule, (Span, String)> {
    let profile = CompileProfile::stock(false).with_caps(
        CapabilitySet::stock().with_auto_parallel(auto_parallel),
    );
    let partial = check_source_recovering(text, &profile);
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
pub fn check_source_recovering(text: &str, profile: &CompileProfile) -> PartialCheck {
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
    let mut visibility = NameVisibility::default();
    if let Err(e) = crate::load::apply_default_stdlib_to_module(&mut m, &mut visibility) {
        diagnostics.push((Span::dummy(), format!("stdlib: {e}")));
        return PartialCheck {
            typed: None,
            diagnostics,
        };
    }
    let caps = &profile.caps;
    let hir = match lower_module_with_options(&m, &caps.to_lower_options()) {
        Ok(h) => h,
        Err(e) => {
            diagnostics.push((e.span, format!("lower: {}", e.message)));
            return PartialCheck {
                typed: None,
                diagnostics,
            };
        }
    };
    let opts = caps.to_typecheck_options(profile.trust_foreign_pure);
    let (typed, ty_errs) = typecheck_hir_recovering(&hir, visibility, &opts);
    for e in ty_errs {
        diagnostics.push((e.span().unwrap_or_default(), e.message().to_string()));
    }
    PartialCheck { typed, diagnostics }
}

/// Inject `assert` failure messages (`file:line: assert failed`) before Core lower.
pub fn annotate_assert_messages(module: &mut lumi_hir::Module, loaded: &LoadedProgram) {
    for item in &mut module.items {
        match item {
            lumi_hir::Item::Fun(f) => annotate_assert_expr(&mut f.body, loaded),
            lumi_hir::Item::Val { body, .. } => annotate_assert_expr(body, loaded),
        }
    }
}

fn annotate_assert_expr(e: &mut lumi_hir::Expr, loaded: &LoadedProgram) {
    use lumi_hir::{Builtin, Expr};
    match e {
        Expr::BuiltinCall {
            name: Builtin::Assert,
            args,
            span,
        } => {
            for a in args.iter_mut() {
                annotate_assert_expr(a, loaded);
            }
            ensure_assert_message(args, *span, loaded);
        }
        Expr::Call { callee, args, span } => {
            if let Expr::Var(name, _) = callee.as_ref() {
                if name == "assert" {
                    for a in args.iter_mut() {
                        annotate_assert_expr(a, loaded);
                    }
                    if args.len() == 1 {
                        let cond = args.remove(0);
                        let mut assert_args = vec![cond];
                        ensure_assert_message(&mut assert_args, *span, loaded);
                        *e = Expr::BuiltinCall {
                            name: Builtin::Assert,
                            args: assert_args,
                            span: *span,
                        };
                    }
                    return;
                }
            }
            annotate_assert_expr(callee, loaded);
            for a in args {
                annotate_assert_expr(a, loaded);
            }
        }
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            for a in args {
                annotate_assert_expr(a, loaded);
            }
        }
        Expr::Let { value, body, .. } => {
            annotate_assert_expr(value, loaded);
            annotate_assert_expr(body, loaded);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            annotate_assert_expr(cond, loaded);
            annotate_assert_expr(then_branch, loaded);
            annotate_assert_expr(else_branch, loaded);
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            annotate_assert_expr(cond, loaded);
            annotate_assert_expr(body, loaded);
            if let Some(s) = step {
                annotate_assert_expr(s, loaded);
            }
        }
        Expr::Binary { left, right, .. } => {
            annotate_assert_expr(left, loaded);
            annotate_assert_expr(right, loaded);
        }
        Expr::Unary { expr, .. } => annotate_assert_expr(expr, loaded),
        Expr::Lambda { body, .. } => annotate_assert_expr(body, loaded),
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                annotate_assert_expr(s, loaded);
            }
        }
        Expr::Assign { value, .. } | Expr::Return { value, .. } => {
            annotate_assert_expr(value, loaded)
        }
        Expr::Alt { scrutinee, alt, .. } => {
            annotate_assert_expr(scrutinee, loaded);
            annotate_assert_expr(alt, loaded);
        }
        Expr::With { base, fields, .. } => {
            annotate_assert_expr(base, loaded);
            for (_, e) in fields {
                annotate_assert_expr(e, loaded);
            }
        }
        Expr::Var(_, _)
        | Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Bool(_, _)
        | Expr::String(_, _)
        | Expr::Char(_, _)
        | Expr::Unit(_)
        | Expr::Break(_)
        | Expr::Continue(_) => {}
    }
}

/// Append `path:line: assert failed` when the assert has only a condition.
fn ensure_assert_message(
    args: &mut Vec<lumi_hir::Expr>,
    span: lumi_syntax::Span,
    loaded: &LoadedProgram,
) {
    if args.len() != 1 {
        return;
    }
    let file = loaded.file(span.file);
    let starts = lumi_syntax::line_starts(&file.src);
    let (line, _) = lumi_syntax::byte_to_line_col(&starts, span.start);
    let msg = format!("{}:{}: assert failed", path_label(&file.path), line);
    args.push(lumi_hir::Expr::String(msg, span));
}

fn diag_err(loaded: &LoadedProgram, span: Span, kind: &str, message: &str) -> anyhow::Error {
    let file = loaded.file(span.file);
    anyhow::anyhow!(lumi_syntax::format_diagnostic(
        &path_label(&file.path),
        &file.src,
        span,
        kind,
        message,
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
    use lumi_syntax::{Module as SynModule, Span};
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
    use lumi_hir::{Expr, Item, Module as HirModule};
    use lumi_syntax::Span;
    use rustc_hash::{FxHashMap, FxHashSet};

    #[test]
    fn recovering_keeps_later_item_types() {
        let src = r#"
module Main
import lumi.io.{println}
val add = { a, b -> a + b
val main = {
    println(1)
}
"#;
        let partial = check_source_recovering(src, &CompileProfile::stock(false));
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
                body: Expr::Call {
                    callee: Box::new(Expr::Var("assert".into(), assert_span)),
                    args: vec![Expr::Bool(false, assert_span)],
                    span: assert_span,
                },
                ty: None,
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
