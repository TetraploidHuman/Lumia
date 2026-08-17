//! String, IO, and trap helpers (split for maintainability).

mod io;
mod string;
mod trap;

pub use io::{
    lumia_println_auto, lumia_println_bool, lumia_println_cstr, lumia_println_float,
    lumia_println_int, lumia_println_str, lumia_println_unit, lumia_read_stdin,
};
pub use string::{
    lumia_alloc_string, lumia_cstr_to_string, lumia_str_byte_len, lumia_str_concat,
    lumia_str_contains, lumia_str_ends_with, lumia_str_len, lumia_str_reverse, lumia_str_slice,
    lumia_str_split, lumia_str_starts_with, lumia_str_substring, lumia_str_take, lumia_str_to_lower,
    lumia_str_to_upper, lumia_str_trim, lumia_string_cstr,
};
pub(crate) use string::with_str_bytes;
pub use trap::{lumia_assert, lumia_match_fail, lumia_trap_div0, lumia_trap_overflow};
