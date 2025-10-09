use alloc::vec::Vec;
use crate::same::SameLayout;

/// A dynamically sized buffer for vectors of types with the same layout.
pub struct VecBuffer<T> {
    element_buffer: Vec<T>,
}

/// A temporary view on a VecBuffer, with a different element type.
pub struct TempVec<'lt, T> {
    from: &'lt mut dyn DynBufferWith<T>,
    vec: Vec<T>,
}

/// Type-erase way for Vec with elements layout compatible to `T`.
trait DynBufferWith<T> {
    fn swap_internal_with(&mut self, _: &mut Vec<T>);
}

/// A compatible wrapper around a `Vec`, meant to be used as wrapping a mutable pointer to one.
/// Here we capture that `SameLayout<T, U>` is inhabited without any indirection layer. This allows
/// us to erase the type parameter of the original vector and swap it for a different one.
#[repr(transparent)]
struct Wrap<T, U> {
    elements: alloc::vec::Vec<T>,
    marker: SameLayout<T, U>,
}

impl<T> VecBuffer<T> {
    /// Create an empty buffer.
    pub fn new() -> Self {
        VecBuffer::default()
    }

    /// Create a buffer with a pre-defined capacity.
    ///
    /// This buffer will not need to reallocate until the element count required for any temporary
    /// vector exceeds this number of elements.
    pub fn with_capacity(cap: usize) -> Self {
        VecBuffer {
            element_buffer: Vec::with_capacity(cap),
        }
    }

    /// Use the allocated buffer for a compatible type of elements.
    ///
    /// When the temporary view is dropped the allocation is returned to the buffer. This means its
    /// capacity might be automatically increased, or decreased, based on the used of the vector.
    pub fn use_for<U>(&mut self, marker: SameLayout<T, U>) -> TempVec<'_, U> {
        let from = Wrap::new(&mut self.element_buffer, marker);
        let elements = core::mem::take(&mut from.elements);
        let vec = from.marker.forget_vec(elements);
        TempVec { from, vec, }
    }
}

impl<T> From<Vec<T>> for VecBuffer<T> {
    fn from(mut element_buffer: Vec<T>) -> Self {
        element_buffer.clear();
        VecBuffer { element_buffer }
    }
}

impl<T> Default for VecBuffer<T> {
    fn default() -> Self {
        VecBuffer { element_buffer: Vec::new() }
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
