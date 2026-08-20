use super::{fill_iota_i64, sum_i64};

#[test]
fn sum_matches_scalar_many() {
    for n in [0usize, 1, 3, 4, 7, 8, 15, 16, 100, 1000] {
        let mut v = vec![0i64; n];
        for (i, x) in v.iter_mut().enumerate() {
            *x = (i as i64).wrapping_mul(17).wrapping_add(3);
        }
        let mut expect = 0i64;
        for &x in &v {
            expect = expect.wrapping_add(x);
        }
        assert_eq!(sum_i64(v.as_ptr(), n), expect, "n={n}");
    }
}

#[test]
fn fill_iota_matches_scalar() {
    for (n, start) in [(0usize, 0i64), (1, 5), (4, -2), (17, 100), (64, -1000)] {
        let mut got = vec![0i64; n];
        fill_iota_i64(got.as_mut_ptr(), n, start);
        for i in 0..n {
            assert_eq!(got[i], start.wrapping_add(i as i64), "n={n} i={i}");
        }
    }
}
