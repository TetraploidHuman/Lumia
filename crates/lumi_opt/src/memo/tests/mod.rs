use super::*;
use crate::Pass;
use lumi_core::{Block, CoreFun, CoreModule, Local, MemoTf, Op, Value};
use lumi_hir::Builtin;
use lumi_syntax::{BinOp, UnOp};
use lumi_ty::{Effect, Type};

use rustc_hash::FxHashSet as HashSet;
fn bare_fun(name: &str, params: Vec<Local>, body: Block) -> CoreFun {
    let n = params.len();
    CoreFun {
        name: name.into(),
        params,
        param_names: (0..n).map(|i| format!("p{i}")).collect(),
        param_tys: vec![Type::Int; n],
        body,
        ret_ty: Type::Int,
        effect: Effect::pure(),
        is_main: false,
        memo: None,
        external: None,
        escaping: HashSet::default(),
        scheme_poly: false,
        mono_of: None,
    }
}

mod cse;
mod fold_adt;
mod fold_arith;
mod fold_iota;
mod fold_list;
mod fold_map_set;
mod licm;
#[cfg(feature = "opt-memo")]
mod memo_tf;
