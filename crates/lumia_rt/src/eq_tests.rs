use super::lumia_eq;

#[test]
fn scalar_non_heap_eq_is_bit_identity() {
    assert_eq!(lumia_eq(0, 0), 1);
    assert_eq!(lumia_eq(1, 2), 0);
    let pos0 = 0.0f64.to_bits() as i64;
    let neg0 = (-0.0f64).to_bits() as i64;
    assert_ne!(pos0, neg0);
    assert_eq!(lumia_eq(pos0, neg0), 0);
}
