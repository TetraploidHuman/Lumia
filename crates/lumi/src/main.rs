//! Lumi CLI — thin front-end over the `lumi` library.

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use lumi::check::check_program_with_profile;
use lumi::load::path_label;
use lumi::pkg;
use lumi::compiler_config::{load_for_file, CapDisables, PassDisables};
use lumi::profile::CompileProfile;
use lumi::{doc, lsp};
use lumi_syntax::{format_diagnostic, parse_module, stamp_module};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(not(feature = "codegen"))]
use anyhow::bail;
#[cfg(feature = "codegen")]
use lumi::build::{compile_prepared, prepare_with_profile};
#[cfg(feature = "codegen")]
use lumi_core::format_module;

/// Phase C capability toggles (CLI `--no-*`; default = all on).
#[derive(Args, Debug, Clone, Default)]
struct CapFlags {
    /// Disable auto-parallel `List.map` / `List.fold` (DESIGN §11.1).
    #[arg(long = "no-parallel")]
    no_parallel: bool,
    /// Disable HIR map/filter/fold deforestation.
    #[arg(long = "no-hof-fuse")]
    no_hof_fuse: bool,
    /// Disable codegen loop pattern SR (collatz / number theory / …).
    #[arg(long = "no-loop-sr")]
    no_loop_sr: bool,
    /// Disable musttail SCC tail-call optimization.
    #[arg(long = "no-tco")]
    no_tco: bool,
    /// Disable proven-safe NSW arithmetic annotations.
    #[arg(long = "no-nsw-iv")]
    no_nsw_iv: bool,
}

impl CapFlags {
    fn to_disables(&self) -> CapDisables {
        CapDisables {
            no_parallel: self.no_parallel,
            no_hof_fuse: self.no_hof_fuse,
            no_loop_sr: self.no_loop_sr,
            no_tco: self.no_tco,
            no_nsw_iv: self.no_nsw_iv,
        }
    }
}

/// Mid-end pass toggles (CLI `--no-*`; default = all on).
#[derive(Args, Debug, Clone, Default)]
struct PassFlags {
    /// Disable small pure-function inlining (`opt-inline`).
    #[arg(long = "no-inline")]
    no_inline: bool,
    /// Disable dense `List[Float]` fast path (`opt-dense-f64`).
    #[arg(long = "no-dense-f64")]
    no_dense_f64: bool,
    /// Disable escape + Lit* representation select (`opt-repr-stack`).
    #[arg(long = "no-repr-select")]
    no_repr_select: bool,
    /// Disable escape analysis (also drops `repr_select`).
    #[arg(long = "no-escape")]
    no_escape: bool,
}

impl PassFlags {
    #[cfg(feature = "codegen")]
    fn to_disables(&self) -> PassDisables {
        PassDisables {
            no_inline: self.no_inline,
            no_dense_f64: self.no_dense_f64,
            no_repr_select: self.no_repr_select,
            no_escape: self.no_escape,
        }
    }
}

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
        #[command(flatten)]
        caps: CapFlags,
        /// Trust `foreign "C" pure` (FFI purity is not verified).
        #[arg(long = "trust-foreign-pure")]
        trust_foreign_pure: bool,
        /// List Phase C capabilities (and CoreOpt catalog), then exit.
        #[arg(long = "list-caps")]
        list_caps: bool,
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
        #[command(flatten)]
        caps: CapFlags,
        #[command(flatten)]
        passes: PassFlags,
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
        /// Print Memo `T_f` hit/miss stats to stderr at exit.
        #[arg(long = "show-memo-stats")]
        show_memo_stats: bool,
        /// Use slot-based Memo `T_f` only (disable DenseInt tables).
        #[arg(long = "no-memo-dense")]
        no_memo_dense: bool,
        /// Memory manager: `ms` (mark-sweep, default) or `arc` (eager COW free-on-zero).
        #[arg(long = "mm", default_value = "ms")]
        mm: String,
        /// Print GC collection stats to stderr at exit.
        #[arg(long = "show-gc-stats")]
        show_gc_stats: bool,
        /// List Phase C capabilities (and CoreOpt catalog), then exit.
        #[arg(long = "list-caps")]
        list_caps: bool,
        /// List mid-end pass enablement for this profile, then exit.
        #[arg(long = "list-passes")]
        list_passes: bool,
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
            caps,
            trust_foreign_pure,
            list_caps,
        } => {
            let config = load_for_file(&file);
            let profile = CompileProfile::assemble(
                false,
                true,
                trust_foreign_pure,
                false,
                Vec::new(),
                &config,
                &caps.to_disables(),
                &PassDisables::default(),
            )
            .map_err(|e| anyhow::anyhow!("profile: {e}"))?;
            if list_caps {
                print!("{}", profile.format_list_caps());
                return Ok(());
            }
            let _ = check_program_with_profile(&file, &profile)?;
            println!("ok");
            Ok(())
        }
        Commands::Build {
            file,
            output,
            release,
            no_memo,
            caps,
            passes,
            trust_foreign_pure,
            link,
            show_ir,
            emit_llvm,
            show_memo_stats,
            no_memo_dense,
            mm,
            show_gc_stats,
            list_caps,
            list_passes,
        } => {
            #[cfg(not(feature = "codegen"))]
            {
                let _ = (
                    file,
                    output,
                    release,
                    no_memo,
                    caps,
                    passes,
                    trust_foreign_pure,
                    link,
                    show_ir,
                    emit_llvm,
                    show_memo_stats,
                    no_memo_dense,
                    mm,
                    show_gc_stats,
                    list_caps,
                    list_passes,
                );
                bail!(
                    "`lumi build` needs a codegen-enabled binary \
                     (install via ./scripts/install.sh, or cargo build -p lumi)"
                );
            }
            #[cfg(feature = "codegen")]
            {
                let profile = build_profile(
                    &file,
                    release,
                    !no_memo,
                    caps,
                    passes,
                    trust_foreign_pure,
                    emit_llvm,
                    show_memo_stats,
                    no_memo_dense,
                    mm,
                    show_gc_stats,
                    link,
                )?;
                if list_caps {
                    print!("{}", profile.format_list_caps());
                    return Ok(());
                }
                if list_passes {
                    print!("{}", profile.format_list_passes());
                    return Ok(());
                }
                let out = output.unwrap_or_else(|| {
                    file.file_stem()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("a.out"))
                });
                build_file(&file, &out, &profile, show_ir)?;
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
fn build_profile(
    file: &Path,
    release: bool,
    memo_tf: bool,
    caps: CapFlags,
    passes: PassFlags,
    trust_foreign_pure: bool,
    emit_ir: bool,
    show_memo_stats: bool,
    no_memo_dense: bool,
    mm: String,
    show_gc_stats: bool,
    link: Vec<String>,
) -> Result<CompileProfile> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut validated_link = Vec::with_capacity(link.len());
    for a in &link {
        validated_link.push(pkg::validate_cli_link_arg(&cwd, a)?);
    }
    let config = load_for_file(file);
    let prefer_dense = config.prefer_dense_memo.unwrap_or(true) && !no_memo_dense;
    let mm_mode = parse_mm_mode(&mm)?;
    CompileProfile::assemble(
        release,
        memo_tf,
        trust_foreign_pure,
        emit_ir,
        validated_link,
        &config,
        &caps.to_disables(),
        &passes.to_disables(),
    )
    .map(|p| {
        p.with_show_memo_stats(show_memo_stats)
            .with_show_gc_stats(show_gc_stats)
            .with_memo_prefer_dense(prefer_dense)
            .with_mm_mode(mm_mode)
    })
    .map_err(|e| anyhow::anyhow!("profile: {e}"))
}

#[cfg(feature = "codegen")]
fn parse_mm_mode(s: &str) -> Result<i64> {
    match s.trim().to_ascii_lowercase().as_str() {
        "ms" | "marksweep" | "mark-sweep" => Ok(0),
        "arc" | "refcount" => Ok(1),
        other => anyhow::bail!("unknown --mm mode `{other}` (expected ms or arc)"),
    }
}

#[cfg(feature = "codegen")]
fn build_file(file: &Path, output: &Path, profile: &CompileProfile, show_ir: bool) -> Result<()> {
    let prepared = prepare_with_profile(file, profile)?;
    if show_ir {
        print!("{}", format_module(&prepared.core));
    }
    compile_prepared(&prepared, output, profile)
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
