use super::*;

extern "C" fn show_stub(x: i64) -> *mut u8 {
    let _ = x;
    0xdead as *mut u8
}

#[test]
fn register_and_lookup_show() {
    let name = b"UnitTestType\0";
    // SAFETY: `name` is a valid C string; `show_stub` matches Show ABI.
    unsafe {
        lumia_dict_register(TRAIT_SHOW, name.as_ptr(), show_stub as *const ());
        let p = lumia_dict_lookup(TRAIT_SHOW, name.as_ptr());
        assert!(!p.is_null());
        let out = lumia_dict_show(TRAIT_SHOW, name.as_ptr(), 0);
        assert_eq!(out, 0xdead as *mut u8);
    }
}
