// Broad syntax / usage surface coverage.

#[test]
fn e2e_syntax_surface() {
    run_example(
        "examples/syntax_surface.lm",
        &[
        "42",
        "3.5",
        "true",
        "false",
        "Q",
        "hi",
        "7",
        "14",
        "1",
        "1",
        "1",
        "0",
        "1",
        "1",
        "1",
        "11",
        "100",
        "25",
        "15",
        "60",
        "2",
        "6",
        "10",
        "4",
        "1",
        "4",
        "4",
        "9",
        "1",
        "2",
        "3",
        "1",
        "5",
        "-1",
        "99",
        "1",
        "1",
        "1",
        "15",
        "3",
        "10",
        ],
    );
}

#[test]
fn e2e_syntax_hof_match() {
    run_example(
        "examples/syntax_hof_match.lm",
        &[
        "9",
        "10",
        "13",
        "7",
        "42",
        "5",
        "1",
        "2",
        "1",
        "1",
        ],
    );
}

#[test]
fn e2e_syntax_extras() {
    run_example(
        "examples/syntax_extras.lm",
        &[
        "2",
        "-1",
        "0",
        "1",
        "4",
        "Lumia",
        "n=7",
        "sum=8",
        "20",
        "12",
        "5",
        "1",
        ],
    );
}

#[test]
fn e2e_sum_mixed_arity() {
    run_example(
        "examples/sum_mixed_arity.lm",
        &[
        "9",
        "10",
        "4",
        ],
    );
}

#[test]
fn e2e_eq_hash_consistent() {
    run_example(
        "examples/eq_hash_consistent.lm",
        &[
        "1",
        "1",
        "10",
        ],
    );
}

#[test]
fn e2e_syntax_ascription() {
    run_example(
        "examples/syntax_ascription.lm",
        &["42", "7", "10", "11", "1.5"],
    );
}
