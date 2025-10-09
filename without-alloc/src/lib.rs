//! Dynamic data structures that do not require a global allocator.
//!
//! The most directly useful might be a local `FixedVec` that requires its element to be neither
//! `Clone`, `Copy` nor `Default` but is simply a normal vector on a locally borrowed chunk of raw
//! memory. For this we will use the `Bump` allocator of [`static-alloc`] which loan us memory from
//! the stack and cleans up by leaving the scope without requiring explicit deallocation. Sort of
//! like `alloca` but safe and pretty.
//!
//! [`static-alloc`]: https://crates.io/crates/static-alloc
//!
//! ```rust
//! use static_alloc::Bump;
//! use without_alloc::{FixedVec, alloc::LocalAllocLeakExt};
//!
//! let mut pool: Bump<[usize; 16]> = Bump::uninit();
//! // Allocate a vector with capacity of 16 from the slab.
//! let mut vector = pool.fixed_vec(16).unwrap();
//!
//! let mut num = 0;
//! // Push a mutable reference, not `Copy` nor `Clone`!
//! vector.push(&mut num);
//!
//! *vector.pop().unwrap() = 4;
//! drop(vector);
//!
//! assert_eq!(num, 4);
//! ```
// Copyright 2019–2020 Aurelia Molzer
#![no_std]
#![deny(missing_docs)]
#![cfg_attr(feature = "nightly_set_ptr_value", feature(set_ptr_value))]

pub mod alloc;
pub mod boxed;
pub mod rc;
pub mod uninit;
pub mod fixed_vec;

pub use boxed::Box;
pub use fixed_vec::FixedVec;
pub use uninit::Uninit;
