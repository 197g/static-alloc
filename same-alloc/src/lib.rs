//! The `SameVec` makes it possible to re-use allocations across multiple invocations of
//! zero-copy parsers.
//!
//! This crate provides an allocated buffer that can be used by vectors of
//! different element types, as long as they have the same layout. Most prominently
//! this allows use of one buffer where the element type depends on a function
//! local lifetime. The required vector type would be impossible to name outside
//! the function.
//!
//! # Example
//!
//! ```rust
//! # use same_alloc::{same, VecBuffer};
//!
//! fn select_median_name(unparsed: &str) -> &str {
//!     // Problem: This type depends on the lifetime parameter. Ergo, we can not normally store
//!     // _one_vector in the surrounding function, and instead need to allocate here a new one.
//!     let mut names: Vec<_> = unparsed.split(' ').collect();
//!     let idx = names.len() / 2;
//!     *names.select_nth_unstable(idx).1
//! }
//!
//! fn select_median_name_with_buffer<'names>(
//!     unparsed: &'names str,
//!     buf: &mut VecBuffer<*const str>,
//! ) -> &'names str {
//!     let mut names = buf.use_for(same::for_ref());
//!     names.extend(unparsed.split(' '));
//!     let idx = names.len() / 2;
//!     *names.select_nth_unstable(idx).1
//! }
//! ```
#![no_std]
extern crate alloc;

pub mod same;
pub mod vec;

pub use vec::{VecBuffer, TempVec};
pub type SameVec<T> = VecBuffer<T>;
