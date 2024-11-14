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
//!     buf: &mut SameVec<*const str>,
//! ) -> &'names str {
//!     let mut names = buf.use_for(same::for_ref());
//!     names.extend(unparsed.split(' '));
//!     let idx = names.len() / 2;
//!     *names.select_nth_unstable(idx).1
//! }
//! ```
//!
#![no_std]
extern crate alloc;

pub mod same;

use alloc::vec::Vec;
pub use crate::same::SameLayout;

/// A dynamically sized buffer for types with the same layout.
pub struct SameVec<T> {
    elements: Vec<T>,
}

/// A temporary view on a SameVec, with a different element type.
pub struct TempVec<'lt, T> {
    from: &'lt mut dyn DynBufferWith<T>,
    vec: Vec<T>,
}

impl<T> SameVec<T> {
    /// Create an empty buffer.
    pub fn new() -> Self {
        SameVec::default()
    }

    /// Create a buffer with a pre-defined capacity.
    ///
    /// This buffer will not need to reallocate until the element count required for any temporary
    /// vector exceeds this number of elements.
    pub fn with_capacity(cap: usize) -> Self {
        SameVec {
            elements: Vec::with_capacity(cap),
        }
    }

    /// Use the allocated buffer for a compatible type of elements.
    ///
    /// When the temporary view is dropped the allocation is returned to the buffer. This means its
    /// capacity might be automatically increased, or decreased, based on the used of the vector.
    pub fn use_for<U>(&mut self, marker: SameLayout<T, U>) -> TempVec<'_, U> {
        let from = Wrap::new(&mut self.elements, marker);
        let elements = core::mem::take(&mut from.elements);
        let vec = from.marker.forget_vec(elements);
        TempVec { from, vec, }
    }
}

impl<T> Default for SameVec<T> {
    fn default() -> Self {
        SameVec { elements: Vec::new() }
    }
}

impl<T> Drop for TempVec<'_, T> {
    fn drop(&mut self) {
        self.from.swap_internal_with(&mut self.vec);
    }
}

impl<T> core::ops::Deref for TempVec<'_, T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.vec
    }
}

impl<T> core::ops::DerefMut for TempVec<'_, T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.vec
    }
}

struct Wrap<T, U> {
    marker: SameLayout<T, U>,
    elements: alloc::vec::Vec<T>,
}

/// Type-erase way for Vec with elements layout compatible to `T`.
trait DynBufferWith<T> {
    fn swap_internal_with(&mut self, _: &mut Vec<T>);
}

impl<T, U> Wrap<T, U> {
    fn new(vec: &mut Vec<T>, _: SameLayout<T, U>) -> &mut Self {
        unsafe { &mut *(vec as *mut _ as *mut Wrap<T, U>) }
    }
}

impl<T, U> DynBufferWith<U> for Wrap<T, U> {
    fn swap_internal_with(&mut self, v: &mut Vec<U>) {
        let mut temp = core::mem::take(v);

        temp.clear();
        let mut temp = self.marker.transpose().forget_vec(temp);
        core::mem::swap(&mut temp, &mut self.elements);

        temp.clear();
        *v = self.marker.forget_vec(temp);
    }
}
