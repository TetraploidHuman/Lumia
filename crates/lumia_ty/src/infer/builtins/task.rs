//! BuiltinCall typing — Task / Channel family.

use super::super::Infer;
use crate::types::{Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(crate) fn infer_task_builtin(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let io = Effect::io();
        match name {
            Builtin::ChannelNew => {
                let (ct, ce) = self.infer_expr(&args[0])?;
                self.unify_at(span, ct, Type::Int)?;
                let elem = self.fresh();
                Ok((Type::Channel(Box::new(elem)), self.union_eff(io, ce)))
            }
            Builtin::ChannelSend => {
                let (cht, che) = self.infer_expr(&args[0])?;
                let (vt, ve) = self.infer_expr(&args[1])?;
                let elem = self.fresh();
                self.unify_at(span, cht, Type::Channel(Box::new(elem.clone())))?;
                self.unify_at(span, vt, elem)?;
                Ok((Type::Unit, self.union3_eff(io, che, ve)))
            }
            Builtin::ChannelRecv => {
                let (cht, che) = self.infer_expr(&args[0])?;
                let elem = self.fresh();
                self.unify_at(span, cht, Type::Channel(Box::new(elem.clone())))?;
                Ok((elem, self.union_eff(io, che)))
            }
            Builtin::ChannelRecvOpt => {
                let (cht, che) = self.infer_expr(&args[0])?;
                let elem = self.fresh();
                self.unify_at(span, cht, Type::Channel(Box::new(elem.clone())))?;
                Ok((
                    Type::Adt {
                        name: lumia_hir::OPTION.name.into(),
                        params: vec![elem],
                    },
                    self.union_eff(io, che),
                ))
            }
            Builtin::ChannelClose => {
                let (cht, che) = self.infer_expr(&args[0])?;
                let elem = self.fresh();
                self.unify_at(span, cht, Type::Channel(Box::new(elem)))?;
                Ok((Type::Unit, self.union_eff(io, che)))
            }
            Builtin::TaskJoin => {
                let (tt, te) = self.infer_expr(&args[0])?;
                let elem = self.fresh();
                self.unify_at(span, tt, Type::Task(Box::new(elem.clone())))?;
                Ok((elem, self.union_eff(io, te)))
            }
            Builtin::TaskJoinOpt => {
                let (tt, te) = self.infer_expr(&args[0])?;
                let elem = self.fresh();
                self.unify_at(span, tt, Type::Task(Box::new(elem.clone())))?;
                Ok((
                    Type::Adt {
                        name: lumia_hir::OPTION.name.into(),
                        params: vec![elem],
                    },
                    self.union_eff(io, te),
                ))
            }
            Builtin::TaskSpawn => {
                // DESIGN §11.2: no shared mutability across tasks — reject capturing
                // an outer `var` (capture a `val` snapshot instead).
                for name in crate::infer::free_vars::free_var_names(&args[0]) {
                    if self.is_mutable(&name) {
                        return Err(crate::types::at(
                            span,
                            format!(
                                "spawn cannot capture mutable `{name}` \
                                 (no shared mutability; bind a `val` copy first)"
                            ),
                        ));
                    }
                }
                let (ft, fe) = self.infer_expr(&args[0])?;
                let ret = self.fresh();
                let eff = self.fresh_eff();
                self.unify_at(
                    span,
                    ft,
                    Type::Fun(vec![], Box::new(ret.clone()), eff),
                )?;
                Ok((Type::Task(Box::new(ret)), self.union_eff(io, fe)))
            }
            Builtin::ScopeEnter => {
                let (kt, ke) = self.infer_expr(&args[0])?;
                self.unify_at(span, kt, Type::Int)?;
                Ok((Type::Unit, self.union_eff(io, ke)))
            }
            Builtin::ScopeLeave | Builtin::ScopeCancel => Ok((Type::Unit, io)),
            _ => unreachable!("non-task builtin dispatched to infer_task_builtin"),
        }
    }
}
