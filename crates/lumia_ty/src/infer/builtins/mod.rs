//! BuiltinCall typing (split by family for maintainability).

mod adt;
mod io;
mod list;
mod map_set;
mod string;

use super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::{Builtin, BuiltinFamily, Expr};

impl Infer {
    pub(crate) fn infer_builtin_call(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let info = name.info();
        let n = args.len();
        if n < info.min_arity as usize || n > info.max_arity as usize {
            let expected = if info.min_arity == info.max_arity {
                format!("{}", info.min_arity)
            } else {
                format!("{}..={}", info.min_arity, info.max_arity)
            };
            return Err(at(
                span,
                format!(
                    "{} takes {expected} argument(s), got {n}",
                    name.display_name()
                ),
            ));
        }
        match info.family {
            BuiltinFamily::Io => self.infer_io_builtin(name, args, span),
            BuiltinFamily::List => self.infer_list_builtin(name, args, span),
            BuiltinFamily::MapSet => self.infer_map_set_builtin(name, args, span),
            BuiltinFamily::String => self.infer_string_builtin(name, args, span),
            BuiltinFamily::Adt => self.infer_adt_builtin(name, args, span),
        }
    }
}
