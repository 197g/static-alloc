use crate::{
    bump::{Bump, BumpSlice, BumpView},
    unsync,
};

use allocator_api2::alloc::{AllocError, Allocator, Layout};
use core::ptr::NonNull;

unsafe impl<T> Allocator for Bump<T> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        Allocator::allocate(&self.as_view(), layout)
    }

    unsafe fn deallocate(&self, _: NonNull<u8>, _: Layout) {}

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // Safety: passing along requirements. These two allocators serve the same allocations, a
        // property we permit for these two of our own types.
        unsafe { Allocator::shrink(&self.as_view(), ptr, old_layout, new_layout) }
    }
}

unsafe impl Allocator for BumpSlice {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        Allocator::allocate(&self.as_view(), layout)
    }

    unsafe fn deallocate(&self, _: NonNull<u8>, _: Layout) {}

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // Safety: passing along requirements. These two allocators serve the same allocations, a
        // property we permit for these two of our own types.
        unsafe { Allocator::shrink(&self.as_view(), ptr, old_layout, new_layout) }
    }
}

unsafe impl Allocator for BumpView<'_> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let len = layout.size();
        match self.get_layout(layout) {
            None => Err(AllocError),
            Some(allocation) => Ok(NonNull::slice_from_raw_parts(allocation.ptr, len)),
        }
    }

    unsafe fn deallocate(&self, _: NonNull<u8>, _: Layout) {}

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // Safety: Caller guarantees `ptr` was allocated from `self` (or equivalent, for transitive
        // use of this) which requires it to be valid and described by `old_layout`.
        unsafe { shrink_in_place(ptr, old_layout, new_layout) }
    }
}

unsafe impl<T> Allocator for unsync::Bump<T> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        <unsync::BumpSlice as Allocator>::allocate(self, layout)
    }

    unsafe fn deallocate(&self, _: NonNull<u8>, _: Layout) {}

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // Safety: passing along requirements. These two allocators serve the same allocations, a
        // property we permit for these two of our own types.
        unsafe { <unsync::BumpSlice as Allocator>::shrink(self, ptr, old_layout, new_layout) }
    }
}

unsafe impl Allocator for unsync::BumpSlice {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let len = layout.size();
        match self.alloc(layout) {
            None => Err(AllocError),
            Some(allocation) => Ok(NonNull::slice_from_raw_parts(allocation, len)),
        }
    }

    unsafe fn deallocate(&self, _: NonNull<u8>, _: Layout) {}

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // Safety: Caller guarantees `ptr` was allocated from `self` (or equivalent, for transitive
        // use of this) which requires it to be valid and described by `old_layout`.
        unsafe { shrink_in_place(ptr, old_layout, new_layout) }
    }
}

/// Safety: caller must only call this on `ptr` point to a valid allocation with the fitting layout
/// `old_layout`. Returns a derived pointer into the same allocation on success.
unsafe fn shrink_in_place(
    ptr: NonNull<u8>,
    old_layout: Layout,
    new_layout: Layout,
) -> Result<NonNull<[u8]>, AllocError> {
    debug_assert!(new_layout.size() <= old_layout.size());
    let len = new_layout.size();

    let offset = ptr.align_offset(new_layout.align());

    if offset > 0 {
        if old_layout
            .size()
            .checked_sub(offset)
            .is_none_or(|n| n < len)
        {
            // Won't fit in-place. Sorry.
            return Err(AllocError);
        }

        // Safety: in-bounds as we just verified that old layout has at least as many bytes as
        // offset, and the caller was required to pass a live allocation with corresponding
        // layout; implying that it also has that many bytes.
        let dst = unsafe { ptr.byte_add(offset) };
        // Safety: just verified that layout has at least `len` bytes after the offset so `dst`
        // also has provenance according to the caller's requirements.
        unsafe { ptr.copy_to(dst, len) };
        dst
    } else {
        ptr
    };

    Ok(NonNull::slice_from_raw_parts(ptr, len))
}
