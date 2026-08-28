//! Unified compile profile: [`CapabilitySet`] + mid-end [`PassSet`] + build knobs.

use crate::caps::{CapPhase, CapabilitySet, INVENTORY as CAP_INVENTORY};
use crate::compiler_config::{caps_with_config, CapDisables, CompilerConfig, PassDisables};
use std::fmt::Write;
use std::path::Path;

#[cfg(feature = "codegen")]
use crate::build::BuildOptions;
#[cfg(feature = "codegen")]
use lumi_core::CoreModule;
#[cfg(feature = "codegen")]
use lumi_opt::{
    optimize, optimize_with, pass_info, pass_names_for, validate_pass_set, OptOptions, OptProfile,
    PassSet, ALL_PASSES,
};

/// Full compiler profile for embedders and CLI (`build` / `check`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileProfile {
    pub caps: CapabilitySet,
    pub release: bool,
    /// Transparent Memo `T_f` in Release (ignored when `release` is false).
    pub memo_tf: bool,
    pub trust_foreign_pure: bool,
    pub emit_ir: bool,
    pub link_args: Vec<String>,
    /// Print Memo hit/miss stats at exit (`--show-memo-stats` / `LUMI_MEMO_STATS`).
    pub show_memo_stats: bool,
    /// Prefer DenseInt Memo `T_f` over slot tables when planning (§7.5.3).
    pub memo_prefer_dense: bool,
    /// Memory manager: 0 = mark-sweep (default), 1 = arc stub (`--mm arc`).
    pub mm_mode: i64,
    /// Print GC stats at exit (`--show-gc-stats` / `LUMI_GC_STATS`).
    pub show_gc_stats: bool,
    #[cfg(feature = "codegen")]
    pass_set: PassSet,
}

impl Default for CompileProfile {
    fn default() -> Self {
        Self::stock(false)
    }
}

impl CompileProfile {
    /// Stock caps + stock PassSet for Debug or Release.
    pub fn stock(release: bool) -> Self {
        Self {
            caps: CapabilitySet::stock(),
            release,
            memo_tf: release,
            trust_foreign_pure: false,
            emit_ir: false,
            link_args: Vec::new(),
            show_memo_stats: false,
            memo_prefer_dense: true,
            mm_mode: 0,
            show_gc_stats: false,
            #[cfg(feature = "codegen")]
            pass_set: PassSet::for_profile(OptProfile::from_release(release)),
        }
    }

    #[cfg(feature = "codegen")]
    #[deprecated(
        since = "0.3.2",
        note = "use `CompileProfile::assemble` or `with_caps` on `CompileProfile::stock`"
    )]
    pub fn from_build_options(opts: &BuildOptions, caps: CapabilitySet) -> Self {
        let mut p = Self::stock(opts.release);
        p.caps = caps;
        p.memo_tf = opts.memo_tf;
        p.trust_foreign_pure = opts.trust_foreign_pure;
        p.emit_ir = opts.emit_ir;
        p.link_args = opts.link_args.clone();
        p.show_memo_stats = opts.show_memo_stats;
        p
    }

    pub fn with_caps(mut self, caps: CapabilitySet) -> Self {
        self.caps = caps;
        self
    }

    pub fn with_memo_tf(mut self, on: bool) -> Self {
        self.memo_tf = on;
        self
    }

    pub fn with_trust_foreign_pure(mut self, on: bool) -> Self {
        self.trust_foreign_pure = on;
        self
    }

    pub fn with_emit_ir(mut self, on: bool) -> Self {
        self.emit_ir = on;
        self
    }

    pub fn with_link_args(mut self, args: Vec<String>) -> Self {
        self.link_args = args;
        self
    }

    pub fn with_show_memo_stats(mut self, on: bool) -> Self {
        self.show_memo_stats = on;
        self
    }

    pub fn with_memo_prefer_dense(mut self, on: bool) -> Self {
        self.memo_prefer_dense = on;
        self
    }

    pub fn with_mm_mode(mut self, mode: i64) -> Self {
        self.mm_mode = mode;
        self
    }

    pub fn with_show_gc_stats(mut self, on: bool) -> Self {
        self.show_gc_stats = on;
        self
    }

    #[cfg(feature = "codegen")]
    pub fn without_pass(mut self, id: &str) -> Self {
        self.pass_set = self.pass_set.without(id);
        self
    }

    #[cfg(feature = "codegen")]
    pub fn with_pass(mut self, id: &'static str) -> Self {
        self.pass_set = self.pass_set.with(id);
        self
    }

    #[cfg(feature = "codegen")]
    pub fn pass_set(&self) -> &PassSet {
        &self.pass_set
    }

    #[cfg(feature = "codegen")]
    pub fn opt_profile(&self) -> OptProfile {
        OptProfile::from_release(self.release)
    }

    #[cfg(feature = "codegen")]
    pub fn to_build_options(&self) -> BuildOptions {
        BuildOptions {
            release: self.release,
            memo_tf: self.memo_tf,
            trust_foreign_pure: self.trust_foreign_pure,
            emit_ir: self.emit_ir,
            link_args: self.link_args.clone(),
            show_memo_stats: self.show_memo_stats,
        }
    }

    /// Enabled Phase C capability ids (not CoreOpt passes).
    pub fn capability_ids(&self) -> Vec<&'static str> {
        self.caps.enabled_ids()
    }

    #[cfg(feature = "codegen")]
    pub fn pass_names(&self) -> Vec<&'static str> {
        pass_names_for(self.opt_profile(), &self.pass_set)
    }

    #[cfg(feature = "codegen")]
    pub fn optimize_core(&self, core: &mut CoreModule) -> Result<(), String> {
        let profile = self.opt_profile();
        let memo = self.release && self.memo_tf;
        if self.pass_set.is_stock(profile) {
            optimize(
                core,
                &OptOptions {
                    release: self.release,
                    memo_tf: memo,
                    memo_prefer_dense: self.memo_prefer_dense,
                },
            );
            Ok(())
        } else {
            optimize_with(
                core,
                profile,
                &self.pass_set,
                memo,
                self.memo_prefer_dense,
            )
        }
    }

    #[cfg(feature = "codegen")]
    pub fn validate_passes(&self) -> Result<(), String> {
        validate_pass_set(&self.pass_set)
    }

    /// Default profile for LSP / IDE analysis (Debug stock caps, no codegen).
    pub fn for_lsp() -> Self {
        Self::stock(false)
    }

    /// LSP / check path: nearest `Lumi.toml` + `.lumi/settings.toml` + env.
    pub fn for_lsp_at(file: &Path) -> Self {
        Self::for_lsp_with_overlay(file, &CompilerConfig::default())
    }

    /// LSP with IDE `initializationOptions` / workspace settings overlay.
    pub fn for_lsp_with_overlay(file: &Path, lsp_overlay: &CompilerConfig) -> Self {
        let config = crate::compiler_config::load_for_lsp(file, lsp_overlay);
        Self::assemble(
            false,
            true,
            false,
            false,
            Vec::new(),
            &config,
            &CapDisables::default(),
            &PassDisables::default(),
        )
        .unwrap_or_else(|e| panic!("invalid LSP profile for {}: {e}", file.display()))
    }

    /// Merge manifest / env / CLI into a full profile.
    pub fn assemble(
        release: bool,
        memo_tf: bool,
        trust_foreign_pure: bool,
        emit_ir: bool,
        link_args: Vec<String>,
        config: &CompilerConfig,
        cap_cli: &CapDisables,
        pass_cli: &PassDisables,
    ) -> Result<Self, String> {
        let caps = caps_with_config(config, cap_cli);
        let p = Self::stock(release)
            .with_caps(caps)
            .with_memo_tf(memo_tf)
            .with_trust_foreign_pure(trust_foreign_pure)
            .with_emit_ir(emit_ir)
            .with_link_args(link_args);
        #[cfg(feature = "codegen")]
        {
            let passes = PassDisables::from_config_and_cli(config, pass_cli);
            return p.apply_pass_disables(&passes);
        }
        #[cfg(not(feature = "codegen"))]
        {
            let _ = pass_cli;
            Ok(p)
        }
    }

    /// Apply mid-end pass `--no-*` disables (validates requires edges).
    #[cfg(feature = "codegen")]
    pub fn apply_pass_disables(mut self, d: &PassDisables) -> Result<Self, String> {
        if d.no_inline {
            self = self.without_pass("inline");
        }
        if d.no_dense_f64 {
            self = self.without_pass("dense_f64_sr");
        }
        if d.no_repr_select {
            self = self.without_pass("repr_select");
        }
        if d.no_escape {
            self = self.without_pass("escape").without_pass("repr_select");
        }
        self.validate_passes()?;
        Ok(self)
    }

    /// Enabled CoreOpt pass ids from the current PassSet.
    #[cfg(feature = "codegen")]
    pub fn enabled_pass_ids(&self) -> Vec<&'static str> {
        ALL_PASSES
            .iter()
            .filter(|p| self.pass_set.contains(p.id))
            .map(|p| p.id)
            .collect()
    }

    /// Map Phase C caps onto [`lumi_core::PipelineOptions`] for test/tooling frontends.
    pub fn to_pipeline_options(&self) -> lumi_core::PipelineOptions {
        lumi_core::PipelineOptions {
            lower: self.caps.to_lower_options(),
            typecheck: self
                .caps
                .to_typecheck_options(self.trust_foreign_pure),
        }
    }

    /// Human-readable capability inventory (Phase C + CoreOpt catalog).
    pub fn format_list_caps(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "Phase C capabilities (runtime toggles):");
        for c in CAP_INVENTORY {
            if matches!(
                c.phase,
                CapPhase::HirLower | CapPhase::Typecheck | CapPhase::Codegen
            ) {
                let on = self.caps.contains(c.id);
                let mark = if on { 'x' } else { ' ' };
                let _ = writeln!(out, "  [{mark}] {} — {}", c.id, c.summary);
            }
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "CoreOpt passes (compile-time / PassSet; see --list-passes):");
        for c in CAP_INVENTORY {
            if c.phase == CapPhase::CoreOpt {
                let _ = writeln!(out, "  [catalog] {} — {}", c.id, c.summary);
            }
        }
        out
    }

    /// Human-readable pass enablement for the current profile.
    #[cfg(feature = "codegen")]
    pub fn format_list_passes(&self) -> String {
        let mut out = String::new();
        let profile = self.opt_profile();
        let _ = writeln!(
            out,
            "Mid-end passes (profile={:?}, stock={}):",
            profile,
            self.pass_set.is_stock(profile)
        );
        for info in ALL_PASSES {
            let on = self.pass_set.contains(info.id);
            let mark = if on { 'x' } else { ' ' };
            let summary = pass_info(info.id)
                .map(|p| format!("{:?} @ {:?}", p.kind, p.stage))
                .unwrap_or_default();
            let _ = writeln!(out, "  [{mark}] {} {summary}", info.id);
        }
        let names = self.pass_names();
        let _ = writeln!(out);
        let _ = writeln!(out, "Schedule order ({}):", names.len());
        for (i, n) in names.iter().enumerate() {
            let _ = writeln!(out, "  {}. {n}", i + 1);
        }
        out
    }
}

/// Build [`CapabilitySet`] from CLI `--no-*` negation flags (defaults = all on).
#[deprecated(
    since = "0.3.2",
    note = "use `compiler_config::caps_with_config` or `CompileProfile::assemble`"
)]
pub fn caps_from_cli(
    no_parallel: bool,
    no_hof_fuse: bool,
    no_loop_sr: bool,
    no_tco: bool,
    no_nsw_iv: bool,
) -> CapabilitySet {
    CapabilitySet::stock()
        .with_auto_parallel(!no_parallel)
        .with_hof_fuse(!no_hof_fuse)
        .with_loop_sr(!no_loop_sr)
        .with_tco(!no_tco)
        .with_nsw_iv(!no_nsw_iv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_profile_has_stock_pass_set() {
        #[cfg(feature = "codegen")]
        {
            let p = CompileProfile::stock(true);
            assert!(p.pass_set().is_stock(OptProfile::Release));
            assert!(p.validate_passes().is_ok());
        }
    }

    #[test]
    #[allow(deprecated)]
    fn caps_from_cli_respects_flags() {
        let caps = caps_from_cli(true, true, false, true, false);
        assert!(!caps.auto_parallel && !caps.hof_fuse);
        assert!(caps.loop_sr && !caps.tco && caps.nsw_iv);
    }

    #[test]
    fn list_caps_mentions_phase_c() {
        let text = CompileProfile::stock(false).format_list_caps();
        assert!(text.contains("hof_fuse"));
        assert!(text.contains("memo_tf"));
    }

    #[cfg(feature = "codegen")]
    #[test]
    fn without_pass_validates_or_fails_predictably() {
        let p = CompileProfile::stock(true).without_pass("repr_select");
        assert!(p.validate_passes().is_ok());
        let bad = CompileProfile::stock(true)
            .without_pass("escape")
            .without_pass("repr_select");
        // repr_select off alone would fail requires; both off is ok.
        assert!(bad.validate_passes().is_ok());
    }
}
