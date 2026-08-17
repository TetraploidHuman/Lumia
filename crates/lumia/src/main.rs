//! Lumia CLI — thin front-end over the `lumia` library.

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
#[cfg(not(feature = "codegen"))]
use lumia::check::check_program;
use lumia::load::path_label;
use lumia::pkg;
use lumia::{doc, lsp};
use lumia_syntax::{format_diagnostic, format_matches_source, parse_module, stamp_module};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(name = "lumia", version, about = "Lumia compiler")]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

/// Flags shared by `check` and `build` (keep clap help / defaults in sync).
#[derive(Args, Debug, Clone)]
struct SharedCheckArgs {
    /// Disable auto-parallel `List.map` (default: on when safe; DESIGN §11.1).
    #[arg(long = "no-parallel")]
    no_parallel: bool,
    /// Trust `foreign "C" pure` (FFI purity is not verified). Overrides package.
    #[arg(long = "trust-foreign-pure", conflicts_with = "no_trust_foreign_pure")]
    trust_foreign_pure: bool,
    /// Refuse `foreign "C" pure` even if `package.trust_foreign_pure` is set.
    #[arg(long = "no-trust-foreign-pure", conflicts_with = "trust_foreign_pure")]
    no_trust_foreign_pure: bool,
}

impl SharedCheckArgs {
    /// `Some` = CLI override; `None` = honor `Lumia.toml` / default false.
    fn trust_foreign_pure_override(&self) -> Option<bool> {
        if self.no_trust_foreign_pure {
            Some(false)
        } else if self.trust_foreign_pure {
            Some(true)
        } else {
            None
        }
    }
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Type- and effect-check only
    Check {
        file: PathBuf,
        #[command(flatten)]
        shared: SharedCheckArgs,
    },
    /// Compile to a native executable (requires a codegen-enabled binary)
    #[cfg(feature = "codegen")]
    Build {
        file: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        release: bool,
        /// Disable transparent Memo `T_f` even in `--release` (for benchmarks).
        #[arg(long = "no-memo", alias = "no-memo-l2")]
        no_memo: bool,
        #[command(flatten)]
        shared: SharedCheckArgs,
        /// Disable dense `List[Float]` → `lumia_f64_*` strength reduction (bench baseline).
        #[arg(long = "no-dense-f64-sr")]
        no_dense_f64_sr: bool,
        /// Extra linker args (repeatable), e.g. `--link -lm --link -L/opt/lib`.
        #[arg(long = "link", value_name = "ARG")]
        link: Vec<String>,
        #[arg(long)]
        show_ir: bool,
        #[arg(long)]
        emit_llvm: bool,
    },
    /// Build then run (same flags as `build`; program args after `--`)
    #[cfg(feature = "codegen")]
    Run {
        file: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        release: bool,
        #[arg(long = "no-memo", alias = "no-memo-l2")]
        no_memo: bool,
        #[command(flatten)]
        shared: SharedCheckArgs,
        #[arg(long = "no-dense-f64-sr")]
        no_dense_f64_sr: bool,
        #[arg(long = "link", value_name = "ARG")]
        link: Vec<String>,
        /// Arguments forwarded to the program (place after `--`).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
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
        Commands::Check { file, shared } => {
            #[cfg(feature = "codegen")]
            lumia::build::check_file(
                &file,
                &lumia::build::CompileOptions {
                    auto_parallel: !shared.no_parallel,
                    trust_foreign_pure: shared.trust_foreign_pure_override(),
                    ..lumia::build::CompileOptions::default()
                },
            )?;
            #[cfg(not(feature = "codegen"))]
            {
                let _ = check_program(
                    &file,
                    !shared.no_parallel,
                    shared.trust_foreign_pure_override(),
                )?;
            }
            println!("ok");
            Ok(())
        }
        #[cfg(feature = "codegen")]
        Commands::Build {
            file,
            output,
            release,
            no_memo,
            shared,
            no_dense_f64_sr,
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
            lumia::build::build_file(
                &file,
                &out,
                &lumia::build::CompileOptions {
                    release,
                    memo_tf: !no_memo,
                    auto_parallel: !shared.no_parallel,
                    dense_f64_sr: !no_dense_f64_sr,
                    trust_foreign_pure: shared.trust_foreign_pure_override(),
                    link_args: validated_link,
                    show_ir,
                    emit_llvm,
                },
            )?;
            println!("wrote {}", out.display());
            Ok(())
        }
        #[cfg(feature = "codegen")]
        Commands::Run {
            file,
            output,
            release,
            no_memo,
            shared,
            no_dense_f64_sr,
            link,
            args,
        } => {
            let keep_bin = output.is_some();
            let out = output.unwrap_or_else(|| {
                let stem = file
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "a.out".into());
                std::env::temp_dir().join(format!("lumia_run_{stem}_{}", std::process::id()))
            });
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut validated_link = Vec::with_capacity(link.len());
            for a in &link {
                validated_link.push(pkg::validate_cli_link_arg(&cwd, a)?);
            }
            lumia::build::build_file(
                &file,
                &out,
                &lumia::build::CompileOptions {
                    release,
                    memo_tf: !no_memo,
                    auto_parallel: !shared.no_parallel,
                    dense_f64_sr: !no_dense_f64_sr,
                    trust_foreign_pure: shared.trust_foreign_pure_override(),
                    link_args: validated_link,
                    show_ir: false,
                    emit_llvm: false,
                },
            )?;
            let status = std::process::Command::new(&out)
                .args(&args)
                .status()
                .with_context(|| format!("run {}", out.display()))?;
            if !keep_bin {
                let _ = fs::remove_file(&out);
            }
            if let Some(code) = status.code() {
                if code != 0 {
                    std::process::exit(code);
                }
            } else {
                anyhow::bail!("program terminated by signal ({status})");
            }
            Ok(())
        }
        Commands::Fmt { files, check } => {
            if files.is_empty() {
                anyhow::bail!("lumia fmt: no files specified (pass one or more `.lm` paths)");
            }
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
        if !format_matches_source(&src, &formatted) {
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
