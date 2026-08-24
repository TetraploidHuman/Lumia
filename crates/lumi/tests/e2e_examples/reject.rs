e2e_reject!(e2e_bad_alt_int_rejected, "examples/bad_alt_int.lm", "alt");

e2e_reject!(
    e2e_bad_ascription_rejected,
    "examples/bad_ascription.lm",
    "mismatch"
);

e2e_reject!(
    e2e_bad_return_toplevel_rejected,
    "examples/bad_return_toplevel.lm",
    "`return` is only allowed"
);

#[test]
fn e2e_bad_trait_poly_rejected() {
    run_check(
        "examples/bad_trait_poly.lm",
        true,
        &[],
        &["ToInt", "instance"],
    );
}

#[test]
fn e2e_bad_import_as_original_rejected() {
    run_check(
        "examples/bad_import_as_original.lm",
        true,
        &[],
        &["private or not imported", "`add`"],
    );
}

#[test]
fn e2e_bad_let_destructure_rejected() {
    run_check(
        "examples/bad_let_destructure.lm",
        true,
        &[],
        &["irrefutable", "match"],
    );
}

#[test]
fn e2e_bad_import_priv_rejected() {
    run_check("examples/bad_import_priv.lm", true, &["private"], &[]);
}

#[test]
fn e2e_priv_leak_rejected() {
    let root = workspace_root();
    let src = root.join("examples/priv_leak_test.lm");
    let out = Command::new(lumi_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check priv_leak_test");
    assert!(
        !out.status.success(),
        "priv helper must not be visible via unrelated import"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("private") || combined.contains("unbound") || combined.contains("helper"),
        "expected priv/visibility error, got: {combined}"
    );
}

e2e_reject!(
    e2e_bad_nested_match_rejected,
    "examples/bad_nested_match.lm",
    "non-exhaustive",
    "bad_nested_match.lm:",
    ": lower:"
);

#[test]
fn e2e_bad_int_match_rejected() {
    run_check(
        "examples/bad_int_match.lm",
        true,
        &["non-exhaustive", "Int", "bad_int_match.lm:", "^"],
        &[],
    );
}

#[test]
fn e2e_bad_empty_match_rejected() {
    run_check(
        "examples/bad_empty_match.lm",
        true,
        &["non-exhaustive"],
        &[],
    );
}

#[test]
fn e2e_bad_guard_only_match_rejected() {
    run_check("examples/bad_guard_only.lm", true, &["non-exhaustive"], &[]);
}

#[test]
fn e2e_bad_list_match_rejected() {
    run_check(
        "examples/bad_list_match.lm",
        true,
        &["non-exhaustive", "List", "bad_list_match.lm:"],
        &[],
    );
}

#[test]
fn e2e_bad_assert_aborts() {
    let root = workspace_root();
    let src = root.join("examples/bad_assert.lm");
    let bin = e2e_exe("bad_assert");
    let status = Command::new(lumi_bin())
        .current_dir(&root)
        .args(["build", src.to_str().unwrap(), "-o", bin.to_str().unwrap()])
        .status()
        .expect("build bad_assert");
    assert!(status.success(), "bad_assert should compile");
    let run = Command::new(&bin).output().expect("run bad_assert");
    assert!(
        !run.status.success(),
        "assert(false) should abort the process"
    );
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(
        err.contains("assert failed") && err.contains("bad_assert.lm:"),
        "unexpected stderr: {err}"
    );
}

#[test]
fn e2e_bad_import_type_points_at_dep() {
    run_check(
        "examples/bad_import_type.lm",
        true,
        &["bad_dep.lm:", "type mismatch"],
        &[],
    );
}

#[test]
fn e2e_bad_dep_rejected() {
    run_check(
        "examples/bad_dep.lm",
        true,
        &["bad_dep.lm:", "type mismatch"],
        &[],
    );
}

e2e_reject!(
    e2e_bad_foreign_pure_rejected,
    "examples/bad_foreign_pure.lm",
    "trust-foreign-pure",
    "pure"
);

#[test]
fn e2e_unknown_std_module_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_std_import.lm");
    let out = Command::new(lumi_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_std_import");
    assert!(!out.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("unknown standard module") || combined.contains("not exported"),
        "expected std allowlist error, got: {combined}"
    );
}

#[test]
fn e2e_bad_field_proj_rejected() {
    run_check(
        "examples/bad_field_proj.lm",
        true,
        &[],
        &["expects type", "field projection", "cannot resolve"],
    );
}

#[test]
fn e2e_bad_tuple_proj_rejected() {
    run_check(
        "examples/bad_tuple_proj.lm",
        true,
        &[],
        &["tuple", "mismatch", "type mismatch"],
    );
}

#[test]
fn e2e_unknown_trait_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_trait.lm");
    let out = Command::new(lumi_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_trait");
    assert!(!out.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("unknown trait") || combined.contains("NotATrait"),
        "expected unknown-trait error, got: {combined}"
    );
}

#[test]
fn e2e_int_literal_overflow_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_int_overflow.lm");
    let out = Command::new(lumi_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_int_overflow");
    assert!(!out.status.success(), "overflowing Int literal must fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("out of range") || combined.contains("integer literal"),
        "expected overflow diagnostic, got: {combined}"
    );
}

#[test]
fn e2e_bad_val_assign_rejected() {
    run_check(
        "examples/bad_val_assign.lm",
        true,
        &[],
        &["immutable", "cannot assign"],
    );
}

#[test]
fn e2e_bad_struct_match_rejected() {
    run_check(
        "examples/bad_struct_match.lm",
        true,
        &[],
        &["expects type", "Point", "Rect"],
    );
}

#[test]
fn e2e_bad_ok_arity_rejected() {
    run_check(
        "examples/bad_ok_arity.lm",
        true,
        &[],
        &["expects", "field", "lower"],
    );
}

#[test]
fn e2e_bad_struct_field_match_rejected() {
    run_check(
        "examples/bad_struct_field_match.lm",
        true,
        &[],
        &["unknown field"],
    );
}

#[test]
fn e2e_bad_par_map_io_demoted() {
    // IO List.map must type-check after auto-parallel demotion to sequential.
    run_check("examples/bad_par_map_io.lm", false, &[], &[]);
}

e2e_reject!(
    e2e_bad_with_cross_product_rejected,
    "examples/bad_with_cross_product.lm",
    "unknown field"
);

e2e_reject!(
    e2e_bad_tuple_prefix_short_rejected,
    "examples/bad_tuple_prefix_short.lm",
    "tuple"
);

e2e_reject!(
    e2e_bad_ord_poly_list_rejected,
    "examples/bad_ord_poly_list.lm",
    "Ord"
);

#[test]
fn e2e_bad_std_star_ffi_rejected() {
    run_check(
        "examples/bad_std_star_ffi.lm",
        true,
        &[],
        &["lumi_list_f64_zeros", "private or not imported"],
    );
}

e2e_reject!(
    e2e_bad_eq_fun_rejected,
    "examples/bad_eq_fun.lm",
    "Eq",
    "function"
);

e2e_reject!(
    e2e_bad_eq_poly_fun_rejected,
    "examples/bad_eq_poly_fun.lm",
    "function"
);

e2e_reject!(
    e2e_bad_with_dup_field_rejected,
    "examples/bad_with_dup_field.lm",
    "duplicate"
);

e2e_reject!(
    e2e_bad_ord_poly_set_rejected,
    "examples/bad_ord_poly_set.lm",
    "Ord"
);

e2e_reject!(
    e2e_bad_with_open_ambiguous_rejected,
    "examples/bad_with_open_ambiguous.lm",
    "uniquely"
);

e2e_reject!(
    e2e_bad_eq_adt_fun_rejected,
    "examples/bad_eq_adt_fun.lm",
    "function"
);

e2e_reject!(
    e2e_bad_eq_list_fun_rejected,
    "examples/bad_eq_list_fun.lm",
    "function"
);

e2e_reject!(
    e2e_bad_list_rest_nested_rejected,
    "examples/bad_list_rest_nested.lm",
    "non-exhaustive"
);

e2e_reject!(
    e2e_bad_tuple_diag_rejected,
    "examples/bad_tuple_diag.lm",
    "non-exhaustive"
);
