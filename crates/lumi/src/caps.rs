//! Cross-crate optimization capabilities (Phase C).
//!
//! CoreOpt passes stay in [`lumi_opt`] registry/PassSet. This module inventories
//! HirLower / Typecheck / Codegen capabilities and maps a [`CapabilitySet`] onto
//! each crate's options so `build` / `check` enable them in one place.

use lumi_codegen::CodegenOptions;
use lumi_hir::LowerOptions;
use lumi_ty::TypecheckOptions;

/// Pipeline phase that hosts a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapPhase {
    HirLower,
    Typecheck,
    /// Mid-end PassSet (see `lumi_opt`); listed for a unified inventory.
    CoreOpt,
    Codegen,
}

/// Compile-time description of one cross-cutting optimization capability.
#[derive(Debug, Clone, Copy)]
pub struct CapInfo {
    pub id: &'static str,
    pub phase: CapPhase,
    /// Short role for tooling / docs.
    pub summary: &'static str,
}

/// Runtime enablement for Phase C capabilities (defaults = today's behavior).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilitySet {
    /// HIR map/filter/fold deforestation (`hof_fuse`).
    pub hof_fuse: bool,
    /// FunRef-safe `ListParMap` / assoc `ListParFold` (`--no-parallel`).
    pub auto_parallel: bool,
    /// Codegen Loop SR (collatz / number theory / float / …).
    pub loop_sr: bool,
    /// Musttail SCC TCO.
    pub tco: bool,
    /// NSW / proven-safe arithmetic annotations.
    pub nsw_iv: bool,
}

impl Default for CapabilitySet {
    fn default() -> Self {
        Self::stock()
    }
}

impl CapabilitySet {
    /// Product defaults: everything on (matches pre–Phase C behavior).
    pub fn stock() -> Self {
        Self {
            hof_fuse: true,
            auto_parallel: true,
            loop_sr: true,
            tco: true,
            nsw_iv: true,
        }
    }

    pub fn with_auto_parallel(mut self, on: bool) -> Self {
        self.auto_parallel = on;
        self
    }

    pub fn with_hof_fuse(mut self, on: bool) -> Self {
        self.hof_fuse = on;
        self
    }

    pub fn with_loop_sr(mut self, on: bool) -> Self {
        self.loop_sr = on;
        self
    }

    pub fn with_tco(mut self, on: bool) -> Self {
        self.tco = on;
        self
    }

    pub fn with_nsw_iv(mut self, on: bool) -> Self {
        self.nsw_iv = on;
        self
    }

    pub fn contains(&self, id: &str) -> bool {
        match id {
            "hof_fuse" => self.hof_fuse,
            "auto_parallel" => self.auto_parallel,
            "loop_sr" => self.loop_sr,
            "tco" => self.tco,
            "nsw_iv" => self.nsw_iv,
            _ => false,
        }
    }

    pub fn enabled_ids(&self) -> Vec<&'static str> {
        INVENTORY
            .iter()
            .filter(|c| self.contains(c.id))
            .map(|c| c.id)
            .collect()
    }

    pub fn to_lower_options(&self) -> LowerOptions {
        LowerOptions {
            hof_fuse: self.hof_fuse,
        }
    }

    pub fn to_typecheck_options(&self, trust_foreign_pure: bool) -> TypecheckOptions {
        TypecheckOptions {
            auto_parallel: self.auto_parallel,
            trust_foreign_pure,
        }
    }

    /// Overlay Phase C flags onto an existing [`CodegenOptions`] skeleton.
    pub fn apply_codegen(&self, opts: &mut CodegenOptions) {
        opts.parallel = self.auto_parallel;
        opts.loop_sr = self.loop_sr;
        opts.tco = self.tco;
        opts.nsw_iv = self.nsw_iv;
    }
}

/// Builtin Phase C inventory (+ CoreOpt pointers for a single catalog).
pub const INVENTORY: &[CapInfo] = &[
    CapInfo {
        id: "hof_fuse",
        phase: CapPhase::HirLower,
        summary: "map/filter/fold deforestation in HIR lower",
    },
    CapInfo {
        id: "auto_parallel",
        phase: CapPhase::Typecheck,
        summary: "ListParMap / ListParFold selection (CLI --no-parallel)",
    },
    CapInfo {
        id: "loop_sr",
        phase: CapPhase::Codegen,
        summary: "whole-loop pattern → RT / fast-path emit",
    },
    CapInfo {
        id: "tco",
        phase: CapPhase::Codegen,
        summary: "musttail SCC tail-call optimization",
    },
    CapInfo {
        id: "nsw_iv",
        phase: CapPhase::Codegen,
        summary: "proven-safe NSW arithmetic annotations",
    },
    // CoreOpt (stage A/B) — documented here; enabled via lumi_opt PassSet / opt-* features.
    CapInfo {
        id: "memo_tf",
        phase: CapPhase::CoreOpt,
        summary: "transparent Memo T_f (lumi_opt / opt-memo)",
    },
    CapInfo {
        id: "dense_f64_sr",
        phase: CapPhase::CoreOpt,
        summary: "dense List[Float] → lumi_f64_* (opt-dense-f64)",
    },
    CapInfo {
        id: "inline",
        phase: CapPhase::CoreOpt,
        summary: "small pure leaf inlining (opt-inline)",
    },
    CapInfo {
        id: "repr_select",
        phase: CapPhase::CoreOpt,
        summary: "escape + Lit* representation select (opt-repr-stack)",
    },
];

pub fn cap_info(id: &str) -> Option<&'static CapInfo> {
    INVENTORY.iter().find(|c| c.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_enables_phase_c_caps() {
        let caps = CapabilitySet::stock();
        for id in ["hof_fuse", "auto_parallel", "loop_sr", "tco", "nsw_iv"] {
            assert!(caps.contains(id), "{id}");
        }
    }

    #[test]
    fn no_parallel_only_clears_auto_parallel() {
        let caps = CapabilitySet::stock().with_auto_parallel(false);
        assert!(!caps.auto_parallel);
        assert!(caps.hof_fuse && caps.loop_sr && caps.tco && caps.nsw_iv);
    }

    #[test]
    fn inventory_ids_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for c in INVENTORY {
            assert!(seen.insert(c.id), "duplicate {}", c.id);
        }
    }
}
