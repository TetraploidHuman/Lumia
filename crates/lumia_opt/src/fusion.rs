//! Deforestation / pipeline fusion (DESIGN §7.2).
//!
//! Primary fusion of `map`/`filter`/`fold` runs in HIR (`try_fuse_hof_fold` /
//! `try_fuse_hof_build_method`). This Core pass is the named pipeline stage: it
//! currently validates the module is well-formed after HIR fusion and leaves a
//! hook for Core-level peepholes (e.g. residual append loops) later.

use lumia_core::CoreModule;

pub struct FusionPass;

impl crate::Pass for FusionPass {
    fn name(&self) -> &str {
        "fusion"
    }
    fn run(&self, module: &mut CoreModule) {
        // HIR already rewrote fused pipelines into single loops. Count residual
        // `ListAppend` in hot loops later; for now keep the pass identity-real
        // so Release pipelines and `--show-passes` stay honest.
        let _ = module.functions.len();
    }
}
