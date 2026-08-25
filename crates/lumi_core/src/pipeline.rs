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
use lumi_hir::lower_module;
use lumi_syntax::parse_module;
use lumi_ty::{typecheck_hir, NameVisibility, TypecheckOptions};

/// Options for the test/tooling frontend — same as the shared typecheck path.
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

/// Parse → HIR → [`typecheck_hir`] → Core (incl. mono).
///
/// Mirrors the CLI path up to (but not including) `lumi_opt::optimize`,
/// without multi-file load / visibility / assert annotation.
pub fn compile_source_to_core(src: &str) -> Result<CoreModule, String> {
    compile_source_to_core_with_options(src, &FrontendOptions::default())
}

/// Same as [`compile_source_to_core`] with explicit auto-parallel toggle.
pub fn compile_source_to_core_with_parallel(
    src: &str,
    auto_parallel: bool,
) -> Result<CoreModule, String> {
    compile_source_to_core_with_options(
        src,
        &FrontendOptions::default().with_parallel(auto_parallel),
    )
}

/// Parse → HIR → typecheck (infer + parallel finalize + effects) → Core.
pub fn compile_source_to_core_with_options(
    src: &str,
    opts: &FrontendOptions,
) -> Result<CoreModule, String> {
    let src = rewrite_lumi_io_imports_for_source_pipeline(src);
    let ast = stage("parse", parse_module(&src))?;
    let hir = stage("lower", lower_module(&ast))?;
    let typed = stage(
        "typecheck",
        typecheck_hir(&hir, NameVisibility::default(), opts),
    )?;
    Ok(lower_hir_with_schemes(
        &typed.module,
        &typed.fun_types,
        &typed.fun_schemes,
    ))
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
    fn auto_parallel_off_demotes_list_par_map() {
        let src = r#"
module M
val println(x) = { __println(x) }
val main = {
    println(listOf(1, 2, 3).map({ x -> x + 1 }).len())
}
"#;
        let with_par = compile_source_to_core_with_options(
            src,
            &FrontendOptions::default().with_parallel(true),
        )
        .expect("core");
        let no_par = compile_source_to_core_with_options(
            src,
            &FrontendOptions::default().with_parallel(false),
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
