//! Lumi CLI — thin front-end over the `lumi` library.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use lumi::check::check_program;
use lumi::load::path_label;
use lumi::pkg;
use lumi::{doc, lsp};
use lumi_syntax::{format_diagnostic, parse_module, stamp_module};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(not(feature = "codegen"))]
use anyhow::bail;
#[cfg(feature = "codegen")]
use lumi::build::{compile_prepared, prepare_with_caps, BuildOptions};
#[cfg(feature = "codegen")]
use lumi::caps::CapabilitySet;
#[cfg(feature = "codegen")]
use lumi_core::format_module;

#[derive(Parser, Debug)]
#[command(name = "lumi", version, about = "Lumi compiler")]
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
    /// Write a starter `Lumi.toml` in the current directory
    Init {
        #[arg(long, default_value = "app")]
        name: String,
    },
    /// Resolve path deps and write `Lumi.lock`
    Lock {
        #[arg(long, default_value = "Lumi.toml")]
        manifest: PathBuf,
    },
    /// Add a path dependency to `Lumi.toml` and refresh the lockfile
    Add {
        name: String,
        #[arg(long)]
        path: String,
        #[arg(long, default_value = "Lumi.toml")]
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
                    "`lumi build` needs a codegen-enabled binary \
                     (install via ./scripts/install.sh, or cargo build -p lumi)"
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
                    .join("Lumi.lock");
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
                    .join("Lumi.lock");
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
    let caps = CapabilitySet::stock().with_auto_parallel(auto_parallel);
    let build_opts = BuildOptions {
        release,
        memo_tf,
        trust_foreign_pure,
        emit_ir: emit_llvm,
        link_args,
    };
    let prepared = prepare_with_caps(file, &caps, &build_opts)?;
    if show_ir {
        print!("{}", format_module(&prepared.core));
    }
    compile_prepared(&prepared, output, &caps, &build_opts)?;
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
    let formatted = lumi_syntax::format_module_src(&m);
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
