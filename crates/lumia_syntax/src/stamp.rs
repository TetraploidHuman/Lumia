//! Stamp / shift spans after parse (multi-file SourceMap, `${…}` rebase).

use crate::visit::{map_expr_spans, map_module_spans};
use crate::{Expr, Module};

pub fn stamp_module(m: &mut Module, file: u32) {
    map_module_spans(m, &mut |s| *s = s.with_file(file));
}

/// Shift every span in an expression by `delta` bytes (absolute rebase after
/// parsing a `${…}` fragment that was lexed relative to offset 0).
pub fn offset_expr(e: &mut Expr, delta: u32) {
    if delta == 0 {
        return;
    }
    map_expr_spans(e, &mut |s| *s = s.shift(delta));
}
