use super::{for_each_expr, lower_module, Builtin, BuiltinFamily, Expr, Item};
use lumia_syntax::parse_module;

#[test]
fn builtin_family_routes_map_keys_with_map_set() {
    assert_eq!(Builtin::MapKeys.family(), BuiltinFamily::MapSet);
    assert_eq!(Builtin::Elems.family(), BuiltinFamily::List);
    assert_eq!(Builtin::ListLen.family(), BuiltinFamily::List);
    assert_eq!(Builtin::Show.family(), BuiltinFamily::Io);
    assert_eq!(Builtin::ChannelNew.family(), BuiltinFamily::Task);
    assert_eq!(Builtin::TaskJoin.family(), BuiltinFamily::Task);
}

#[test]
fn builtin_effect_and_symbols_are_wired() {
    assert!(Builtin::Println.is_io());
    assert!(Builtin::ReadStdin.is_io());
    assert!(!Builtin::ListLen.is_io());
    assert!(!Builtin::Assert.is_io());
    assert_eq!(Builtin::ListLen.runtime_symbol(), Some("lumia_len"));
    assert_eq!(Builtin::Println.runtime_symbol(), None);
    assert!(Builtin::ListAppend.info().float_sensitive());
    assert!(!Builtin::ListLen.info().float_sensitive());
    assert_eq!(
        Builtin::ListAppend.info().float_ensures,
        &[(1, lumia_abi::ENSURE_LIST_F64)]
    );
    assert_eq!(
        Builtin::MapSet.info().emit,
        super::BuiltinEmit::ObjI64I64Ptr
    );
    assert_eq!(
        Builtin::MapItems.info().emit,
        super::BuiltinEmit::UnaryObjBoolMask
    );
    assert_eq!(
        Builtin::MapItems.runtime_symbol(),
        Some("lumia_map_items")
    );
    assert_eq!(
        Builtin::SetInsert.info().emit,
        super::BuiltinEmit::ObjI64Ptr
    );
    assert_eq!(Builtin::StrSplit.info().emit, super::BuiltinEmit::ObjI64Ptr);
    assert_eq!(
        Builtin::ListGet.info().emit,
        super::BuiltinEmit::ObjI64OptionTags
    );
}

#[test]
fn method_surface_lowers_to_builtin_calls() {
    let src = r#"
module M
val main = {
listOf(1, 2).len()
listOf(1, 2).drop(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut saw_len = false;
    let mut saw_slice = false;
    crate::visit::for_each_expr(body, &mut |e| {
        if let Expr::BuiltinCall { name, .. } = e {
            saw_len |= *name == Builtin::ListLen;
            saw_slice |= *name == Builtin::ListSlice;
        }
    });
    assert!(saw_len, "expected ListLen from .len()");
    assert!(saw_slice, "expected ListSlice from .drop()");
}

#[test]
fn exhaustiveness_rejects_missing_variant() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Some"), "{err}");
}

#[test]
fn exhaustiveness_accepts_full_option() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
    Some(n) -> { n }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok());
}

#[test]
fn exhaustiveness_rejects_nested_missing() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
    Some(None) -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Some"), "{err}");
    assert!(err.contains("in Some"), "{err}");
}

#[test]
fn exhaustiveness_accepts_nested_option() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
    Some(None) -> { 1 }
    Some(Some(n)) -> { n }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok());
}

#[test]
fn exhaustiveness_rejects_nested_result_missing_err() {
    let src = r#"
module M
type Option { Some(value) None }
type Result { Ok(value) Err(msg) }
val f = { o ->
o match {
    None -> { 0 }
    Some(Ok(n)) -> { n }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Err"), "{err}");
}

#[test]
fn exhaustiveness_rejects_product_field_gap() {
    let src = r#"
module M
type Option { Some(value) None }
type Box { val inner }
val f = { b ->
b match {
    Box { inner = Some(n) } -> { n }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("None"), "{err}");
}

#[test]
fn exhaustiveness_accepts_nested_catch_all_payload() {
    let src = r#"
module M
type Option { Some(value) None }
type Result { Ok(value) Err(msg) }
val f = { o ->
o match {
    None -> { 0 }
    Some(_) -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok());
}

#[test]
fn exhaustiveness_rejects_int_literals_without_wildcard() {
    let src = r#"
module M
val f = { n ->
n match {
    0 -> { 1 }
    1 -> { 2 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Int"), "{err}");
}

#[test]
fn exhaustiveness_rejects_empty_match() {
    let src = r#"
module M
val f = { n ->
n match { }
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
}

#[test]
fn exhaustiveness_rejects_guard_only_arms() {
    let src = r#"
module M
val f = { n ->
n match {
    x if x > 0 -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
}

#[test]
fn exhaustiveness_accepts_int_with_wildcard() {
    let src = r#"
module M
val f = { n ->
n match {
    0 -> { 1 }
    _ -> { 2 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok());
}

#[test]
fn exhaustiveness_accepts_bool_both_arms() {
    let src = r#"
module M
val f = { b ->
b match {
    true -> { 1 }
    false -> { 0 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok(), "{:?}", lower_module(&ast));
}

#[test]
fn exhaustiveness_rejects_bool_missing_false() {
    let src = r#"
module M
val f = { b ->
b match {
    true -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("false") || err.contains("Bool"), "{err}");
}

#[test]
fn exhaustiveness_rejects_char_without_wildcard() {
    let src = r#"
module M
val f = { c ->
c match {
    'a' -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
}

#[test]
fn exhaustiveness_rejects_partial_list() {
    let src = r#"
module M
val f = { xs ->
xs match {
    [] -> { 0 }
    [x] -> { x }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("List"), "{err}");
}

#[test]
fn exhaustiveness_accepts_list_empty_and_rest() {
    let src = r#"
module M
val f = { xs ->
xs match {
    [] -> { 0 }
    [h, ..rest] -> { h }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok());
}

#[test]
fn exhaustiveness_rejects_nested_int_literal_gap() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
    Some(3) -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Int"), "{err}");
}

#[test]
fn exhaustiveness_rejects_nested_partial_list() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
    Some([a]) -> { a }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("List"), "{err}");
}

#[test]
fn with_ambiguous_product_field_defers_to_ty() {
    let src = r#"
module M
type Point { val x val y }
type Rect { val x val w }
val main = {
val p = Point { x = 1, y = 2 }
p with { x = 9 }
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("ambiguous `with` should lower (resolve in ty)");
    let mut found_with = false;
    for it in &hir.items {
        let body = match it {
            Item::Fun(f) => &f.body,
            Item::Val { body, .. } => body,
        };
        for_each_expr(body, &mut |e| {
            if matches!(e, Expr::With { .. }) {
                found_with = true;
            }
        });
    }
    assert!(found_with, "expected deferred HIR With");
}

#[test]
fn with_unique_field_set_still_defers() {
    // Field set {x,w} uniquely matches Rect — must NOT early-expand, or
    // `Point with { x, w }` would become Rect before ty sees the base.
    let src = r#"
module M
type Point { val x val y }
type Rect { val x val w }
val main = {
val p = Point { x = 1, y = 2 }
p with { x = 7, w = 9 }
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let mut found_with = false;
    let mut found_rect_new = false;
    for it in &hir.items {
        let body = match it {
            Item::Fun(f) => &f.body,
            Item::Val { body, .. } => body,
        };
        for_each_expr(body, &mut |e| {
            if matches!(e, Expr::With { .. }) {
                found_with = true;
            }
            if let Expr::AdtNew { adt_name, .. } = e {
                if adt_name == "Rect" {
                    found_rect_new = true;
                }
            }
        });
    }
    assert!(found_with, "expected deferred With");
    assert!(!found_rect_new, "must not early-expand to Rect AdtNew");
}

#[test]
fn struct_pattern_rejects_unknown_field() {
    let src = r#"
module M
type Point { val x val y }
val f = { p ->
p match {
    Point { z } -> z
    _ -> 0
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("unknown field"), "{err}");
    assert!(err.contains('z'), "{err}");
}

#[test]
fn struct_pattern_rejects_unknown_product() {
    let src = r#"
module M
type Point { val x val y }
val f = { p ->
p match {
    Piont { x } -> x
    _ -> 0
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("unknown product"), "{err}");
    assert!(err.contains("Piont"), "{err}");
}

/// Last-arm nullary ctor must still test the tag (and `MatchFail` on miss).
#[test]
fn last_arm_nullary_ctor_keeps_match_fail() {
    let src = r#"
module M
val f = { o ->
o match {
    Some(x) -> x
    None -> 0
}
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let fun = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "f" => Some(f),
            _ => None,
        })
        .expect("fun f");
    let mut has_match_fail = false;
    crate::visit::for_each_expr(&fun.body, &mut |e| {
        if matches!(
            e,
            Expr::BuiltinCall {
                name: Builtin::MatchFail,
                ..
            }
        ) {
            has_match_fail = true;
        }
    });
    assert!(
        has_match_fail,
        "last-arm `None` must remain refutable with MatchFail"
    );
}

#[test]
fn exhaustiveness_checks_instance_method_bodies() {
    let src = r#"
module M
type Option { Some(value) None }
trait Show {
val show = { self -> "" }
}
instance Show for Option {
val show = { self ->
    self match {
        None -> { "none" }
    }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Some"), "{err}");
}

#[test]
fn exhaustiveness_checks_trait_default_method_bodies() {
    let src = r#"
module M
type Option { Some(value) None }
trait Show {
val show = { self ->
    self match {
        None -> { "none" }
    }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
}

#[test]
fn instance_may_precede_trait_in_source_order() {
    let src = r#"
module M
type Point { val x }
instance Show for Point {
val show = { self -> "p" }
}
trait Show {
val show = { self -> "" }
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("instance before trait should lower");
    assert!(hir.instances.contains(&("Show".into(), "Point".into())));
}

#[test]
fn fun_and_val_carry_declaration_span() {
    let src = "module M\nval answer = 42\nval main = { answer }\n";
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let answer = hir.items.iter().find_map(|it| match it {
        Item::Val { name, span, .. } if name == "answer" => Some(*span),
        _ => None,
    });
    let main = hir.items.iter().find_map(|it| match it {
        Item::Fun(f) if f.name == "main" => Some(f.span),
        _ => None,
    });
    let answer = answer.expect("val answer");
    let main = main.expect("fun main");
    // Decl spans should cover the `val` keyword region, not only the body literal/block.
    assert!(
        answer.start.0 < src.find("42").unwrap() as u32,
        "answer decl span {answer:?} should start at `val`"
    );
    assert!(
        main.start.0 < src.find('{').unwrap() as u32,
        "main decl span {main:?} should start at `val`"
    );
}

#[test]
fn map_filter_chain_fuses_single_builder() {
    // Keep a list sink (not `.len()`) so build fusion applies, not count fusion.
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut flt_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__flt_acc") {
                flt_acc += 1;
            }
        }
    });
    assert_eq!(flt_acc, 0, "fused build must not emit separate filter acc");
    assert_eq!(
        map_acc, 1,
        "fused map/filter must use one list builder, got {map_acc}"
    );
}

#[test]
fn map_flatmap_chain_fuses_single_builder() {
    // Keep a list sink (not `.len()`) so flatMap build fusion applies.
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.map({ x -> x }).flatMap({ x -> listOf(x, x) })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut fmap_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "fused flatMap must not keep separate map builder");
    assert_eq!(fmap_acc, 1, "expected one fmap builder, got {fmap_acc}");
}

#[test]
fn map_any_chain_fuses_no_map_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).any({ x -> x > 5 })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut any_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__any_acc") {
                any_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "fused any must not keep map builder");
    assert_eq!(any_acc, 1, "expected one any acc, got {any_acc}");
}

#[test]
fn map_filter_len_fuses_count_loop() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).len()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut flt_acc = 0usize;
    let mut len_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__flt_acc") {
                flt_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "fused len must not keep map builder");
    assert_eq!(flt_acc, 0, "fused len must not keep filter builder");
    // ListLen on the *source* is expected (indexed for-each); builders must be gone.
    assert_eq!(len_acc, 1, "expected one len counter, got {len_acc}");
}

#[test]
fn map_filter_is_empty_fuses_short_circuit() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 100 }).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut empty_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__empty_acc") {
                empty_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "fused isEmpty must not keep map builder");
    assert_eq!(empty_acc, 1, "expected one empty acc, got {empty_acc}");
}

#[test]
fn map_flatmap_len_fuses_count_loop() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.map({ x -> x }).flatMap({ x -> listOf(x, x) }).len()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut fmap_acc = 0usize;
    let mut len_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
        }
    });
    assert_eq!(fmap_acc, 0, "fused flatMap.len must not build concat list");
    assert_eq!(len_acc, 1, "expected one len counter, got {len_acc}");
}

#[test]
fn flatmap_any_fuses_no_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.flatMap({ x -> listOf(x, x) }).any({ y -> y > 2 })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut fmap_acc = 0usize;
    let mut any_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
            if name.starts_with("__any_acc") {
                any_acc += 1;
            }
        }
    });
    assert_eq!(fmap_acc, 0, "fused flatMap.any must not build concat list");
    assert_eq!(any_acc, 1, "expected one any acc, got {any_acc}");
}

#[test]
fn flatmap_fold_fuses_no_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.flatMap({ x -> listOf(x, x) }).fold(0, { a, y -> a + y })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut fmap_acc = 0usize;
    let mut fuse_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
            if name.starts_with("__fuse_acc") {
                fuse_acc += 1;
            }
        }
    });
    assert_eq!(fmap_acc, 0, "fused flatMap.fold must not build concat list");
    assert!(fuse_acc >= 1, "expected fused fold acc");
}

#[test]
fn map_filter_get_fuses_no_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).get(0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut flt_acc = 0usize;
    let mut get_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__flt_acc") {
                flt_acc += 1;
            }
            if name.starts_with("__get_acc") {
                get_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "fused get must not keep map builder");
    assert_eq!(flt_acc, 0, "fused get must not keep filter builder");
    assert_eq!(get_acc, 1, "expected one get Option slot, got {get_acc}");
}

#[test]
fn let_bound_map_filter_get_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.get(0) + ys.get(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut flt_acc = 0usize;
    let mut get_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__flt_acc") {
                flt_acc += 1;
            }
            if name.starts_with("__get_acc") {
                get_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let-bound fused get must not keep map builder");
    assert_eq!(flt_acc, 0, "let-bound fused get must not keep filter builder");
    assert_eq!(get_acc, 2, "expected two fused gets, got {get_acc}");
}

#[test]
fn lone_map_let_get_still_materializes() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  val ys = xs.map({ x -> x * 2 })
  ys.get(0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut par_map = 0usize;
    let mut get_acc = 0usize;
    for_each_expr(body, &mut |e| {
        match e {
            Expr::Let { name, .. } if name.starts_with("__get_acc") => get_acc += 1,
            Expr::BuiltinCall {
                name: Builtin::ListParMap,
                ..
            } => par_map += 1,
            _ => {}
        }
    });
    assert_eq!(get_acc, 0, "lone map + let-get must not deforest");
    assert!(
        par_map >= 1,
        "lone map + let-get must keep ListParMap (Float ABI clones)"
    );
}
