# v0.2.2

- `FixedVec` now overwrites `as_{mut_,}ptr` and returns a pointer with
  provenance for the capacity of the allocation, instead of only to the
  initialized region.
- Fixed related **unsound** bugs that would cause `FixedVec::fill` to write
  through a pointer that didn't have the right provenance for doing so. Found
  via Miri.
- Depends on `unsize` which enables a manual emulation of the unsize coercion
  on `Box` and `Uninit{,View}`.

# v0.2.1

- Align `Extend` implementation to t-libs recommendation, panics on overflow
- Fixed an aliasing issue in the *test suite*

# v0.2.0

Contains all previous data structure work from `static-alloc`
