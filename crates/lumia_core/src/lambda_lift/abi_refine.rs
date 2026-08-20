//! ABI refinement that historically lived under `lambda_lift`.
//!
//! True lift is [`super::rewrite`]/captures` / `heap`]. Channel payload hints and
//! post-mono float-capture fixups are mid-end ABI contracts — not lifting.
//! Prefer this module at call sites so the package boundary stays honest.

pub(crate) use super::abi_refresh::fixup_closure_float_caps;
pub(crate) use super::channel_hint::refine_channel_elem_hint;
