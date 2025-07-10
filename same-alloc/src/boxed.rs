use core::mem::MaybeUninit;
use alloc::boxed::Box;
use crate::same::SameLayout;

/// An allocated buffer for types with the same layout.
pub struct SameBox<T> {
    element_buffer: Box<MaybeUninit<T>>,
}

pub struct TempBox<'lt, U> {
    from: &'lt mut dyn DynBufferWith<U>,
    boxed: Option<Box<U>>,
}

/// A compatible wrapper around a `Vec`, meant to be used as wrapping a mutable pointer to one.
/// Here we capture that `SameLayout<T, U>` is inhabited without any indirection layer. This allows
/// us to erase the type parameter of the original vector and swap it for a different one.
#[repr(transparent)]
struct Wrap<T, U> {
    elements: Box<MaybeUninit<T>>,
    marker: SameLayout<T, U>,
}

/// Type-erase way for Vec with elements layout compatible to `U`.
trait DynBufferWith<U> {
    fn swap_internal_with(&mut self, _: &mut Option<Box<U>>);
}

impl<T> Default for SameBox<T> {
    fn default() -> Self {
        SameBox { element_buffer: Box::new(MaybeUninit::uninit()) }
    }
}

impl<T> Drop for TempBox<'_, T> {
    fn drop(&mut self) {
        self.from.swap_internal_with(&mut self.boxed);
    }
}

impl<T> core::ops::Deref for TempBox<'_, T> {
    type Target = Box<T>;

    fn deref(&self) -> &Box<T> {
        self.boxed.as_ref().unwrap()
    }
}

impl<T> core::ops::DerefMut for TempBox<'_, T> {
    fn deref_mut(&mut self) -> &mut Box<T> {
        self.boxed.as_mut().unwrap()
    }
}

impl<T, U> DynBufferWith<U> for Wrap<T, U> {
    fn swap_internal_with(&mut self, v: &mut Option<Box<U>>) {
        let temp = core::mem::take(v).unwrap();
        let (v, mut temp) = self.marker.transpose().deinit_box(temp);
        drop(v);
        core::mem::swap(&mut temp, &mut self.elements);
    }
}
