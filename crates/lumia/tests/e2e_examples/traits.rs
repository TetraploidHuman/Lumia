#[test]
fn e2e_trait_ord() {
    // `instance Ord for Point` enables lexicographic `<`/`>` on products.
    run_example(
        "examples/trait_ord.lm",
        &["true", "true", "true", "true", "true"],
    );
}

#[test]
fn e2e_trait_show() {
    // Custom Show override; named structural Show for types without a method body.
    run_example("examples/trait_show.lm", &["Point", "Box(9)"]);
}

#[test]
fn e2e_trait_show_method() {
    // UFCS `p.show()` → Show builtin / instance override.
    run_example("examples/trait_show_method.lm", &["Point", "Point"]);
}

#[test]
fn e2e_trait_custom_method() {
    // User trait UFCS → mangled `__ToInt_Point_toInt`.
    run_example("examples/trait_custom_method.lm", &["7", "3"]);
}

#[test]
fn e2e_trait_poly_show() {
    // `{ x -> x.show() }` monomorphized at two Show instances.
    run_example("examples/trait_poly_show.lm", &["P", "B"]);
}

#[test]
fn e2e_trait_poly_method() {
    // `{ x -> x.toInt() }` deferred UFCS + post-mono mangled resolve.
    run_example("examples/trait_poly_method.lm", &["7", "4"]);
}

e2e_ok!(
    e2e_trait_custom_default,
    "examples/trait_custom_default.lm",
    "default"
);

e2e_ok!(
    e2e_trait_default_show,
    "examples/trait_default_show.lm",
    "default-show"
);

#[test]
fn e2e_trait_eq_ord() {
    run_example(
        "examples/trait_eq_ord.lm",
        &["true", "false", "true", "true", "true"],
    );
}

#[test]
fn e2e_trait_eq_ord_method() {
    // UFCS `p.eq(q)` / `p.less(r)` → Binary Eq/Lt + trait overrides.
    run_example(
        "examples/trait_eq_ord_method.lm",
        &["true", "true", "false"],
    );
}

e2e_ok!(e2e_trait_num, "examples/trait_num.lm", "6", "8", "8", "15");
