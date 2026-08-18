use super::lumia_mem_traffic_checksum;

#[test]
fn mem_traffic_bench_fingerprint() {
    assert_eq!(
        lumia_mem_traffic_checksum(1_500_000, 3, 2_000_000),
        860_371_869
    );
}

#[test]
fn mem_traffic_edges() {
    assert_eq!(lumia_mem_traffic_checksum(0, 3, 10), 0);
    assert_eq!(lumia_mem_traffic_checksum(-1, 3, 10), 0);
    assert_eq!(lumia_mem_traffic_checksum(10, 0, 0), 0);
}
