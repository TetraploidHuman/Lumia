// Extracted from production module (Todo: RT 测例半迁).
use super::*;



#[test]
fn with_heap_reentrant() {
    with_heap(|h| {
        h.bytes_young = 42;
        with_heap(|inner| {
            assert_eq!(inner.bytes_young, 42);
            inner.bytes_young = 7;
        });
        assert_eq!(h.bytes_young, 7);
        h.bytes_young = 0;
    });
}
