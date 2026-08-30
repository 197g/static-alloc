use core::ptr::{copy_nonoverlapping, write_bytes, NonNull};

use crate::NonZeroLayout;
use crate::LocalAlloc;

/// An allocation without tracked lifetime.
#[derive(Copy, Clone)]
pub struct Allocation {
    ptr: NonNull<u8>,
    layout: NonZeroLayout,
}

/// A references to an allocator.
///
/// Allocations created from this instance are valid for the lifetime of `self`. That is one must
/// be able to move the `self` and all allocations remain valid. In particular, a reference to a
/// [`LocalAlloc`] is an `AllocRef` as it is an immutable pin.
///
/// An allocation is said to be live when it is valid and has not been passed to `dealloc` and has
/// not been passed to `realloc` that returned `Some(_)`.
///
/// Note that all methods require being passed some existing allocation, even if they do not
/// consume it. See `InitialAllocRef` for those that do not.
///
/// [`LocalAlloc`]: trait.LocalAlloc.html
pub unsafe trait AllocRef {
    /// Allocate one block of memory.
    ///
    /// The `existing` allocation can be used by the `AllocRef` to uniquely identify the allocator
    /// that is used internaly. It must be valid for the same `AllocRef`.
    ///
    /// The callee guarantees that a successful return contains a pointer that fits the layout
    /// requested by the caller and the layout in the struct is the same as requested.
    unsafe fn alloc_from(&mut self, existing: Allocation, layout: NonZeroLayout) -> Option<Allocation>;

    /// Allocate a block of memory initialized with zeros.
    ///
    /// The callee guarantees that a successful return contains a pointer that fits the layout
    /// requested by the caller and the layout in the struct is the same as requested.
    unsafe fn alloc_zeroed_from(&mut self, existing: Allocation, layout: NonZeroLayout)
        -> Option<Allocation> 
    {
        let allocation = self.alloc_from(existing, layout)?;
        write_bytes(allocation.ptr.as_ptr(), 0u8, allocation.layout.size().into());
        Some(allocation)
    }

    /// Deallocate a block previously allocated.
    /// # Safety
    /// The caller must ensure that:
    /// * `alloc` has been previously returned from a call to `alloc`.
    /// * There are no more pointer to the allocation.
    unsafe fn dealloc(&mut self, alloc: Allocation);

    /// Change the layout of a block previously allocated.
    ///
    /// The callee guarantees that a successful return contains a pointer that it fits the layout
    /// requested by the caller and the contiguous region of bytes, starting at the pointer and
    /// with the size of the returned layout, is initialized with the prefix of the previous
    /// allocation that is still valid, and that the layout in the returned struct is the same as
    /// requested.
    unsafe fn realloc(&mut self, alloc: Allocation, layout: NonZeroLayout)
        -> Option<Allocation>
    {
        let new_alloc = self.alloc_from(alloc, layout)?;
        copy_nonoverlapping(
            alloc.ptr.as_ptr(),
            new_alloc.ptr.as_ptr(),
            layout.size().min(alloc.layout.size()).into());
        self.dealloc(alloc);
        Some(new_alloc)
    }
}

/// A trait for an `AllocRef` that requires no existing allocation to allocate.
pub unsafe trait InitialAllocRef: AllocRef {
    /// Allocate one block of memory.
    ///
    /// The callee guarantees that a successful return contains a pointer that fits the layout
    /// requested by the caller and the layout in the struct is the same as requested.
    fn alloc(&mut self, layout: NonZeroLayout) -> Option<Allocation>;

    /// Allocate a block of memory initialized with zeros.
    ///
    /// The callee guarantees that a successful return contains a pointer that fits the layout
    /// requested by the caller and the layout in the struct is the same as requested.
    fn alloc_zeroed(&mut self, layout: NonZeroLayout)
        -> Option<Allocation> 
    {
        let allocation = InitialAllocRef::alloc(self, layout)?;
        unsafe {
            write_bytes(allocation.ptr.as_ptr(), 0u8, allocation.layout.size().into());
        }
        Some(allocation)
    }
}

impl Allocation {
    pub(crate) fn from_local(alloc: crate::local::Allocation) -> Self {
        Allocation {
            ptr: alloc.ptr,
            layout: alloc.layout,
        }
    }

    pub(crate) fn into_local<'lt>(self) -> crate::local::Allocation<'lt> {
        crate::local::Allocation {
            ptr: self.ptr,
            layout: self.layout,
            lifetime: crate::AllocTime::default(),
        }
    }
}

/// A reference is an `AllocRef`.
///
/// Note the lifetime of the references is exactly the same as the lifetime of the trait. If the
/// reference outlived the parameter then the returned allocations would not live long enough.
/// Conversely, if the parameter outlived the lifetime then it would not be possible to construct
/// the reference necessary for the trait.
unsafe impl<'rf, T> AllocRef for &'rf T
    where T: LocalAlloc<'rf>
{
    unsafe fn alloc_from(&mut self, _: Allocation, layout: NonZeroLayout) -> Option<Allocation> {
        InitialAllocRef::alloc(self, layout)
    }

    unsafe fn alloc_zeroed_from(&mut self, _: Allocation, layout: NonZeroLayout)
        -> Option<Allocation> 
    {
        InitialAllocRef::alloc_zeroed(self, layout)
    }

    unsafe fn dealloc(&mut self, alloc: Allocation) {
        LocalAlloc::<'rf>::dealloc(*self, alloc.into_local())
    }

    unsafe fn realloc(&mut self, alloc: Allocation, layout: NonZeroLayout)
        -> Option<Allocation>
    {
        LocalAlloc::<'rf>::realloc(*self, alloc.into_local(), layout)
            .map(Allocation::from_local)
    }
}

unsafe impl<'rf, T> InitialAllocRef for &'rf T
    where T: LocalAlloc<'rf>
{
    fn alloc(&mut self, layout: NonZeroLayout) -> Option<Allocation> {
        LocalAlloc::<'rf>::alloc(*self, layout).map(Allocation::from_local)
    }

    fn alloc_zeroed(&mut self, layout: NonZeroLayout) -> Option<Allocation> {
        LocalAlloc::<'rf>::alloc_zeroed(*self, layout).map(Allocation::from_local)
    }
}
