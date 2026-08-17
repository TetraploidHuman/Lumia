//! Float-heavy checksums recognized by codegen SR.

/// Escape-time Mandelbrot checksum on a fixed 200×140 grid over [-2.5,1]×[-1,1].
///
/// Processes 4 pixels in `x` with successive `cx += dx` (FP-identical to scalar)
/// and an interleaved escape loop so independent lanes share ILP.
#[no_mangle]
pub extern "C" fn lumia_mandelbrot_checksum(max_it: i64) -> i64 {
    mandelbrot_x4(max_it)
}

fn mandelbrot_x4(max_it: i64) -> i64 {
    const W: i64 = 200;
    const H: i64 = 140;
    // Match Lumia: `for it < maxIt` never runs when maxIt≤0, then every pixel
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
#[path = "float_kernels_tests.rs"]
mod tests;
