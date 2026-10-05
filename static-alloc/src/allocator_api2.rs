use crate::{
    bump::{Bump, BumpSlice, BumpView, Level},
    unsync,
};

use allocator_api2::alloc::{AllocError, Allocator, Layout};
use core::{cell, ptr::NonNull};

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

/// Wrapper around an allocator, that gathers stats about the underlying store while allocating.
///
/// The `allocator_api2` design intentionally avoids generics in its result types. Allocations in
/// the bump arena however provide some auxiliary information such as the usage level of the bump
/// before and after an allocation (at least a snapshot of it for a Sync allocator). This
/// information can be especially useful in case of an allocation failure but gather it separately
/// would be racy. Allocating with this wrapper avoids the race.
///
/// For these use cases you can replace the allocator handle with this wrapper. The wrapper is
/// [equivalent] to the underlying allocator.
///
/// [equivalent]: https://doc.rust-lang.org/stable/std/alloc/trait.Allocator.html#equivalent-allocators
///
/// # Usage
///
/// ```
/// use static_alloc::allocator_api2::AllocatorStats;
/// use static_alloc::bump::Bump;
/// ```
///
/// # Design
///
/// Interestingly, we could also provide per-call allocator configuration. For a `Bump` shared
/// between threads for instance there is an inner loop around a fallible CAS. A parameter could
/// constrain this loop to exit early on a case-by-case basis. It does not make too much sense to
/// enforce it for all allocations in the arena. You would often prefer to use a non-racy
/// thread-local arena in those cases (please open an issue if you have a use case). A handle
/// wrapper on the other hand would provide surgical amounts of control.
pub struct AllocatorStats<'lt, A: ?Sized> {
    observed_level: cell::Cell<Level>,
    /// We could have a type-erased other type where this is a direct `BumpView` instead. Making
    /// this work would require complicated generics though.
    inner: &'lt A,
}

impl<'lt, A: ?Sized> AllocatorStats<'lt, A> {
    /// Wrap an arena to gather stats.
    ///
    /// This does not constrain the type `A`. However, it can only be sensibly used with a
    /// [`Bump`](`crate::bump::Bump`) or [`Bump`](`crate::unsync::Bump`) and their type erased
    /// slices.
    pub fn new(arena: &'lt A) -> Self {
        AllocatorStats {
            observed_level: cell::Cell::new(Level(0)),
            inner: arena,
        }
    }

    /// Query the consumed level observed by the last allocation call.
    ///
    /// The value is updated both on success and failure.
    ///
    /// # Usage
    ///
    /// You will get information about the consumed bytes with each attempted allocation.
    ///
    /// ```
    /// use static_alloc::allocator_api2::AllocatorStats;
    /// use static_alloc::bump::Bump;
    /// use allocator_api2::vec::Vec;
    ///
    /// let arena =  Bump::<[usize; 4]>::uninit();
    /// let hdl = AllocatorStats::new(&arena);
    ///
    /// let mut vec = Vec::<u32, _>::with_capacity_in(1, hdl);
    /// assert_eq!(vec.allocator().last_observed_level().byte_offset(), 4);
    ///
    /// // This will not fit, and fail to allocate.
    /// vec.try_reserve(32);
    /// assert_eq!(vec.allocator().last_observed_level().byte_offset(), 4);
    ///
    /// // This will, reallocating the vector.
    /// vec.try_reserve_exact(2);
    /// assert_eq!(vec.allocator().last_observed_level().byte_offset(), 12);
    /// ```
    ///
    /// The information is updated also when the arena is used concurrently for another purpose,
    /// too.
    ///
    /// ```
    /// use static_alloc::allocator_api2::AllocatorStats;
    /// use static_alloc::bump::Bump;
    /// use allocator_api2::vec::Vec;
    ///
    /// let arena =  Bump::<[usize; 4]>::uninit();
    /// let hdl = AllocatorStats::new(&arena);
    ///
    /// let mut vec = Vec::<u32, _>::new_in(hdl);
    /// let _ = arena.leak(0u32);
    ///
    /// // Afterwards exactly three `u32` allocator, the one leaked and the 2 reserved.
    /// vec.try_reserve_exact(2);
    /// assert_eq!(vec.allocator().last_observed_level().byte_offset(), 12);
    /// ```
    pub fn last_observed_level(&self) -> Level {
        self.observed_level.get()
    }
}

unsafe impl<T> Allocator for AllocatorStats<'_, Bump<T>> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let len = layout.size();
        match self.inner.as_view().try_alloc(layout) {
            Ok(allocation) => {
                self.observed_level.set(allocation.level);
                Ok(NonNull::slice_from_raw_parts(allocation.ptr, len))
            }
            Err(level) => {
                self.observed_level.set(level);
                Err(AllocError)
            }
        }
    }

    unsafe fn deallocate(&self, _: NonNull<u8>, _: Layout) {}

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // Safety: forwarding arguments, i.e  requirements, to an equivalent allocator.
        unsafe { Allocator::shrink(&self.inner, ptr, old_layout, new_layout) }
    }
}
