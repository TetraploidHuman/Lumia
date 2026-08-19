//! Whole-function `floatOrbitChecksum` → RT `lumia_float_orbit_checksum`.

use super::externs::RtArg;
use lumia_core::{
    first_direct_loop, match_float_orbit_shape, result_is_slot, same_local, slot_init_const,
    CoreFun, OrbitBound, Value,
};
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

pub(super) fn is_float_orbit_fun(name: &str) -> bool {
    name == "floatOrbitChecksum" || name.starts_with("floatOrbitChecksum$c_")
}

/// `floatOrbitChecksum(n, iters)` logistic-orbit Int checksum.
///
/// Also matches fully const-specialized `$c_` clones (`params` empty). Inner `iters`
/// must be a compile-time const (same constraint as the former codegen loop SR).
pub(super) fn match_float_orbit_checksum_fun(
    fun: &CoreFun,
    defs: &HashMap<u32, Value>,
) -> Option<Vec<RtArg>> {
    if !is_float_orbit_fun(&fun.name) || fun.ret_ty != Type::Int {
        return None;
    }
    let specialized = fun.params.is_empty();
    if !specialized && fun.params.len() != 2 {
        return None;
    }
    if !slot_init_const(&fun.body, "h", 0, defs) || !slot_init_const(&fun.body, "i", 0, defs) {
        return None;
    }
    let (header, body, latch) = first_direct_loop(&fun.body)?;
    let shape = match_float_orbit_shape(header, body, latch, defs)?;
    if shape.h != "h" || shape.i != "i" {
        return None;
    }
    if !result_is_slot(&fun.body, "h", defs) {
        return None;
    }
    let n_arg = match shape.n {
        OrbitBound::Const(c) => RtArg::Const(c),
        OrbitBound::Local(l) if !specialized => {
            if same_local(l, fun.params[0], defs) {
                RtArg::Param(0)
            } else {
                return None;
            }
        }
        OrbitBound::Local(_) => return None,
    };
    let iters_arg = if specialized {
        RtArg::Const(shape.iters)
    } else if fun.params.len() == 2 {
        // Shape match requires const inner bound; param case only when specialize
        // folded iters into the header (clone) — otherwise bail.
        RtArg::Const(shape.iters)
    } else {
        return None;
    };
    Some(vec![n_arg, iters_arg])
}
