#![feature(try_reserve)]

use static_alloc::Bump;

// Provide more memory than #[test] needs for the setup.
// Previously this constant was 1 << 16 which proved too little. This does not cost much more than
// some compile time. Not much, actually. Except on wasm where it will produce a zeroed data
// segment that can be quite large.
const MORE_THAN_CFG_TEST_ALLOCATES: usize = 1 << 20;
#[global_allocator]
static A: Bump<[u8; MORE_THAN_CFG_TEST_ALLOCATES]> = Bump::uninit();

#[test]
fn vec_fail_reserve() {
    let mut v: Vec<u8> = Vec::new();
    assert!(v.try_reserve(MORE_THAN_CFG_TEST_ALLOCATES + 1).is_err());
}
