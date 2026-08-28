#[test]
fn e2e_tco_sum() {
    // 2e6 tail calls — overflows without musttail; result = n(n+1)/2.
    run_example("examples/tco_sum.lm", &["2000001000000"]);
}

#[test]
fn e2e_tco_list_sum() {
    // Heap List param + musttail after root_pop; sum of range(0, 2e6) = 0..1999999.
    run_example("examples/tco_list_sum.lm", &["1999999000000"]);
}

#[test]
fn e2e_tco_io_countdown() {
    // IO on base case; recursive arm still musttail (~2e6 frames).
    run_example("examples/tco_io_countdown.lm", &["done", "0"]);
}

#[test]
fn e2e_tco_even_odd() {
    run_example("examples/tco_even_odd.lm", &["true", "false", "false"]);
}

#[test]
fn e2e_tco_funref() {
    // FunRef local → directized Call + musttail (2e6 depth).
    run_example("examples/tco_funref.lm", &["true", "false", "false"]);
}

#[test]
fn e2e_tco_float_sum() {
    // Pure Float musttail — same closed form as Int `tco_sum`.
    run_example("examples/tco_float_sum.lm", &["2000001000000"]);
}

#[test]
fn e2e_tco_sum_small() {
    run_example("examples/tco_sum_small.lm", &["12502500"]);
}
