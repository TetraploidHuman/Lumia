//! Compiler caps / pass toggles from `Lumi.toml`, `.lumi/settings.toml`, and env.

use crate::caps::CapabilitySet;
use crate::pkg::{find_manifest, load_manifest};
use serde::Deserialize;
use std::path::Path;

/// Optional overrides in `Lumi.toml` `[compiler]` or `.lumi/settings.toml`.
///
/// `no_* = true` disables a capability or pass (same semantics as CLI `--no-*`).
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct CompilerConfig {
    #[serde(default)]
    pub no_parallel: Option<bool>,
    #[serde(default)]
    pub no_hof_fuse: Option<bool>,
    #[serde(default)]
    pub no_loop_sr: Option<bool>,
    #[serde(default)]
    pub no_tco: Option<bool>,
    #[serde(default)]
    pub no_nsw_iv: Option<bool>,
    #[serde(default)]
    pub no_inline: Option<bool>,
    #[serde(default)]
    pub no_dense_f64: Option<bool>,
    #[serde(default)]
    pub no_repr_select: Option<bool>,
    #[serde(default)]
    pub no_escape: Option<bool>,
}

/// CLI `--no-*` pass disables (codegen builds only).
#[derive(Debug, Clone, Default)]
pub struct PassDisables {
    pub no_inline: bool,
    pub no_dense_f64: bool,
    pub no_repr_select: bool,
    pub no_escape: bool,
}

/// CLI `--no-*` capability disables.
#[derive(Debug, Clone, Default)]
pub struct CapDisables {
    pub no_parallel: bool,
    pub no_hof_fuse: bool,
    pub no_loop_sr: bool,
    pub no_tco: bool,
    pub no_nsw_iv: bool,
}

impl CapDisables {
    pub fn apply_on_top(&self, mut caps: CapabilitySet) -> CapabilitySet {
        if self.no_parallel {
            caps = caps.with_auto_parallel(false);
        }
        if self.no_hof_fuse {
            caps = caps.with_hof_fuse(false);
        }
        if self.no_loop_sr {
            caps = caps.with_loop_sr(false);
        }
        if self.no_tco {
            caps = caps.with_tco(false);
        }
        if self.no_nsw_iv {
            caps = caps.with_nsw_iv(false);
        }
        caps
    }
}

/// Load `[compiler]` from the nearest `Lumi.toml` for `file`, then merge `.lumi/settings.toml`.
pub fn load_for_file(file: &Path) -> CompilerConfig {
    let mut cfg = CompilerConfig::default();
    if let Some(manifest_path) = find_manifest(file) {
        if let Ok(m) = load_manifest(&manifest_path) {
            cfg = merge_config(cfg, m.compiler);
        }
        if let Some(parent) = manifest_path.parent() {
            cfg = merge_config(cfg, load_dot_lumi_settings(parent));
        }
    }
    merge_env(&mut cfg);
    cfg
}

fn load_dot_lumi_settings(package_root: &Path) -> CompilerConfig {
    let path = package_root.join(".lumi").join("settings.toml");
    let Ok(src) = std::fs::read_to_string(&path) else {
        return CompilerConfig::default();
    };
    #[derive(Deserialize)]
    struct Wrapper {
        #[serde(default)]
        compiler: CompilerConfig,
    }
    toml::from_str(&src)
        .ok()
        .map(|w: Wrapper| w.compiler)
        .unwrap_or_default()
}

fn merge_config(mut base: CompilerConfig, overlay: CompilerConfig) -> CompilerConfig {
    macro_rules! merge_opt {
        ($field:ident) => {
            if overlay.$field.is_some() {
                base.$field = overlay.$field;
            }
        };
    }
    merge_opt!(no_parallel);
    merge_opt!(no_hof_fuse);
    merge_opt!(no_loop_sr);
    merge_opt!(no_tco);
    merge_opt!(no_nsw_iv);
    merge_opt!(no_inline);
    merge_opt!(no_dense_f64);
    merge_opt!(no_repr_select);
    merge_opt!(no_escape);
    base
}

fn merge_env(cfg: &mut CompilerConfig) {
    env_flag("LUMI_NO_PARALLEL", &mut cfg.no_parallel);
    env_flag("LUMI_NO_HOF_FUSE", &mut cfg.no_hof_fuse);
    env_flag("LUMI_NO_LOOP_SR", &mut cfg.no_loop_sr);
    env_flag("LUMI_NO_TCO", &mut cfg.no_tco);
    env_flag("LUMI_NO_NSW_IV", &mut cfg.no_nsw_iv);
    env_flag("LUMI_NO_INLINE", &mut cfg.no_inline);
    env_flag("LUMI_NO_DENSE_F64", &mut cfg.no_dense_f64);
    env_flag("LUMI_NO_REPR_SELECT", &mut cfg.no_repr_select);
    env_flag("LUMI_NO_ESCAPE", &mut cfg.no_escape);
}

fn env_flag(name: &str, slot: &mut Option<bool>) {
    match std::env::var(name) {
        Ok(v) if truthy(&v) => *slot = Some(true),
        Ok(v) if falsy(&v) => *slot = Some(false),
        Ok(_) => {}
        Err(_) => {}
    }
}

fn truthy(s: &str) -> bool {
    matches!(s, "1" | "true" | "yes" | "on")
}

fn falsy(s: &str) -> bool {
    matches!(s, "0" | "false" | "no" | "off")
}

/// Apply config + CLI disables onto stock caps.
pub fn caps_with_config(config: &CompilerConfig, cli: &CapDisables) -> CapabilitySet {
    let mut caps = CapabilitySet::stock();
    apply_cap_no(config.no_parallel, &mut caps, CapabilitySet::with_auto_parallel);
    apply_cap_no(config.no_hof_fuse, &mut caps, CapabilitySet::with_hof_fuse);
    apply_cap_no(config.no_loop_sr, &mut caps, CapabilitySet::with_loop_sr);
    apply_cap_no(config.no_tco, &mut caps, CapabilitySet::with_tco);
    apply_cap_no(config.no_nsw_iv, &mut caps, CapabilitySet::with_nsw_iv);
    if cli.no_parallel {
        caps = caps.with_auto_parallel(false);
    }
    if cli.no_hof_fuse {
        caps = caps.with_hof_fuse(false);
    }
    if cli.no_loop_sr {
        caps = caps.with_loop_sr(false);
    }
    if cli.no_tco {
        caps = caps.with_tco(false);
    }
    if cli.no_nsw_iv {
        caps = caps.with_nsw_iv(false);
    }
    caps
}

fn apply_cap_no(
    no: Option<bool>,
    caps: &mut CapabilitySet,
    f: fn(CapabilitySet, bool) -> CapabilitySet,
) {
    if no == Some(true) {
        *caps = f(caps.clone(), false);
    } else if no == Some(false) {
        *caps = f(caps.clone(), true);
    }
}

#[cfg(feature = "codegen")]
impl PassDisables {
    pub fn from_config_and_cli(config: &CompilerConfig, cli: &PassDisables) -> Self {
        Self {
            no_inline: config.no_inline == Some(true) || cli.no_inline,
            no_dense_f64: config.no_dense_f64 == Some(true) || cli.no_dense_f64,
            no_repr_select: config.no_repr_select == Some(true) || cli.no_repr_select,
            no_escape: config.no_escape == Some(true) || cli.no_escape,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn dot_lumi_settings_merge() {
        let dir = std::env::temp_dir().join(format!("lumi_cfg_{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".lumi"));
        let settings = dir.join(".lumi/settings.toml");
        let mut f = std::fs::File::create(&settings).unwrap();
        write!(f, "[compiler]\nno_parallel = true\n").unwrap();
        let cfg = load_dot_lumi_settings(&dir);
        assert_eq!(cfg.no_parallel, Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caps_config_and_cli_cli_wins() {
        let mut cfg = CompilerConfig::default();
        cfg.no_parallel = Some(false);
        let caps = caps_with_config(
            &cfg,
            &CapDisables {
                no_parallel: true,
                ..Default::default()
            },
        );
        assert!(!caps.auto_parallel);
    }
}
