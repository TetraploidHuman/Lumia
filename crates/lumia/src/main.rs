mod load;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use load::load_program;
use lumia_codegen::{compile_module, find_runtime_lib, CodegenOptions};
use lumia_core::{format_module, lower_hir};
use lumia_hir::lower_module;
use lumia_opt::{optimize, OptOptions};
use lumia_ty::{check_effect_boundaries, infer_module};
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
    },
    /// Compile to a native executable
    Build {
        file: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        release: bool,
        #[arg(long)]
        show_ir: bool,
        #[arg(long)]
        emit_llvm: bool,
    },
    /// Format (stub: rewrite unchanged for now)
    Fmt {
        files: Vec<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Commands::Check { file } => {
            let _ = check_file(&file)?;
            println!("ok");
            Ok(())
        }
        Commands::Build {
            file,
            output,
            release,
            show_ir,
            emit_llvm,
        } => {
            let out = output.unwrap_or_else(|| {
                file.file_stem()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("a.out"))
            });
            build_file(&file, &out, release, show_ir, emit_llvm)?;
            println!("wrote {}", out.display());
            Ok(())
        }
        Commands::Fmt { files } => {
            for f in files {
                println!("fmt: {} (no-op stub)", f.display());
            }
            Ok(())
        }
    }
}

fn check_file(file: &Path) -> Result<lumia_ty::TypedModule> {
    let ast = load_program(file)?;
    let hir = lower_module(&ast).map_err(|e| anyhow::anyhow!("lower: {e}"))?;
    let typed = infer_module(&hir).map_err(|e| anyhow::anyhow!("type: {e}"))?;
    check_effect_boundaries(&typed).map_err(|e| anyhow::anyhow!("effect: {e}"))?;
    Ok(typed)
}

fn build_file(
    file: &Path,
    output: &Path,
    release: bool,
    show_ir: bool,
    emit_llvm: bool,
) -> Result<()> {
    let typed = check_file(file)?;
    let option_tags = option_ctor_tags(&typed.module.adts);
    let mut core = lower_hir(&typed.module, &typed.fun_types);
    optimize(
        &mut core,
        &OptOptions {
            release,
        },
    );
    if show_ir {
        print!("{}", format_module(&core));
    }

    ensure_runtime_built(release)?;

    let target_dir = workspace_target_dir();
    let runtime_lib = find_runtime_lib(&target_dir)?;

    compile_module(
        &core,
        &CodegenOptions {
            release,
            output: output.to_path_buf(),
            emit_ir: emit_llvm,
            runtime_lib,
            option_some_tag: option_tags.0,
            option_none_tag: option_tags.1,
        },
    )?;
    Ok(())
}

fn option_ctor_tags(adts: &[lumia_hir::AdtDef]) -> (i64, i64) {
    for a in adts {
        if a.name == "Option" {
            let some = a
                .variants
                .iter()
                .find(|v| v.name == "Some")
                .map(|v| v.tag)
                .unwrap_or(0);
            let none = a
                .variants
                .iter()
                .find(|v| v.name == "None")
                .map(|v| v.tag)
                .unwrap_or(1);
            return (some, none);
        }
    }
    (0, 1)
}

fn workspace_target_dir() -> PathBuf {
    if let Ok(t) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(t);
    }
    // Walk up from current exe or cwd
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
    let mut cmd = Command::new("cargo");
    cmd.arg("build").arg("-p").arg("lumia_rt");
    if release {
        cmd.arg("--release");
    }
    let status = cmd.status().context("cargo build lumia_rt")?;
    if !status.success() {
        anyhow::bail!("failed to build lumia_rt");
    }
    Ok(())
}
