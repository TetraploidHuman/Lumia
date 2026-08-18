//! Local const-fold + copy-prop (DESIGN §7.5.1-A).

use lumia_core::{for_each_ctrl_nested_block_mut, Block, Local, Op, Value};
use lumia_core::{CoreBinOp as BinOp, CoreUnOp as UnOp};
use lumia_hir::Builtin;
use rustc_hash::FxHashMap as HashMap;

use super::cse::rewrite_value;

mod adt;
mod helpers;
mod iota;
mod list;
mod map_set;

pub(crate) struct FoldEnv {
    known_int: crate::ir_util::KnownScalars,
    /// IEEE bits for known Float locals (so ±0 map/set keys can be compacted).
    known_float: HashMap<u32, u64>,
    /// Known String literal contents (for setOf/mapOf key compact).
    known_string: HashMap<u32, String>,
    known_list: HashMap<u32, Vec<Local>>,
    known_adt: HashMap<u32, Vec<Local>>,
    known_adt_tag: HashMap<u32, i64>,
    known_map: HashMap<u32, Vec<Local>>,
    known_set: HashMap<u32, Vec<Local>>,
    known_iota: HashMap<u32, (i64, i64)>,
}

impl FoldEnv {
    fn new() -> Self {
        Self {
            known_int: crate::ir_util::KnownScalars::new(),
            known_float: HashMap::default(),
            known_string: HashMap::default(),
            known_list: HashMap::default(),
            known_adt: HashMap::default(),
            known_adt_tag: HashMap::default(),
            known_map: HashMap::default(),
            known_set: HashMap::default(),
            known_iota: HashMap::default(),
        }
    }

    fn propagate_alias(&mut self, dst: u32, src: u32) {
        if let Some(n) = self.known_int.get(src) {
            self.known_int.insert(dst, n);
        }
        if let Some(&bits) = self.known_float.get(&src) {
            self.known_float.insert(dst, bits);
        }
        if let Some(s) = self.known_string.get(&src).cloned() {
            self.known_string.insert(dst, s);
        }
        if let Some(elems) = self.known_list.get(&src).cloned() {
            self.known_list.insert(dst, elems);
        }
        if let Some(fields) = self.known_adt.get(&src).cloned() {
            self.known_adt.insert(dst, fields);
        }
        if let Some(&tag) = self.known_adt_tag.get(&src) {
            self.known_adt_tag.insert(dst, tag);
        }
        if let Some(pairs) = self.known_map.get(&src).cloned() {
            self.known_map.insert(dst, pairs);
        }
        if let Some(elems) = self.known_set.get(&src).cloned() {
            self.known_set.insert(dst, elems);
        }
        if let Some(iota) = self.known_iota.get(&src).copied() {
            self.known_iota.insert(dst, iota);
        }
    }

    fn fold_builtin(&mut self, name: Builtin, args: &[Local], local: u32, value: &mut Value) {
        let _ = iota::fold(self, name, args, local, value)
            || list::fold(self, name, args, local, value)
            || map_set::fold(self, name, args, local, value)
            || adt::fold(self, name, args, local, value);
    }
}

pub(crate) fn const_fold_block(block: &mut Block) {
    let mut env = FoldEnv::new();
    for op in &mut block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                match value {
                    Value::Int(_) | Value::Bool(_) | Value::Char(_) => {
                        env.known_int.track(local.0, value);
                    }
                    Value::Float(f) => {
                        env.known_float.insert(local.0, f.to_bits());
                    }
                    Value::String(s) => {
                        env.known_string.insert(local.0, s.clone());
                    }
                    Value::Local(Local(src)) => {
                        // Track constants through aliases; keep Local for CSE sharing.
                        env.propagate_alias(local.0, *src);
                    }
                    Value::AllocList { elems, .. } => {
                        env.known_list.insert(local.0, elems.clone());
                    }
                    Value::AllocMap { flat_pairs, .. } => {
                        helpers::compact_map_pairs(flat_pairs, &env);
                        env.known_map.insert(local.0, flat_pairs.clone());
                    }
                    Value::AllocSet { elems, .. } => {
                        helpers::compact_set_elems(elems, &env);
                        env.known_set.insert(local.0, elems.clone());
                    }
                    Value::AllocAdt { tag, fields, .. } => {
                        env.known_adt.insert(local.0, fields.clone());
                        env.known_adt_tag.insert(local.0, *tag);
                    }
                    Value::Unary {
                        op: UnOp::Neg,
                        operand,
                    } => {
                        if let Some(&bits) = env.known_float.get(&operand.0) {
                            let neg = (-f64::from_bits(bits)).to_bits();
                            env.known_float.insert(local.0, neg);
                        } else if let Some(n) = env.known_int.get(operand.0) {
                            if let Some(r) = n.checked_neg() {
                                *value = Value::Int(r);
                                env.known_int.insert(local.0, r);
                            }
                            // Overflow (i64::MIN): leave Neg for runtime trap.
                        }
                    }
                    Value::Unary {
                        op: UnOp::Not,
                        operand,
                    } => {
                        if let Some(n) = env.known_int.get(operand.0) {
                            let r = n == 0;
                            *value = Value::Bool(r);
                            env.known_int.insert(local.0, if r { 1 } else { 0 });
                        }
                    }
                    Value::Binary { op, left, right } => {
                        if let (Some(a), Some(b)) =
                            (env.known_int.get(left.0), env.known_int.get(right.0))
                        {
                            if let Some(r) = fold_bin(*op, a, b) {
                                // Keep Bool for cmp/logic so println / ABI typing stay correct.
                                *value = if matches!(
                                    op,
                                    BinOp::Eq
                                        | BinOp::Ne
                                        | BinOp::Lt
                                        | BinOp::Le
                                        | BinOp::Gt
                                        | BinOp::Ge
                                        | BinOp::And
                                        | BinOp::Or
                                ) {
                                    Value::Bool(r != 0)
                                } else {
                                    Value::Int(r)
                                };
                                env.known_int.insert(local.0, r);
                            }
                        }
                    }
                    Value::Builtin { name, args, .. } => {
                        let name = *name;
                        let args = args.clone();
                        let local_id = local.0;
                        env.fold_builtin(name, &args, local_id, value);
                    }
                    v @ (Value::If { .. } | Value::Loop { .. }) => {
                        for_each_ctrl_nested_block_mut(v, &mut |nested| {
                            const_fold_block(nested);
                        });
                    }
                    _ => {}
                }
            }
            Op::Let { value, .. } => {
                for_each_ctrl_nested_block_mut(value, &mut |nested| {
                    const_fold_block(nested);
                });
            }
            _ => {}
        }
    }
}

fn fold_bin(op: BinOp, a: i64, b: i64) -> Option<i64> {
    Some(match op {
        BinOp::Add => a.checked_add(b)?,
        BinOp::Sub => a.checked_sub(b)?,
        BinOp::Mul => a.checked_mul(b)?,
        BinOp::Div if b != 0 && !(a == i64::MIN && b == -1) => a / b,
        BinOp::Rem if b != 0 && !(a == i64::MIN && b == -1) => a % b,
        BinOp::Eq => (a == b) as i64,
        BinOp::Ne => (a != b) as i64,
        BinOp::Lt => (a < b) as i64,
        BinOp::Le => (a <= b) as i64,
        BinOp::Gt => (a > b) as i64,
        BinOp::Ge => (a >= b) as i64,
        BinOp::And => ((a != 0) && (b != 0)) as i64,
        BinOp::Or => ((a != 0) || (b != 0)) as i64,
        _ => return None,
    })
}

pub(crate) fn copy_prop_block(block: &mut Block) {
    let mut rewrite: HashMap<u32, u32> = HashMap::default();
    for op in &mut block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                rewrite_value(value, &rewrite);
                if let Value::Local(Local(src)) = value {
                    let root = rewrite.get(src).copied().unwrap_or(*src);
                    rewrite.insert(local.0, root);
                    *value = Value::Local(Local(root));
                }
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    copy_prop_block(then_block);
                    copy_prop_block(else_block);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    copy_prop_block(header);
                    copy_prop_block(body);
                    copy_prop_block(latch);
                }
            }
            Op::Let { value, .. } => {
                rewrite_value(value, &rewrite);
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    copy_prop_block(then_block);
                    copy_prop_block(else_block);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    copy_prop_block(header);
                    copy_prop_block(body);
                    copy_prop_block(latch);
                }
            }
            Op::Assign { value, .. } | Op::Return { value } => {
                if let Some(r) = rewrite.get(&value.0) {
                    *value = Local(*r);
                }
            }
            Op::Break | Op::Continue => {}
        }
    }
    if let Some(r) = block.result {
        if let Some(nr) = rewrite.get(&r.0) {
            block.result = Some(Local(*nr));
        }
    }
}
