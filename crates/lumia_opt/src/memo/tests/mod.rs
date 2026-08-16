use super::*;
use lumia_core::{Block, CoreFun, CoreModule, Local, MemoTf, Op, Value, FunKind};
use lumia_hir::Builtin;
use lumia_core::{CoreBinOp as BinOp, CoreUnOp as UnOp};
use lumia_ty::{Effect, Type};

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
        foreign_abi: lumia_core::ForeignAbi::C,
        escaping: HashSet::default(),
        scheme_poly: false,
        mono_of: None,
        kind: FunKind::Normal,
    }
}

mod cse;
mod fold_adt;
mod fold_arith;
mod fold_iota;
mod fold_list;
mod fold_map_set;
mod licm;
mod memo_tf;
