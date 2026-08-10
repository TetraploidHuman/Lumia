//! Lumia CLI — thin front-end over the `lumia` library.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use lumia::check::check_program;
use lumia::pkg;
use lumia::{doc, lsp};
use lumia_syntax::{format_diagnostic, parse_module, stamp_module};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(not(feature = "codegen"))]
use anyhow::bail;
#[cfg(feature = "codegen")]
use lumia::check::annotate_assert_messages;
#[cfg(feature = "codegen")]
use lumia_codegen::{compile_module, find_runtime_lib_prefer, CodegenOptions};
#[cfg(feature = "codegen")]
use lumia_core::{format_module, lower_hir_with_schemes};
#[cfg(feature = "codegen")]
use lumia_opt::{optimize, OptOptions};
#[cfg(feature = "codegen")]
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
    Lsp {
        /// Accepted for VS Code / Cursor clients (`TransportKind.stdio` adds this).
        #[arg(long = "stdio", hide = true)]
        _stdio: bool,
    },
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
            trust_foreign_pure,
        } => {
            let _ = check_program(&file, !no_parallel, trust_foreign_pure)?;
            println!("ok");
            Ok(())
        }
        Commands::Build {
            file,
            output,
            release,
            no_memo,
            no_parallel,
            trust_foreign_pure,
            link,
            show_ir,
            emit_llvm,
        } => {
            #[cfg(not(feature = "codegen"))]
            {
                let _ = (
                    file,
                    output,
                    release,
                    no_memo,
                    no_parallel,
                    trust_foreign_pure,
                    link,
                    show_ir,
                    emit_llvm,
                );
                bail!(
                    "`lumia build` needs a codegen-enabled binary \
                     (install via ./scripts/install.sh, or cargo build -p lumia)"
                );
            }
            #[cfg(feature = "codegen")]
            {
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
        Commands::Lsp { .. } => lsp::run_lsp(),
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

#[cfg(feature = "codegen")]
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
    let (mut typed, loaded) = check_program(file, auto_parallel, trust_foreign_pure)?;
    annotate_assert_messages(&mut typed.module, &loaded);
    let option_tags = option_ctor_tags(&typed.module.adts);
    let mut core = lower_hir_with_schemes(&typed.module, &typed.fun_types, &typed.fun_schemes);
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
            parallel: auto_parallel,
            link_args: link,
        },
    )?;
    Ok(())
}

#[cfg(feature = "codegen")]
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

/// Workspace root that contains this compiler (`…/Lumia`), baked in at build time.
/// Used so `lumia build` works outside the repo (e.g. `~/文档`) without hunting cwd.
#[cfg(feature = "codegen")]
fn compiler_workspace_root() -> PathBuf {
    lumia_abi::workspace_root(env!("CARGO_MANIFEST_DIR"))
}

#[cfg(feature = "codegen")]
fn workspace_target_dir() -> PathBuf {
    if let Ok(t) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(t);
    }
    compiler_workspace_root().join("target")
}

#[cfg(feature = "codegen")]
fn ensure_runtime_built(release: bool) -> Result<()> {
    let root = compiler_workspace_root();
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&root);
    cmd.arg("build").arg("-p").arg("lumia_rt");
    if release {
        cmd.arg("--release");
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn cargo build -p lumia_rt in {}", root.display()))?;
    if !status.success() {
        anyhow::bail!("failed to build lumia_rt");
    }
    Ok(())
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.display().to_string())
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
