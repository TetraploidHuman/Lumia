use super::*;

#[test]
fn memo_tf_hit_miss() {
    lumia_memo_l2_reset();
    let mut out = 0i64;
    assert_eq!(lumia_memo_l2_lookup(0, 1, 42, 0, 0, 0, &mut out), 0);
    lumia_memo_l2_store(0, 1, 42, 0, 0, 0, 99);
    assert_eq!(lumia_memo_l2_lookup(0, 1, 42, 0, 0, 0, &mut out), 1);
    assert_eq!(out, 99);
    assert_eq!(lumia_memo_l2_lookup(0, 1, 7, 0, 0, 0, &mut out), 0);
    // 4-arg key
    lumia_memo_l2_store(1, 4, 1, 2, 3, 4, 77);
    assert_eq!(lumia_memo_l2_lookup(1, 4, 1, 2, 3, 4, &mut out), 1);
    assert_eq!(out, 77);
    assert_eq!(lumia_memo_l2_lookup(1, 4, 1, 2, 3, 5, &mut out), 0);
    assert!(lumia_memo_l2_hits() >= 2);
    assert!(lumia_memo_l2_misses() >= 2);
    lumia_memo_l2_reset();
}

#[test]
fn memo_idx_hit_miss() {
    lumia_memo_idx_reset();
    let mut out = 0i64;
    // Cold miss must not allocate a dense table (created on first store).
    assert_eq!(lumia_memo_idx_lookup(0, 10, &mut out), 0);
    assert_eq!(lumia_memo_idx_misses(), 0);
    lumia_memo_idx_store(0, 10, 55);
    assert_eq!(lumia_memo_idx_lookup(0, 10, &mut out), 1);
    assert_eq!(out, 55);
    assert_eq!(lumia_memo_idx_lookup(0, 11, &mut out), 0);
    assert_eq!(lumia_memo_idx_lookup(0, -1, &mut out), 0);
    assert_eq!(lumia_memo_idx_lookup(0, MEMO_IDX_CAP as i64, &mut out), 0);
    assert!(lumia_memo_idx_hits() >= 1);
    assert!(lumia_memo_idx_misses() >= 1);
    lumia_memo_idx_reset();
}
