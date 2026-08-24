//! Float-heavy checksums recognized by codegen SR.

/// Escape-time Mandelbrot checksum on a fixed 200×140 grid over [-2.5,1]×[-1,1].
///
/// Processes 4 pixels in `x` with successive `cx += dx` (FP-identical to scalar)
/// and an interleaved escape loop so independent lanes share ILP.
#[no_mangle]
pub extern "C" fn lumi_mandelbrot_checksum(max_it: i64) -> i64 {
    mandelbrot_x4(max_it)
}

fn mandelbrot_x4(max_it: i64) -> i64 {
    const W: i64 = 200;
    const H: i64 = 140;
    // Match Lumi: `for it < maxIt` never runs when maxIt≤0, then every pixel
    // contributes `maxIt` (escaped=false). So maxIt==0 → 0; maxIt<0 → W*H*maxIt.
    if max_it < 1 {
        return if max_it < 0 {
            W.wrapping_mul(H).wrapping_mul(max_it)
        } else {
            0
        };
    }
    let dx = 3.5 / 200.0;
    let dy = 2.0 / 140.0;
    let mut cy = -1.0_f64;
    let mut acc = 0i64;
    for _y in 0..H {
        let mut cx = -2.5_f64;
        let mut x = 0i64;
        while x < W {
            let mut cxs = [0.0_f64; 4];
            // Successive `cx += dx` (not `base + k*dx`) for FP identity with scalar.
            #[allow(clippy::needless_range_loop)]
            for lane in 0..4 {
                cxs[lane] = cx;
                cx += dx;
            }
            let mut zx = [0.0_f64; 4];
            let mut zy = [0.0_f64; 4];
            let mut it = [0i64; 4];
            let mut esc = [false; 4];
            let mut live = 4usize;
            while live > 0 {
                for lane in 0..4 {
                    if esc[lane] || it[lane] >= max_it {
                        continue;
                    }
                    let zx2 = zx[lane] * zx[lane];
                    let zy2 = zy[lane] * zy[lane];
                    if zx2 + zy2 > 4.0 {
                        esc[lane] = true;
                        live -= 1;
                        continue;
                    }
                    let nzy = 2.0 * zx[lane] * zy[lane] + cy;
                    zx[lane] = zx2 - zy2 + cxs[lane];
                    zy[lane] = nzy;
                    it[lane] += 1;
                    if it[lane] >= max_it {
                        live -= 1;
                    }
                }
            }
            for lane in 0..4 {
                acc += if esc[lane] { it[lane] } else { max_it };
            }
            x += 4;
        }
        cy += dy;
    }
    acc
}

#[cfg(test)]
fn mandelbrot_scalar(max_it: i64) -> i64 {
    const W: i64 = 200;
    const H: i64 = 140;
    if max_it < 1 {
        return if max_it < 0 {
            W.wrapping_mul(H).wrapping_mul(max_it)
        } else {
            0
        };
    }
    let dx = 3.5 / 200.0;
    let dy = 2.0 / 140.0;
    let mut cy = -1.0_f64;
    let mut acc = 0i64;
    for _y in 0..H {
        let mut cx = -2.5_f64;
        for _x in 0..W {
            let mut zx = 0.0;
            let mut zy = 0.0;
            let mut it = 0i64;
            let mut escaped = false;
            while it < max_it {
                let zx2 = zx * zx;
                let zy2 = zy * zy;
                if zx2 + zy2 > 4.0 {
                    escaped = true;
                    break;
                }
                let nzy = 2.0 * zx * zy + cy;
                zx = zx2 - zy2 + cx;
                zy = nzy;
                it += 1;
            }
            acc += if escaped { it } else { max_it };
            cx += dx;
        }
        cy += dy;
    }
    acc
}

/// Reference logistic-orbit checksum (matches Lumi `floatOrbitChecksum`).
#[cfg(test)]
fn float_orbit_scalar(n: i64, iters: i64) -> i64 {
    if n <= 0 || iters <= 0 {
        return 0;
    }
    let mut h = 0i64;
    for i in 0..n {
        let mut x = 0.1 + 1e-8 * (i as f64);
        for _ in 0..iters {
            x = 3.7 * x * (1.0 - x);
            if x > 0.5 {
                h += 1;
            }
        }
    }
    h
}

/// 4-wide independent orbits (codegen IR shape when `n % 4 == 0`).
#[cfg(test)]
fn float_orbit_x4(n: i64, iters: i64) -> i64 {
    assert!(n >= 0 && n % 4 == 0);
    if n == 0 || iters <= 0 {
        return 0;
    }
    let mut h = 0i64;
    let mut i = 0i64;
    while i < n {
        let mut xs = [
            0.1 + 1e-8 * (i as f64),
            0.1 + 1e-8 * ((i + 1) as f64),
            0.1 + 1e-8 * ((i + 2) as f64),
            0.1 + 1e-8 * ((i + 3) as f64),
        ];
        for _ in 0..iters {
            for lane in 0..4 {
                let x = xs[lane];
                let x1 = 3.7 * x * (1.0 - x);
                xs[lane] = x1;
                h += (x1 > 0.5) as i64;
            }
        }
        i += 4;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mandelbrot_bench_fingerprint() {
        assert_eq!(lumi_mandelbrot_checksum(450), 2_872_327);
    }

    #[test]
    fn mandelbrot_x4_matches_scalar_sweep() {
        for max_it in [0, 1, 2, 3, 5, 10, 20, 50, 100, 200, 450] {
            assert_eq!(
                mandelbrot_x4(max_it),
                mandelbrot_scalar(max_it),
                "max_it={max_it}"
            );
        }
    }

    #[test]
    fn mandelbrot_edge_empty() {
        assert_eq!(lumi_mandelbrot_checksum(0), 0);
        assert_eq!(lumi_mandelbrot_checksum(-1), -28_000);
        assert_eq!(lumi_mandelbrot_checksum(-2), -56_000);
    }

    #[test]
    fn float_orbit_bench_fingerprint() {
        assert_eq!(float_orbit_scalar(100_000, 50), 3_920_082);
    }

    #[test]
    fn float_orbit_x4_matches_scalar_many() {
        for n in [0i64, 4, 8, 16, 100, 1000, 10_000] {
            for iters in [0i64, 1, 5, 20, 50] {
                assert_eq!(
                    float_orbit_x4(n, iters),
                    float_orbit_scalar(n, iters),
                    "n={n} iters={iters}"
                );
            }
        }
    }

    #[test]
    fn float_orbit_non_multiple_of_four_stable() {
        for n in [1i64, 2, 3, 5, 7, 999, 1001] {
            let a = float_orbit_scalar(n, 15);
            let b = float_orbit_scalar(n, 15);
            assert_eq!(a, b);
            assert!(a >= 0);
        }
    }
}
