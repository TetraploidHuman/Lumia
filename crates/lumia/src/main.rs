mod doc;
mod load;
mod lsp;
mod pkg;
mod vis;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crate::load::{load_program, LoadedProgram};
use lumia_codegen::{compile_module, find_runtime_lib_prefer, CodegenOptions};
use lumia_core::{format_module, lower_hir};
use lumia_hir::lower_module;
use lumia_opt::{optimize, OptOptions};
use lumia_syntax::{format_diagnostic, parse_module, stamp_module, Span};
use lumia_ty::{
    check_effect_boundaries, finalize_auto_parallel, infer_module_with_options, InferOptions,
    TypeError,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser, Debug)]
#[command(name = "lumia", version, about = "Lumia compiler")]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Type- and effect-check only
    Check {
        file: PathBuf,
        /// Disable auto-parallel `List.map` (default: on when safe).
        #[arg(long = "no-parallel")]
        no_parallel: bool,
        /// Deprecated no-op (auto-parallel is on by default).
        #[arg(long, hide = true)]
        parallel: bool,
        /// Trust `foreign "C" pure` (FFI purity is not verified).
        #[arg(long = "trust-foreign-pure")]
        trust_foreign_pure: bool,
    },
    /// Compile to a native executable
    Build {
        file: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        release: bool,
        /// Disable transparent Memo `T_f` even in `--release` (for benchmarks).
        #[arg(long = "no-memo", alias = "no-memo-l2")]
        no_memo: bool,
        /// Disable auto-parallel `List.map` (default: on when safe; DESIGN §11.1).
        #[arg(long = "no-parallel")]
        no_parallel: bool,
        /// Deprecated no-op (auto-parallel is on by default).
        #[arg(long, hide = true)]
        parallel: bool,
        /// Trust `foreign "C" pure` (FFI purity is not verified).
        #[arg(long = "trust-foreign-pure")]
        trust_foreign_pure: bool,
        /// Extra linker args (repeatable), e.g. `--link -lm --link -L/opt/lib`.
        #[arg(long = "link", value_name = "ARG")]
        link: Vec<String>,
        #[arg(long)]
        show_ir: bool,
        #[arg(long)]
        emit_llvm: bool,
    },
    /// Format source files (basic pretty-printer)
    Fmt {
        files: Vec<PathBuf>,
        #[arg(long)]
        check: bool,
    },
    /// Generate Markdown docs from `///` comments (DESIGN §13)
    Doc {
        /// Source file (`.lm`)
        file: PathBuf,
        /// Write Markdown to this path instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Language server (stdio JSON-RPC)
    Lsp,
    /// Package manifest / lockfile helpers
    Pkg {
        #[command(subcommand)]
        cmd: PkgCmd,
    },
}

#[derive(Subcommand, Debug)]
enum PkgCmd {
    /// Write a starter `Lumia.toml` in the current directory
    Init {
        #[arg(long, default_value = "app")]
        name: String,
    },
    /// Resolve path deps and write `Lumia.lock`
    Lock {
        #[arg(long, default_value = "Lumia.toml")]
        manifest: PathBuf,
    },
    /// Add a path dependency to `Lumia.toml` and refresh the lockfile
    Add {
        name: String,
        #[arg(long)]
        path: String,
        #[arg(long, default_value = "Lumia.toml")]
        manifest: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Commands::Check {
            file,
            no_parallel,
            parallel: _,
            trust_foreign_pure,
        } => {
            let _ = check_file(&file, !no_parallel, trust_foreign_pure)?;
            println!("ok");
            Ok(())
        }
        Commands::Build {
            file,
            output,
            release,
            no_memo,
            no_parallel,
            parallel: _,
            trust_foreign_pure,
            link,
            show_ir,
            emit_llvm,
        } => {
            let out = output.unwrap_or_else(|| {
                file.file_stem()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("a.out"))
            });
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut validated_link = Vec::with_capacity(link.len());
            for a in &link {
                validated_link.push(pkg::validate_cli_link_arg(&cwd, a)?);
            }
            build_file(
                &file,
                &out,
                release,
                !no_memo,
                !no_parallel,
                trust_foreign_pure,
                validated_link,
                show_ir,
                emit_llvm,
            )?;
            println!("wrote {}", out.display());
            Ok(())
        }
        Commands::Fmt { files, check } => {
            for f in files {
                fmt_file(&f, check)?;
            }
            Ok(())
        }
        Commands::Doc { file, output } => {
            let md = doc::render_file(&file)?;
            if let Some(out) = output {
                fs::write(&out, &md).with_context(|| format!("write {}", out.display()))?;
                println!("wrote {}", out.display());
            } else {
                print!("{md}");
            }
            Ok(())
        }
        Commands::Lsp => lsp::run_lsp(),
        Commands::Pkg { cmd } => match cmd {
            PkgCmd::Init { name } => {
                let path = pkg::init_manifest(Path::new("."), &name)?;
                println!("wrote {}", path.display());
                Ok(())
            }
            PkgCmd::Lock { manifest } => {
                let m = pkg::load_manifest(&manifest)?;
                let lock = pkg::lock_from_manifest(&manifest, &m)?;
                let lock_path = manifest
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("Lumia.lock");
                pkg::write_lockfile(&lock_path, &lock)?;
                println!("wrote {}", lock_path.display());
                Ok(())
            }
            PkgCmd::Add {
                name,
                path,
                manifest,
            } => {
                pkg::add_path_dep(&manifest, &name, &path)?;
                let m = pkg::load_manifest(&manifest)?;
                let lock = pkg::lock_from_manifest(&manifest, &m)?;
                let lock_path = manifest
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("Lumia.lock");
                pkg::write_lockfile(&lock_path, &lock)?;
                println!("added `{name}` → {path}; wrote {}", lock_path.display());
                Ok(())
            }
        },
    }
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn diag_err(loaded: &LoadedProgram, span: Span, kind: &str, message: &str) -> anyhow::Error {
    let file = loaded.file(span.file);
    anyhow::anyhow!(format_diagnostic(
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

fn check_file(
    file: &Path,
    auto_parallel: bool,
    trust_foreign_pure: bool,
) -> Result<(lumia_ty::TypedModule, LoadedProgram)> {
    let loaded = load_program(file)?;
    let hir = lower_module(&loaded.module).map_err(|e| {
        diag_err(&loaded, e.span, "lower", &e.message)
    })?;
    let opts = InferOptions {
        trust_foreign_pure: trust_foreign_pure || loaded.trust_foreign_pure,
    };
    let mut typed = infer_module_with_options(&hir, loaded.visibility.clone(), opts)
        .map_err(|e| type_err(&loaded, e))?;
    finalize_auto_parallel(&mut typed, auto_parallel);
    check_effect_boundaries(&typed).map_err(|e| type_err(&loaded, e))?;
    Ok((typed, loaded))
}

fn build_file(
    file: &Path,
    output: &Path,
    release: bool,
    memo_tf: bool,
    auto_parallel: bool,
    trust_foreign_pure: bool,
    link_args: Vec<String>,
    show_ir: bool,
    emit_llvm: bool,
) -> Result<()> {
    let (mut typed, loaded) = check_file(file, auto_parallel, trust_foreign_pure)?;
    annotate_assert_messages(&mut typed.module, &loaded);
    let option_tags = option_ctor_tags(&typed.module.adts);
    let mut core = lower_hir(&typed.module, &typed.fun_types);
    optimize(
        &mut core,
        &OptOptions {
            release,
            memo_tf: release && memo_tf,
        },
    );
    if show_ir {
        print!("{}", format_module(&core));
    }

    ensure_runtime_built(release)?;

    let target_dir = workspace_target_dir();
    let runtime_lib = find_runtime_lib_prefer(&target_dir, release)?;

    let mut link = link_args;
    for a in &loaded.link_args {
        if !link.iter().any(|x| x == a) {
            link.push(a.clone());
        }
    }
    compile_module(
        &core,
        &CodegenOptions {
            release,
            output: output.to_path_buf(),
            runtime_lib,
            emit_ir: emit_llvm,
            option_some_tag: option_tags.0,
            option_none_tag: option_tags.1,
            // Parallel selection happens in `finalize_auto_parallel` before Core.
            parallel: auto_parallel,
            link_args: link,
        },
    )?;
    Ok(())
}

fn annotate_assert_messages(module: &mut lumia_hir::Module, loaded: &LoadedProgram) {
    for item in &mut module.items {
        match item {
            lumia_hir::Item::Fun(f) => annotate_assert_expr(&mut f.body, loaded),
            lumia_hir::Item::Val { body, .. } => annotate_assert_expr(body, loaded),
        }
    }
}

fn annotate_assert_expr(e: &mut lumia_hir::Expr, loaded: &LoadedProgram) {
    use lumia_hir::{Builtin, Expr};
    match e {
        Expr::BuiltinCall {
            name: Builtin::Assert,
            args,
            span,
        } => {
            for a in args.iter_mut() {
                annotate_assert_expr(a, loaded);
            }
            if args.len() == 1 {
                let file = loaded.file(span.file);
                let starts = lumia_syntax::line_starts(&file.src);
                let (line, _) = lumia_syntax::byte_to_line_col(&starts, span.start);
                let msg = format!("{}:{}: assert failed", path_label(&file.path), line);
                args.push(Expr::String(msg, *span));
            }
        }
        Expr::BuiltinCall { args, .. } | Expr::Call { args, .. } | Expr::AdtNew { args, .. } => {
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
        Expr::Assign { value, .. } => annotate_assert_expr(value, loaded),
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

fn option_ctor_tags(adts: &[lumia_hir::AdtDef]) -> (i64, i64) {
    for a in adts {
        if a.name == "Option" {
            let mut some = 0i64;
            let mut none = 1i64;
            for v in &a.variants {
                if v.name == "Some" {
                    some = v.tag;
                }
                if v.name == "None" {
                    none = v.tag;
                }
            }
            return (some, none);
        }
    }
    (0, 1)
}

fn workspace_target_dir() -> PathBuf {
    if let Ok(t) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(t);
    }
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for _ in 0..6 {
        let cand = dir.join("target");
        if cand.exists() {
            return cand;
        }
        if !dir.pop() {
            break;
        }
    }
    PathBuf::from("target")
}

fn ensure_runtime_built(release: bool) -> Result<()> {
    // Skip when the profile artifact already exists so parallel e2e tests do not
    // stampede `cargo build` (file-lock races / flaky failures on Windows).
    let target_dir = workspace_target_dir();
    let profile = if release { "release" } else { "debug" };
    let already = [
        target_dir.join(profile).join("liblumia_rt.a"),
        target_dir.join(profile).join("lumia_rt.lib"),
        target_dir.join(profile).join("lumia_rt.dll.lib"),
    ]
    .iter()
    .any(|p| p.exists());
    if already {
        return Ok(());
    }

    let mut cmd = Command::new("cargo");
    cmd.arg("build").arg("-p").arg("lumia_rt");
    if release {
        cmd.arg("--release");
    }
    let status = cmd
        .status()
        .context("spawn cargo build -p lumia_rt")?;
    if !status.success() {
        anyhow::bail!("failed to build lumia_rt");
    }
    Ok(())
}

fn fmt_file(path: &Path, check: bool) -> Result<()> {
    let src = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut m = parse_module(&src).map_err(|e| {
        anyhow::anyhow!(format_diagnostic(
            &path_label(path),
            &src,
            e.span,
            "parse",
            &e.message,
        ))
    })?;
    stamp_module(&mut m, 0);
    let formatted = lumia_syntax::format_module_src(&m);
    if check {
        if formatted.trim_end() != src.trim_end() {
            anyhow::bail!("{} would be reformatted", path.display());
        }
        println!("ok {}", path.display());
    } else {
        let out = if formatted.ends_with('\n') {
            formatted
        } else {
            format!("{formatted}\n")
        };
        fs::write(path, out).with_context(|| format!("write {}", path.display()))?;
        println!("formatted {}", path.display());
    }
    Ok(())
}
