// Extracted from production module (Todo: RT 测例半迁).
use super::{
    lumia_alloc_string, lumia_str_byte_len, lumia_str_len, lumia_str_reverse, lumia_str_slice,
    lumia_str_substring, lumia_str_take, lumia_str_to_lower, lumia_str_to_upper, with_str_bytes,
};

#[test]
fn len_and_substring_use_codepoints() {
    let s = unsafe { lumia_alloc_string("你好".as_ptr(), "你好".len() as u64) };
    assert_eq!(unsafe { lumia_str_len(s) }, 2);
    assert_eq!(unsafe { lumia_str_byte_len(s) }, 6);
    let one = unsafe { lumia_str_substring(s, 0, 1) };
    with_str_bytes(one, |b| assert_eq!(b, "你".as_bytes()));
    let both = unsafe { lumia_str_substring(s, 0, 2) };
    with_str_bytes(both, |b| assert_eq!(b, "你好".as_bytes()));
    let emoji = unsafe { lumia_alloc_string("a😀b".as_ptr(), "a😀b".len() as u64) };
    assert_eq!(unsafe { lumia_str_len(emoji) }, 3);
    let mid = unsafe { lumia_str_substring(emoji, 1, 2) };
    with_str_bytes(mid, |b| assert_eq!(b, "😀".as_bytes()));
    let take = unsafe { lumia_str_take(s, 1) };
    with_str_bytes(take, |b| assert_eq!(b, "你".as_bytes()));
    let drop = unsafe { lumia_str_slice(s, 1) };
    with_str_bytes(drop, |b| assert_eq!(b, "好".as_bytes()));
    let rev = unsafe { lumia_str_reverse(s) };
    with_str_bytes(rev, |b| assert_eq!(b, "好你".as_bytes()));
    let low = unsafe { lumia_str_to_lower(lumia_alloc_string("ÄBC".as_ptr(), "ÄBC".len() as u64)) };
    with_str_bytes(low, |b| assert_eq!(b, "äbc".as_bytes()));
    let up = unsafe { lumia_str_to_upper(lumia_alloc_string("café".as_ptr(), "café".len() as u64)) };
    with_str_bytes(up, |b| assert_eq!(b, "CAFÉ".as_bytes()));
}
