use super::*;

#[test]
fn open_product_field_rejects_wrong_receiver() {
    let src = r#"
module M
type Point { val x val y }
val getx = { p -> p.x }
val main = { getx(1) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(
        infer_module(&hir).is_err(),
        "open field proj must not accept Int"
    );
}

#[test]
fn open_tuple_proj_rejects_non_tuple() {
    let src = r#"
module M
val fst = { t -> t.0 }
val main = { fst(1) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("`.0` on Int must fail");
    assert!(
        err.message().contains("tuple") || err.message().contains("mismatch"),
        "got {}",
        err.message()
    );
}

#[test]
fn open_tuple_proj_accepts_longer_tuple() {
    let src = r#"
module M
val fst = { t -> t.0 }
val main = { fst((10, 20)) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("`.0` on pair must type-check");
}
