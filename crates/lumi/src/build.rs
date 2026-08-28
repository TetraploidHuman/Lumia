//! Check → lower → opt → codegen pipeline for embedders and tests.

#[cfg(feature = "codegen")]
use crate::profile::CompileProfile;
#[cfg(feature = "codegen")]
use crate::caps::CapabilitySet;
#[cfg(feature = "codegen")]
use crate::check::{annotate_assert_messages, check_program_with_profile};
#[cfg(feature = "codegen")]
use crate::load::LoadedProgram;
#[cfg(feature = "codegen")]
use anyhow::{Context, Result};
#[cfg(feature = "codegen")]
use lumi_codegen::{compile_module, find_runtime_lib_prefer, CodegenOptions};
#[cfg(feature = "codegen")]
use lumi_core::{lower_hir_with_schemes, CoreModule};
#[cfg(feature = "codegen")]
use lumi_hir::AdtDef;
#[cfg(feature = "codegen")]
use std::path::{Path, PathBuf};
#[cfg(feature = "codegen")]
use std::process::Command;

/// Options for [`compile_with_caps`] (mirrors CLI `build` knobs).
#[cfg(feature = "codegen")]
#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub release: bool,
    pub memo_tf: bool,
    pub trust_foreign_pure: bool,
    pub emit_ir: bool,
    pub link_args: Vec<String>,
    pub show_memo_stats: bool,
}

#[cfg(feature = "codegen")]
impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            release: false,
            memo_tf: true,
            trust_foreign_pure: false,
            emit_ir: false,
            link_args: Vec::new(),
            show_memo_stats: false,
        }
    }
}

/// Optimized Core module plus link metadata from the frontend.
#[cfg(feature = "codegen")]
pub struct PreparedProgram {
    pub core: CoreModule,
    pub loaded: LoadedProgram,
    pub option_some_tag: i64,
    pub option_none_tag: i64,
}

/// Typecheck, lower, and optimize — stops before codegen (for `--show-ir`).
#[cfg(feature = "codegen")]
pub fn prepare_with_profile(file: &Path, profile: &CompileProfile) -> Result<PreparedProgram> {
    let (mut typed, loaded) = check_program_with_profile(file, profile)?;
    annotate_assert_messages(&mut typed.module, &loaded);
    let option_tags = option_ctor_tags(&typed.module.adts);
    let mut core = lower_hir_with_schemes(&typed.module, &typed.fun_types, &typed.fun_schemes);
    profile
        .optimize_core(&mut core)
        .map_err(|e| anyhow::anyhow!("optimize: {e}"))?;
    Ok(PreparedProgram {
        core,
        loaded,
        option_some_tag: option_tags.0,
        option_none_tag: option_tags.1,
    })
}

/// Full compile with an explicit [`CompileProfile`].
#[cfg(feature = "codegen")]
pub fn compile_with_profile(
    file: &Path,
    output: &Path,
    profile: &CompileProfile,
) -> Result<()> {
    let prepared = prepare_with_profile(file, profile)?;
    compile_prepared(&prepared, output, profile)
}

/// Typecheck, lower, and optimize with a [`CapabilitySet`] (legacy helper).
#[cfg(feature = "codegen")]
#[deprecated(
    since = "0.3.2",
    note = "use `prepare_with_profile` and `CompileProfile` instead"
)]
pub fn prepare_with_caps(
    file: &Path,
    caps: &CapabilitySet,
    opts: &BuildOptions,
) -> Result<PreparedProgram> {
    prepare_with_profile(file, &CompileProfile::from_build_options(opts, caps.clone()))
}

/// Full compile with caps + build options (legacy helper).
#[cfg(feature = "codegen")]
#[deprecated(
    since = "0.3.2",
    note = "use `compile_with_profile` and `CompileProfile` instead"
)]
pub fn compile_with_caps(
    file: &Path,
    output: &Path,
    caps: &CapabilitySet,
    opts: &BuildOptions,
) -> Result<()> {
    compile_with_profile(
        file,
        output,
        &CompileProfile::from_build_options(opts, caps.clone()),
    )
}

/// Codegen + link for an already-optimized module.
#[cfg(feature = "codegen")]
pub fn compile_prepared(
    prepared: &PreparedProgram,
    output: &Path,
    profile: &CompileProfile,
) -> Result<()> {
    ensure_runtime_built(profile.release)?;

    let target_dir = workspace_target_dir();
    let runtime_lib = find_runtime_lib_prefer(&target_dir, profile.release)?;

    let mut link = profile.link_args.clone();
    for a in &prepared.loaded.link_args {
        if !link.iter().any(|x| x == a) {
            link.push(a.clone());
        }
    }
    let mut cg_opts = CodegenOptions {
        release: profile.release,
        output: output.to_path_buf(),
        emit_ir: profile.emit_ir,
        runtime_lib,
        option_some_tag: prepared.option_some_tag,
        option_none_tag: prepared.option_none_tag,
        loop_sr: false,
        tco: false,
        nsw_iv: false,
        link_args: link,
        show_memo_stats: profile.show_memo_stats,
        show_gc_stats: profile.show_gc_stats,
        mm_mode: profile.mm_mode,
    };
    profile.caps.apply_codegen(&mut cg_opts);
    compile_module(&prepared.core, &cg_opts)
}

#[cfg(feature = "codegen")]
fn option_ctor_tags(adts: &[AdtDef]) -> (i64, i64) {
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

#[cfg(feature = "codegen")]
fn compiler_workspace_root() -> PathBuf {
    lumi_abi::workspace_root(env!("CARGO_MANIFEST_DIR"))
}

#[cfg(feature = "codegen")]
fn workspace_target_dir() -> PathBuf {
    if let Ok(t) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(t);
    }
    compiler_workspace_root().join("target")
}

#[cfg(feature = "codegen")]
fn runtime_build_stamp(release: bool) -> PathBuf {
    let feats: Vec<&str> = vec![
        #[cfg(feature = "opt-memo")]
        "opt-memo",
        #[cfg(feature = "opt-dense-f64")]
        "opt-dense-f64",
    ];
    let profile = if release { "release" } else { "debug" };
    let feat_key = if feats.is_empty() {
        "none".to_string()
    } else {
        feats.join("+")
    };
    workspace_target_dir().join(format!(".lumi_rt_built_{profile}_{feat_key}"))
}

/// Fingerprint `lumi_rt` sources + manifest so stamp invalidates on edits.
#[cfg(feature = "codegen")]
fn lumi_rt_source_fingerprint(root: &Path) -> Result<String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let rt_root = root.join("crates/lumi_rt");
    let mut hasher = DefaultHasher::new();
    let mut paths = vec![rt_root.join("Cargo.toml")];
    if let Ok(rd) = std::fs::read_dir(rt_root.join("src")) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.extension().is_some_and(|e| e == "rs") {
                paths.push(p);
            }
        }
    }
    paths.sort();
    for p in paths {
        p.to_string_lossy().hash(&mut hasher);
        let meta = std::fs::metadata(&p)
            .with_context(|| format!("stat {}", p.display()))?;
        if let Ok(modified) = meta.modified() {
            if let Ok(d) = modified.duration_since(SystemTime::UNIX_EPOCH) {
                d.as_nanos().hash(&mut hasher);
            }
        }
        meta.len().hash(&mut hasher);
    }
    Ok(format!("{:016x}", hasher.finish()))
}

#[cfg(feature = "codegen")]
fn runtime_stamp_is_fresh(stamp_path: &Path, fingerprint: &str) -> bool {
    std::fs::read_to_string(stamp_path)
        .ok()
        .is_some_and(|s| s.trim() == fingerprint)
}

#[cfg(feature = "codegen")]
fn write_runtime_stamp(stamp_path: &Path, fingerprint: &str) -> Result<()> {
    if let Some(parent) = stamp_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(stamp_path, fingerprint)
        .with_context(|| format!("write {}", stamp_path.display()))?;
    Ok(())
}

#[cfg(feature = "codegen")]
fn ensure_runtime_built(release: bool) -> Result<()> {
    let root = compiler_workspace_root();
    let fingerprint = lumi_rt_source_fingerprint(&root)?;
    let stamp = runtime_build_stamp(release);
    if runtime_stamp_is_fresh(&stamp, &fingerprint) {
        return Ok(());
    }
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&root);
    cmd.arg("build")
        .arg("-p")
        .arg("lumi_rt")
        .arg("--no-default-features");
    let feats: &[&str] = &[
        #[cfg(feature = "opt-memo")]
        "opt-memo",
        #[cfg(feature = "opt-dense-f64")]
        "opt-dense-f64",
    ];
    if !feats.is_empty() {
        cmd.arg("--features").arg(feats.join(","));
    }
    if release {
        cmd.arg("--release");
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn cargo build -p lumi_rt in {}", root.display()))?;
    if !status.success() {
        anyhow::bail!("failed to build lumi_rt");
    }
    write_runtime_stamp(&stamp, &fingerprint)?;
    Ok(())
}
