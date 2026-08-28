//! Pass stages, schedules, and PassSet assembly (§7.1 / Phase A).
//!
//! Hot path stays a static `PipelinePass` slice — no `Box<dyn Pass>`.
//! [`PassSet`] filters a profile schedule; [`validate_pass_set`] enforces
//! hard requires (e.g. `repr_select` ⇒ `escape`).

use crate::copy_elim::CopyElimPass;
use crate::dense_f64_sr::DenseF64SrPass;
use crate::escape::EscapePass;
use crate::fusion::ConcatIdentPass;
use crate::inline::InlinePass;
use crate::memo::{cse_module, ConstFoldPass, LicmPass};
use crate::registry;
use crate::repr_select::ReprSelect;
use crate::specialize_const::SpecializeConstPass;
use crate::Pass;
use lumi_core::CoreModule;

/// Pipeline slot — fixed order contract between producers and consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PassStage {
    /// Transparent Memo planning (before CSE; not a `PipelinePass`).
    Plan,
    /// Local reuse: CSE / const-fold.
    LocalReuse,
    /// Call-site const specialization + fold.
    Specialize,
    /// Loop-invariant code motion.
    Loop,
    /// Whole-function pattern → RT kernels (e.g. dense f64).
    PatternSr,
    /// Size-reducing transforms (inline).
    Size,
    /// Escape analysis → `CoreFun::escaping`.
    Escape,
    /// Mid cleanup that does not allocate (concat ident / fold).
    CleanupMid,
    /// Representation selection → `Alloc*.repr`.
    Repr,
    /// Late cleanup (copy-elim).
    CleanupLate,
}

/// What a pass primarily does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassKind {
    /// Fills IR annotations only (or primarily).
    Analysis,
    /// Rewrites Core ops / functions.
    Transform,
    /// Plans work applied outside the pass loop (e.g. `memo_tf`).
    Plan,
}

/// Optional Core annotations that passes read/write.
///
/// Missing annotation ⇒ consumers must take the §7.1.1 default path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrAnno {
    /// `CoreFun::escaping`
    Escaping,
    /// `CoreFun::memo`
    Memo,
    /// `Alloc* { repr }`
    Repr,
    /// Function bodies / call graph shape
    FunBodies,
}

/// Compile-time contract for one optimization capability.
#[derive(Debug, Clone, Copy)]
pub struct PassInfo {
    pub id: &'static str,
    pub kind: PassKind,
    pub stage: PassStage,
    /// Pass ids that must be enabled whenever this one is.
    pub requires: &'static [&'static str],
    pub writes: &'static [IrAnno],
    pub reads: &'static [IrAnno],
}

/// Named profile → fixed schedule (may repeat passes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptProfile {
    Debug,
    Release,
}

impl OptProfile {
    pub fn from_release(release: bool) -> Self {
        if release {
            Self::Release
        } else {
            Self::Debug
        }
    }
}

/// Zero-cost dispatcher for one schedule slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PipelinePass {
    Cse,
    ConstFold,
    SpecializeConst,
    Licm,
    Escape,
    DenseF64Sr,
    Inline,
    ConcatIdent,
    ReprSelect,
    CopyElim,
}

impl PipelinePass {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Cse => "cse",
            Self::ConstFold => "const_fold",
            Self::SpecializeConst => "specialize_const",
            Self::Licm => "licm",
            Self::Escape => "escape",
            Self::DenseF64Sr => "dense_f64_sr",
            Self::Inline => "inline",
            Self::ConcatIdent => "concat_ident",
            Self::ReprSelect => "repr_select",
            Self::CopyElim => "copy_elim",
        }
    }

    pub(crate) fn run(self, module: &mut CoreModule) {
        match self {
            Self::Cse => CsePass.run(module),
            Self::ConstFold => ConstFoldPass.run(module),
            Self::SpecializeConst => SpecializeConstPass.run(module),
            Self::Licm => LicmPass.run(module),
            Self::Escape => EscapePass.run(module),
            Self::DenseF64Sr => DenseF64SrPass.run(module),
            Self::Inline => InlinePass.run(module),
            Self::ConcatIdent => ConcatIdentPass.run(module),
            Self::ReprSelect => ReprSelect.run(module),
            Self::CopyElim => CopyElimPass.run(module),
        }
    }
}

struct CsePass;
impl Pass for CsePass {
    fn name(&self) -> &str {
        "cse"
    }
    fn run(&self, module: &mut CoreModule) {
        cse_module(module);
    }
}

/// Debug schedule — exact historical order.
pub(crate) const DEBUG_SCHEDULE: &[PipelinePass] = &[
    PipelinePass::Cse,
    PipelinePass::ConstFold,
    // Light PE without Inline/memo — bake Int/Bool/Char into leaf clones.
    PipelinePass::SpecializeConst,
    PipelinePass::ConstFold,
    PipelinePass::Licm,
    // Same dense-float SR as Release so Debug matches hot RT kernels (no Inline).
    PipelinePass::DenseF64Sr,
    PipelinePass::Escape,
    PipelinePass::ReprSelect,
];

/// Release schedule — exact historical order.
pub(crate) const RELEASE_SCHEDULE: &[PipelinePass] = &[
    PipelinePass::Cse,
    PipelinePass::ConstFold,
    // Bake Int/Bool/Char call-site constants into leaf clones before inline/PE.
    PipelinePass::SpecializeConst,
    PipelinePass::ConstFold,
    PipelinePass::Licm,
    PipelinePass::DenseF64Sr,
    PipelinePass::Inline,
    // Inlined nests / composed helpers — second SR before fold/specialize.
    PipelinePass::DenseF64Sr,
    // Inline exposes fresh literals / builtins — fold, specialize, then escape.
    PipelinePass::ConstFold,
    PipelinePass::SpecializeConst,
    PipelinePass::ConstFold,
    PipelinePass::Escape,
    PipelinePass::ConcatIdent,
    PipelinePass::ConstFold,
    PipelinePass::ReprSelect,
    PipelinePass::CopyElim,
];

/// Which pass ids are enabled. Filtering a profile schedule drops disabled slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassSet {
    enabled: Vec<&'static str>,
}

impl PassSet {
    fn from_ids(mut ids: Vec<&'static str>) -> Self {
        ids.sort_unstable();
        ids.dedup();
        Self { enabled: ids }
    }

    /// Every builtin pass id (including `memo_tf`).
    pub fn all() -> Self {
        Self::from_ids(registry::ALL.iter().map(|p| p.id).collect())
    }

    /// Default enabled set for a profile (matches today's Debug/Release content).
    pub fn for_profile(profile: OptProfile) -> Self {
        let mut ids: Vec<&'static str> = match profile {
            OptProfile::Debug => DEBUG_SCHEDULE.iter().map(|p| p.name()).collect(),
            OptProfile::Release => RELEASE_SCHEDULE.iter().map(|p| p.name()).collect(),
        };
        if matches!(profile, OptProfile::Release) {
            ids.push("memo_tf");
        }
        Self::from_ids(ids)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.enabled.binary_search_by(|&e| e.cmp(id)).is_ok()
    }

    pub fn without(mut self, id: &str) -> Self {
        self.enabled.retain(|&e| e != id);
        self
    }

    pub fn with(self, id: &'static str) -> Self {
        let mut ids = self.enabled;
        ids.push(id);
        Self::from_ids(ids)
    }

    pub fn ids(&self) -> &[&'static str] {
        &self.enabled
    }

    /// True when this set matches the stock profile enablement (hot path).
    pub fn is_stock(&self, profile: OptProfile) -> bool {
        *self == Self::for_profile(profile)
    }
}

/// Validate hard `requires` edges from the registry.
pub fn validate_pass_set(set: &PassSet) -> Result<(), String> {
    for info in registry::ALL {
        if !set.contains(info.id) {
            continue;
        }
        for &req in info.requires {
            if !set.contains(req) {
                return Err(format!(
                    "pass `{}` requires `{}`, but it is disabled",
                    info.id, req
                ));
            }
        }
    }
    for &id in set.ids() {
        if registry::info(id).is_none() {
            return Err(format!("unknown pass id `{id}`"));
        }
    }
    Ok(())
}

pub(crate) fn schedule_for(profile: OptProfile) -> &'static [PipelinePass] {
    match profile {
        OptProfile::Debug => DEBUG_SCHEDULE,
        OptProfile::Release => RELEASE_SCHEDULE,
    }
}

/// Build the run list for `profile`, filtered by `set` (allocating).
pub(crate) fn build_schedule(
    profile: OptProfile,
    set: &PassSet,
) -> Result<Vec<PipelinePass>, String> {
    validate_pass_set(set)?;
    Ok(schedule_for(profile)
        .iter()
        .copied()
        .filter(|p| set.contains(p.name()))
        .collect())
}

pub(crate) fn run_schedule(module: &mut CoreModule, passes: &[PipelinePass]) {
    for p in passes {
        p.run(module);
    }
}

/// Diagnostic name list: schedule order, then `memo_tf` if enabled.
pub fn pass_names_for(profile: OptProfile, set: &PassSet) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = schedule_for(profile)
        .iter()
        .filter(|p| set.contains(p.name()))
        .map(|p| p.name())
        .collect();
    if set.contains("memo_tf") {
        names.push("memo_tf");
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_requires_are_known() {
        for info in registry::ALL {
            for &req in info.requires {
                assert!(
                    registry::info(req).is_some(),
                    "{} requires unknown {}",
                    info.id,
                    req
                );
            }
        }
    }

    #[test]
    fn stock_sets_validate() {
        validate_pass_set(&PassSet::for_profile(OptProfile::Debug)).unwrap();
        validate_pass_set(&PassSet::for_profile(OptProfile::Release)).unwrap();
        validate_pass_set(&PassSet::all()).unwrap();
    }

    #[test]
    fn repr_select_requires_escape() {
        let set = PassSet::for_profile(OptProfile::Release).without("escape");
        let err = validate_pass_set(&set).expect_err("must fail");
        assert!(err.contains("repr_select"), "{err}");
        assert!(err.contains("escape"), "{err}");
    }

    #[test]
    fn disable_escape_and_repr_is_ok() {
        let set = PassSet::for_profile(OptProfile::Release)
            .without("escape")
            .without("repr_select");
        validate_pass_set(&set).unwrap();
        let sched = build_schedule(OptProfile::Release, &set).unwrap();
        assert!(!sched.iter().any(|p| p.name() == "escape"));
        assert!(!sched.iter().any(|p| p.name() == "repr_select"));
    }

    #[test]
    fn escape_before_repr_in_both_schedules() {
        for sched in [DEBUG_SCHEDULE, RELEASE_SCHEDULE] {
            let escape_i = sched.iter().position(|p| *p == PipelinePass::Escape);
            let repr_i = sched.iter().position(|p| *p == PipelinePass::ReprSelect);
            assert!(escape_i.is_some() && repr_i.is_some());
            assert!(escape_i.unwrap() < repr_i.unwrap());
        }
    }

    #[test]
    fn stages_match_registry() {
        for p in RELEASE_SCHEDULE {
            let info = registry::info(p.name()).expect(p.name());
            match p {
                PipelinePass::Escape => assert_eq!(info.stage, PassStage::Escape),
                PipelinePass::ReprSelect => assert_eq!(info.stage, PassStage::Repr),
                PipelinePass::Inline => assert_eq!(info.stage, PassStage::Size),
                PipelinePass::CopyElim => assert_eq!(info.stage, PassStage::CleanupLate),
                PipelinePass::Licm => assert_eq!(info.stage, PassStage::Loop),
                PipelinePass::DenseF64Sr => assert_eq!(info.stage, PassStage::PatternSr),
                PipelinePass::ConcatIdent => assert_eq!(info.stage, PassStage::CleanupMid),
                PipelinePass::Cse => assert_eq!(info.stage, PassStage::LocalReuse),
                PipelinePass::ConstFold => assert_eq!(info.stage, PassStage::LocalReuse),
                PipelinePass::SpecializeConst => assert_eq!(info.stage, PassStage::Specialize),
            }
        }
        assert_eq!(registry::MEMO_TF.stage, PassStage::Plan);
    }

    #[test]
    fn schedule_ids_are_registered_and_covered() {
        for sched in [DEBUG_SCHEDULE, RELEASE_SCHEDULE] {
            for p in sched {
                assert!(
                    registry::info(p.name()).is_some(),
                    "schedule pass `{}` missing from registry",
                    p.name()
                );
            }
        }
        let mut scheduled = std::collections::BTreeSet::new();
        for sched in [DEBUG_SCHEDULE, RELEASE_SCHEDULE] {
            for p in sched {
                scheduled.insert(p.name());
            }
        }
        scheduled.insert("memo_tf");
        for info in registry::ALL {
            assert!(
                scheduled.contains(info.id),
                "registry pass `{}` never appears in Debug/Release schedule (or memo_tf)",
                info.id
            );
        }
    }

    #[test]
    fn all_equals_release_enablement() {
        assert_eq!(PassSet::all(), PassSet::for_profile(OptProfile::Release));
    }

    #[test]
    fn with_without_roundtrip_keeps_stock() {
        let stock = PassSet::for_profile(OptProfile::Release);
        let round = stock.clone().without("inline").with("inline");
        assert!(round.is_stock(OptProfile::Release));
    }

    #[test]
    fn custom_set_without_memo_disables_even_if_flag() {
        let set = PassSet::for_profile(OptProfile::Release).without("memo_tf");
        assert!(!set.contains("memo_tf"));
        assert!(!set.is_stock(OptProfile::Release));
        // Mirrors optimize_with gate: flag alone is not enough once set is custom.
        let do_memo = true && (set.contains("memo_tf") || set.is_stock(OptProfile::Release));
        assert!(!do_memo);
    }
}
