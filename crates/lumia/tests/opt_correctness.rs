//! Release fingerprints + Debug≡Release for SR / perf-opt workloads.
//!
//! Catches silent semantic drift from codegen SR, RT helpers, and NSW/O3 paths.
//! Run: `cargo test -p lumia --test opt_correctness`

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    lumia_abi::workspace_root_canonical(env!("CARGO_MANIFEST_DIR"))
}

fn lumia_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lumia"))
}

fn out_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lumia_opt_corr_{}_{}",
        std::process::id(),
        format!("{:?}", std::thread::current().id()).replace(['(', ')'], "")
    ));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn exe_path(stem: &str) -> PathBuf {
    let name = if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    };
    out_dir().join(name)
}

fn build_and_run(rel: &str, release: bool) -> Vec<String> {
    let root = workspace_root();
    let src = root.join(rel);
    assert!(src.is_file(), "missing {}", src.display());
    let stem = Path::new(rel)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let suffix = if release { "rel" } else { "dbg" };
    // Unique path per call: parallel tests must not overwrite each other's binary.
    let exe = exe_path(&format!(
        "{stem}_{suffix}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let mut args = vec![
        "build".into(),
        src.to_str().unwrap().to_string(),
        "-o".into(),
        exe.to_str().unwrap().to_string(),
    ];
    if release {
        args.push("--release".into());
    }
    let status = Command::new(lumia_bin())
        .current_dir(&root)
        .args(&args)
        .status()
        .expect("spawn lumia build");
    assert!(
        status.success(),
        "lumia build failed for {rel} release={release}: {status}"
    );

    let output = Command::new(&exe)
        .output()
        .unwrap_or_else(|e| panic!("run {}: {e}", exe.display()));
    assert!(
        output.status.success(),
        "{rel} exited {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect()
}

/// Full `bench_cpu.lm` Release fingerprints (same as `scripts/bench_cpu.sh`).
#[test]
fn bench_cpu_release_fingerprints() {
    let got = build_and_run("examples/bench_cpu.lm", true);
    let expect = [
        "63951",
        "1998964270721",
        "2872327",
        "352279148",
        "142794532",
        "102334155",
        "720427763375",
        "9122320",
        "197458334",
        "405134546788",
        "3920082",
        "25014941572",
        "860371869",
    ];
    assert_eq!(
        got.len(),
        expect.len(),
        "line count mismatch\n got: {got:?}\n want {} lines",
        expect.len()
    );
    for (i, (g, e)) in got.iter().zip(expect.iter()).enumerate() {
        assert_eq!(g, e, "bench_cpu line {}: got {g}, want {e}", i + 1);
    }
}

/// Medium SR suite fingerprints (Release).
#[test]
fn opt_sr_correctness_release_fingerprints() {
    let got = build_and_run("examples/opt_sr_correctness.lm", true);
    let expect = [
        "2262",
        "3205658583",
        "355304",
        "2872327",
        "1834634",
        "955457",
        "6765",
        "796912738",
        "139848",
        "1166750",
        "162716781",
        "720196539084",
        "30072",
        "3920082",
        "12012",
        "100003461",
        "46455169",
        "0",
        "0",
        "0",
        "-28000",
        "0",
        "0",
        "0",
        "0",
    ];
    assert_eq!(got, expect, "opt_sr_correctness Release mismatch");
}

/// Debug and Release must agree on every checksum (opts must be semantics-preserving).
#[test]
fn opt_sr_correctness_debug_matches_release() {
    let rel = build_and_run("examples/opt_sr_correctness.lm", true);
    let dbg = build_and_run("examples/opt_sr_correctness.lm", false);
    assert_eq!(
        dbg, rel,
        "Debug/Release divergence in opt_sr_correctness\n debug: {dbg:?}\n release: {rel:?}"
    );
}

/// Cross-check selected Release lines against RT / reference oracles.
#[test]
fn opt_sr_correctness_matches_rt_oracles() {
    let got = build_and_run("examples/opt_sr_correctness.lm", true);
    // Indices aligned with `examples/opt_sr_correctness.lm` main.
    assert_eq!(
        got[0].parse::<i64>().unwrap(),
        lumia_rt::lumia_count_primes(20_000)
    );
    assert_eq!(
        got[1].parse::<i64>().unwrap(),
        lumia_rt::lumia_matmul_affine_checksum(80, 1_000_003)
    );
    assert_eq!(
        got[2].parse::<i64>().unwrap(),
        lumia_rt::lumia_mandelbrot_checksum(40)
    );
    assert_eq!(
        got[3].parse::<i64>().unwrap(),
        lumia_rt::lumia_mandelbrot_checksum(450)
    );
    assert_eq!(
        got[4].parse::<i64>().unwrap(),
        lumia_rt::lumia_collatz_total(20_000)
    );
    assert_eq!(
        got[5].parse::<i64>().unwrap(),
        lumia_rt::lumia_collatz_strided(1, 30_000, 3)
    );
    assert_eq!(
        got[7].parse::<i64>().unwrap(),
        lumia_rt::lumia_affine2_rem_sum(400, 131, 17, 1, 10_007)
    );
    assert_eq!(got[8].parse::<i64>().unwrap(), lumia_rt::lumia_gcd_sum(200));
    assert_eq!(
        got[9].parse::<i64>().unwrap(),
        lumia_rt::lumia_divisor_sum(100_000)
    );
    assert_eq!(
        got[10].parse::<i64>().unwrap(),
        lumia_rt::lumia_product_rem_sum(200, 10_007)
    );
    assert_eq!(
        got[11].parse::<i64>().unwrap(),
        lumia_rt::lumia_product_rem_sum(12_000, 10_007)
    );
    assert_eq!(
        got[15].parse::<i64>().unwrap(),
        lumia_rt::lumia_affine1_rem_sum(20_000, 131, 17, 10_007)
    );
}
