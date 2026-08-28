//! Shared frontend→Core pipeline for tests and tooling.
//!
//! Multi-file load, import visibility, and assert-message annotation remain
//! CLI-only ([`lumi`] crate). Effect-boundary checks mirror the CLI via
//! [`lumi_ty::typecheck_hir`].
//!
//! Source-only compilation rewrites `import lumi.io.{…}` into local intrinsic
//! wrappers so examples compile without the package loader.

use crate::ir::CoreModule;
use crate::lower::lower_hir_with_schemes;
use lumi_hir::{lower_module_with_options, LowerOptions};
use lumi_syntax::parse_module;
use lumi_ty::{typecheck_hir, NameVisibility, TypecheckOptions};

/// Frontend pipeline options: HIR lower + typecheck (mirrors Phase C caps on the CLI path).
#[derive(Debug, Clone)]
pub struct PipelineOptions {
    pub lower: LowerOptions,
    pub typecheck: TypecheckOptions,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            lower: LowerOptions::default(),
            typecheck: TypecheckOptions::default(),
        }
    }
}

impl PipelineOptions {
    pub fn stock() -> Self {
        Self::default()
    }

    pub fn with_parallel(mut self, auto_parallel: bool) -> Self {
        self.typecheck.auto_parallel = auto_parallel;
        self
    }

    pub fn with_hof_fuse(mut self, on: bool) -> Self {
        self.lower.hof_fuse = on;
        self
    }

    pub fn with_trust_foreign_pure(mut self, on: bool) -> Self {
        self.typecheck.trust_foreign_pure = on;
        self
    }
}

/// Typecheck-only view (legacy alias). Prefer [`PipelineOptions`] for new callers.
pub type FrontendOptions = TypecheckOptions;

/// Format a staged pipeline failure (`parse: …`, `lower: …`, …).
fn stage<T, E: std::fmt::Display>(name: &str, r: Result<T, E>) -> Result<T, String> {
    r.map_err(|e| format!("{name}: {e}"))
}

/// Rewrite `import lumi.io.{…}` / `import lumi.io.*` into local vals that call
/// `__println` / `__readStdin` / `__assert`. The CLI loader inlines real
/// `lumi/io.lm`; this keeps the source-only test pipeline working.
fn rewrite_lumi_io_imports_for_source_pipeline(src: &str) -> String {
    let mut wrappers: Vec<String> = Vec::new();
    let mut body: Vec<&str> = Vec::new();
    for line in src.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("import lumi.io.") {
            if rest == "*" {
                push_io_wrapper(&mut wrappers, "println", "println");
                push_io_wrapper(&mut wrappers, "readStdin", "readStdin");
                push_io_wrapper(&mut wrappers, "assert", "assert");
                continue;
            }
            if let Some(inner) = rest.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                for part in inner.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    let (export, local) = match part.split_once(" as ") {
                        Some((n, a)) => (n.trim(), a.trim()),
                        None => (part, part),
                    };
                    push_io_wrapper(&mut wrappers, export, local);
                }
                continue;
            }
        }
        body.push(line);
    }
    inject_default_io_wrappers_if_needed(src, &mut wrappers);
    if wrappers.is_empty() {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len() + wrappers.len() * 40);
    let mut inserted = false;
    for (i, line) in body.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line);
        if !inserted && line.trim_start().starts_with("module ") {
            out.push('\n');
            for w in &wrappers {
                out.push('\n');
                out.push_str(w);
            }
            inserted = true;
        }
    }
    if !inserted {
        let mut prefix = wrappers.join("\n");
        prefix.push('\n');
        prefix.push_str(&out);
        return prefix;
    }
    if src.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn push_io_wrapper(out: &mut Vec<String>, export: &str, local: &str) {
    match export {
        "println" => out.push(format!("val {local}(x) = {{ __println(x) }}")),
        "readStdin" => out.push(format!("val {local} = {{ __readStdin() }}")),
        "assert" => out.push(format!("val {local}(c) = {{ __assert(c) }}")),
        _ => {}
    }
}

/// When no `import lumi.io.*` was present, mirror CLI auto-import of `lumi.io`
/// for examples/tests that call `println` / `readStdin` / `assert` directly.
fn inject_default_io_wrappers_if_needed(src: &str, wrappers: &mut Vec<String>) {
    if !wrappers.is_empty() {
        return;
    }
    if src.contains("println(") && !src.contains("val println") {
        push_io_wrapper(wrappers, "println", "println");
    }
    if src.contains("readStdin(") && !src.contains("val readStdin") {
        push_io_wrapper(wrappers, "readStdin", "readStdin");
    }
    if src.contains("assert(") && !src.contains("val assert") {
        push_io_wrapper(wrappers, "assert", "assert");
    }
}

/// Parse → HIR → [`typecheck_hir`] → Core (incl. mono).
///
/// Mirrors the CLI path up to (but not including) `lumi_opt::optimize`,
/// without multi-file load / visibility / assert annotation.
pub fn compile_source_to_core(src: &str) -> Result<CoreModule, String> {
    compile_source_to_core_with_pipeline(src, &PipelineOptions::default())
}

/// Same as [`compile_source_to_core`] with explicit auto-parallel toggle.
pub fn compile_source_to_core_with_parallel(
    src: &str,
    auto_parallel: bool,
) -> Result<CoreModule, String> {
    compile_source_to_core_with_pipeline(
        src,
        &PipelineOptions::default().with_parallel(auto_parallel),
    )
}

/// Parse → HIR → typecheck (infer + parallel finalize + effects) → Core.
pub fn compile_source_to_core_with_pipeline(
    src: &str,
    opts: &PipelineOptions,
) -> Result<CoreModule, String> {
    let src = rewrite_lumi_io_imports_for_source_pipeline(src);
    let ast = stage("parse", parse_module(&src))?;
    let hir = stage(
        "lower",
        lower_module_with_options(&ast, &opts.lower),
    )?;
    let typed = stage(
        "typecheck",
        typecheck_hir(&hir, NameVisibility::default(), &opts.typecheck),
    )?;
    Ok(lower_hir_with_schemes(
        &typed.module,
        &typed.fun_types,
        &typed.fun_schemes,
    ))
}

/// Legacy: typecheck options only (`hof_fuse` stays at default).
pub fn compile_source_to_core_with_options(
    src: &str,
    opts: &FrontendOptions,
) -> Result<CoreModule, String> {
    compile_source_to_core_with_pipeline(
        src,
        &PipelineOptions {
            lower: LowerOptions::default(),
            typecheck: opts.clone(),
        },
    )
}

/// Read a `.lm` file and compile through to Core.
pub fn compile_file_to_core(path: &std::path::Path) -> Result<CoreModule, String> {
    let src = stage("read", std::fs::read_to_string(path))?;
    compile_source_to_core(&src)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Op, Value};
    use lumi_hir::Builtin;

    fn has_builtin(core: &CoreModule, b: Builtin) -> bool {
        core.functions.iter().any(|f| {
            f.body.ops.iter().any(|op| match op {
                Op::Let { value, .. } | Op::Effect { value } => {
                    matches!(value, Value::Builtin { name, .. } if *name == b)
                }
                _ => false,
            })
        })
    }

    #[test]
    fn rewrite_io_import_injects_println_wrapper() {
        let src = "module M\nimport lumi.io.{println}\nval main = { println(1) }\n";
        let out = rewrite_lumi_io_imports_for_source_pipeline(src);
        assert!(out.contains("val println(x) = { __println(x) }"), "{out}");
        assert!(!out.contains("import lumi.io"), "{out}");
        compile_source_to_core(src).expect("core");
    }

    #[test]
    fn rewrite_io_import_alias() {
        let src = "module M\nimport lumi.io.{println as log}\nval main = { log(1) }\n";
        let out = rewrite_lumi_io_imports_for_source_pipeline(src);
        assert!(out.contains("val log(x) = { __println(x) }"), "{out}");
        compile_source_to_core(src).expect("core");
    }

    #[test]
    fn auto_inject_println_without_import() {
        let src = "module M\nval main = { println(1) }\n";
        let out = rewrite_lumi_io_imports_for_source_pipeline(src);
        assert!(out.contains("val println(x) = { __println(x) }"), "{out}");
        compile_source_to_core(src).expect("core");
    }

    #[test]
    fn trust_foreign_pure_allows_pure_ffi() {
        let src = r#"
module M
foreign "C" pure fn add(a: Int, b: Int) -> Int
val main = { add(1, 2) }
"#;
        let err = compile_source_to_core(src).expect_err("default rejects foreign pure");
        assert!(
            err.to_lowercase().contains("pure") || err.to_lowercase().contains("trust"),
            "unexpected error: {err}"
        );
        let ok = compile_source_to_core_with_options(
            src,
            &FrontendOptions::default().with_trust_foreign_pure(true),
        );
        assert!(ok.is_ok(), "trusted foreign pure: {ok:?}");
    }

    #[test]
    fn hof_fuse_off_still_lowers() {
        let src = r#"
module M
val println(x) = { __println(x) }
val main = {
    println(listOf(1, 2, 3).map({ x -> x + 1 }).len())
}
"#;
        let on = compile_source_to_core_with_pipeline(
            src,
            &PipelineOptions::default().with_hof_fuse(true),
        )
        .expect("hof_fuse on");
        let off = compile_source_to_core_with_pipeline(
            src,
            &PipelineOptions::default().with_hof_fuse(false),
        )
        .expect("hof_fuse off");
        assert!(!on.functions.is_empty() && !off.functions.is_empty());
    }

    #[test]
    fn auto_parallel_off_demotes_list_par_map() {
        let src = r#"
module M
val println(x) = { __println(x) }
val main = {
    println(listOf(1, 2, 3).map({ x -> x + 1 }).len())
}
"#;
        let with_par = compile_source_to_core_with_pipeline(
            src,
            &PipelineOptions::default().with_parallel(true),
        )
        .expect("core");
        let no_par = compile_source_to_core_with_pipeline(
            src,
            &PipelineOptions::default().with_parallel(false),
        )
        .expect("core");
        assert!(
            !has_builtin(&no_par, Builtin::ListParMap),
            "ListParMap must be absent when auto_parallel=false"
        );
        let _ = with_par;
    }

    #[test]
    fn effect_boundaries_run_on_default_pipeline() {
        let bad = r#"
module Bad
val println(x) = { __println(x) }
val xs = println(1)
val main = { 0 }
"#;
        let err = compile_source_to_core(bad).expect_err("top-level IO");
        assert!(
            err.contains("typecheck:") || err.to_lowercase().contains("effect"),
            "unexpected error: {err}"
        );
        let ok = r#"
module Ok
val println(x) = { __println(x) }
val main = { println(1) }
"#;
        compile_source_to_core(ok).expect("main may perform IO");
    }
}
