//! Coroutine / Task / Channel correctness stress (BUILD §7.6).
//!
//! These are RT-level checks (no Lumia frontend). Language-level coverage lives
//! in `examples/task_*.lm` + `examples/bench_task.lm`.

#[cfg(test)]
#[path = "stress_tests.rs"]
mod tests;
