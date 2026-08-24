e2e_ok_release!(
    e2e_memo_tf_release,
    "examples/memo_tf.lm",
    "2646700",
    "2646700",
    "285"
);

e2e_ok!(e2e_memo_local, "examples/memo_local.lm", "42", "42", "65");

// Dense T_f path is Release-oriented; fib(30) is slow without memo tables.

e2e_ok_release!(e2e_memo_dense, "examples/memo_dense.lm", "832040", "55");

#[test]
fn e2e_par_map() {
    // Auto-parallel is on by default (no --parallel flag).
    run_example("examples/par_map.lm", &["200", "0", "398"]);
}

#[test]
fn e2e_par_fold() {
    // sum 0..99 = 4950; FunRef + capture-free lambda both ListParFold-safe.
    run_example("examples/par_fold.lm", &["4950", "4950"]);
}

e2e_ok!(e2e_par_map_fn, "examples/par_map_fn.lm", "50", "0", "98");

#[test]
fn e2e_par_map_toplevel_lam() {
    // `{ x -> double(x) }` with top-level `double` stays ListParMap-safe.
    run_example("examples/par_map_toplevel_lam.lm", &["50", "0", "98"]);
}

#[test]
fn e2e_par_map_capture() {
    // Capturing closure stays sequential; still correct under auto-parallel.
    run_example("examples/par_map_capture.lm", &["5", "10", "14"]);
}
