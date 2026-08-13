use super::*;

#[test]
fn parallel_map_io_demoted_to_sequential() {
    let src = r#"
module ParIo
import std.io.{println}
val boom(x) = {
    println(x + 0)
    x + 1
}
val main = {
    listOf(1, 2, 3).map(boom)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(
        contains_list_par_map(
            hir.items
                .iter()
                .find_map(|i| match i {
                    Item::Fun(f) if f.is_main => Some(&f.body),
                    _ => None,
                })
                .unwrap()
        ),
        "FunRef-safe map should lower to ListParMap candidate"
    );
    let mut typed = infer_module(&hir).expect("IO map must type-check");
    finalize_auto_parallel(&mut typed, true);
    let main_body = typed
        .module
        .items
        .iter()
        .find_map(|i| match i {
            Item::Fun(f) if f.is_main => Some(&f.body),
            _ => None,
        })
        .unwrap();
    assert!(
        !contains_list_par_map(main_body),
        "impure map must be demoted after finalize_auto_parallel"
    );
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn parallel_map_pure_scalar_kept() {
    let src = r#"
module ParOk
val double(x) = x * 2
val main = {
    listOf(1, 2, 3).map(double)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let mut typed = infer_module(&hir).expect("infer");
    finalize_auto_parallel(&mut typed, true);
    let main_body = typed
        .module
        .items
        .iter()
        .find_map(|i| match i {
            Item::Fun(f) if f.is_main => Some(&f.body),
            _ => None,
        })
        .unwrap();
    assert!(
        contains_list_par_map(main_body),
        "pure scalar map should stay ListParMap"
    );
}

#[test]
fn parallel_map_toplevel_lambda_kept() {
    let src = r#"
module ParLam
val double(x) = x * 2
val main = {
    listOf(1, 2, 3).map({ x -> double(x) })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(
        contains_list_par_map(
            hir.items
                .iter()
                .find_map(|i| match i {
                    Item::Fun(f) if f.is_main => Some(&f.body),
                    _ => None,
                })
                .unwrap()
        ),
        "lambda calling only top-level funs should lower to ListParMap"
    );
    let mut typed = infer_module(&hir).expect("infer");
    finalize_auto_parallel(&mut typed, true);
    let main_body = typed
        .module
        .items
        .iter()
        .find_map(|i| match i {
            Item::Fun(f) if f.is_main => Some(&f.body),
            _ => None,
        })
        .unwrap();
    assert!(
        contains_list_par_map(main_body),
        "toplevel-only lambda map should stay ListParMap"
    );
}
