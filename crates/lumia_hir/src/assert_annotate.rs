//! Inject `assert` failure messages before Core lower.

use crate::visit::for_each_expr_mut;
use crate::{Builtin, Expr, Item, Module};
use lumia_syntax::{byte_to_line_col, line_starts};

/// Rewrite bare `assert(cond)` into `assert(cond, "path:line: assert failed")`.
///
/// `files[span.file]` supplies `(path_label, source)` after module stamp.
pub fn annotate_assert_messages(module: &mut Module, files: &[(&str, &str)]) {
    for item in &mut module.items {
        match item {
            Item::Fun(f) => annotate_body(&mut f.body, files),
            Item::Val { body, .. } => annotate_body(body, files),
        }
    }
}

fn annotate_body(body: &mut Expr, files: &[(&str, &str)]) {
    for_each_expr_mut(body, &mut |e| {
        let Expr::BuiltinCall {
            name: Builtin::Assert,
            args,
            span,
        } = e
        else {
            return;
        };
        if args.len() != 1 {
            return;
        }
        let (path, src) = files
            .get(span.file as usize)
            .or_else(|| files.first())
            .copied()
            .unwrap_or(("<unknown>", ""));
        let starts = line_starts(src);
        let (line, _) = byte_to_line_col(&starts, span.start);
        let msg = format!("{path}:{line}: assert failed");
        args.push(Expr::String(msg, *span));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_syntax::Span;
    use rustc_hash::{FxHashMap, FxHashSet};

    #[test]
    fn annotate_adds_message_arg() {
        let span = Span::new(10, 16);
        let mut module = Module {
            name: "M".into(),
            items: vec![Item::Val {
                name: "main".into(),
                body: Expr::BuiltinCall {
                    name: Builtin::Assert,
                    args: vec![Expr::Bool(false, span)],
                    span,
                },
                ty: None,
                span,
            }],
            adts: Vec::new(),
            products: Vec::new(),
            instances: FxHashSet::default(),
            show_methods: FxHashMap::default(),
            trait_methods: FxHashMap::default(),
            method_traits: FxHashMap::default(),
        };
        let src = "module M\nval main = { assert(false) }\n";
        annotate_assert_messages(&mut module, &[("t.lm", src)]);
        let Item::Val { body, .. } = &module.items[0] else {
            panic!("val");
        };
        let Expr::BuiltinCall { args, .. } = body else {
            panic!("assert");
        };
        assert_eq!(args.len(), 2);
        match &args[1] {
            Expr::String(s, _) => assert!(s.contains("t.lm:") && s.contains("assert failed"), "{s}"),
            _ => panic!("expected string message"),
        }
    }
}
