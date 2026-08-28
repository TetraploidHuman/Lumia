//! Value emission — allocations and stack/heap helpers.

mod heap;
#[cfg(feature = "opt-repr-stack")]
mod stack;
