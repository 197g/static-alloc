# About this repository

This repository contains a few interconnected Rust libraries that enable
generic, dynamic data structures in environments that do not have a global
allocator or where its usage should be avoided.

* `alloc-traits`: This base crate contains a trait which defines the contract
  of a local allocator, substituting or supplementing the global allocator.
* `static-alloc`: Defines a limited allocator that requires no runtime
  operating system interaction by allocating from a memory region embedded in
  the program binary, and allocated by the program loader.
* `unsize`: A utility trait that can replace the unstable internal trait
  combination `Unsize`/`CoerceUnsize` so that custom smart pointers such as
  boxes can be unsized.
* `without-alloc`: Defines data structures similar to those in `alloc` which do
  not require a global allocator. They instead work on a once-allocated memory
  region with interoperability with allocators that impl `alloc-traits` traits.
