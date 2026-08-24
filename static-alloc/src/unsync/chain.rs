//! This module defines a simple bump allocator.
//! The allocator is not thread safe.
use core::{
    alloc::{Layout, LayoutErr},
    cell::Cell,
    mem::MaybeUninit,
    ptr::{self, NonNull},
};

use alloc::{alloc::alloc_zeroed, boxed::Box};

use crate::bump::Failure;
use crate::leaked::LeakBox;
use crate::unsync::bump::MemBump;

/// An error representing an error while construction
/// a [`Chain`].
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct TryNewError {
    inner: RawAllocError,
}

impl TryNewError {
    /// Returns the allocation size of a `Chain`
    /// that couldn't be allocated.
    pub const fn allocation_size(&self) -> usize {
        self.inner.allocation_size()
    }
}

type LinkPtr = Option<NonNull<Link>>;

struct Link {
    /// A pointer to the next node within the list.
    /// This is wrapped in a Cell, so we can modify
    /// this field with just an &self reference.
    next: Cell<LinkPtr>,
    /// The bump allocator of this link.
    bump: MemBump,
}

/// A `Chain` is a simple bump allocator, that draws
/// it's memory from another allocator. Chain allocators
/// can be chained together using [`Chain::chain`].
pub struct Chain {
    /// The root. Critically, we must not deallocate before all borrows on self have ended, in
    /// other words until its destructor.
    root: Cell<LinkPtr>,
}

impl Chain {
    /// Creates a new `Chain` that has a capacity of `size`
    /// bytes.
    pub fn new(size: usize) -> Result<Self, TryNewError> {
        let link = Link::alloc(size).map_err(|e| TryNewError { inner: e })?;
        Ok(Chain {
            root: Cell::new(Some(link)),
        })
    }

    /// Attempts to allocate `elem` within the allocator.
    pub fn bump_box<'bump, T: 'bump>(
        &'bump self,
    ) -> Result<LeakBox<'bump, MaybeUninit<T>>, Failure> {
        let root = self.root().ok_or(Failure::Exhausted)?;
        root.as_bump().bump_box()
    }

    /// Chains `self` together with `new`.
    ///
    /// Following allocations will first be allocated from `new`.
    ///
    /// Note that this will drop all but the first link from `new`.
    pub fn chain(&self, new: Chain) {
        // We can't drop our own, but we can drop the tail of `new`.
        let self_bump = self.root.take();

        match new.root() {
            None => self.root.set(self_bump),
            Some(root) => {
                unsafe { root.set_next(self_bump) };
                self.root.set(new.root.take())
            }
        }
    }

    /// Returns the capacity of this `Chain`.
    /// This is how many *bytes* in total can
    /// be allocated within this `Chain`.
    pub fn capacity(&self) -> usize {
        match self.root() {
            None => 0,
            Some(root) => root.as_bump().capacity(),
        }
    }

    /// Returns the remaining capacity of this `Chain`.
    /// This is how many more *bytes* can be allocated
    /// within this `Chain`.
    pub fn remaining_capacity(&self) -> usize {
        match self.root() {
            None => 0,
            Some(root) => self.capacity() - root.as_bump().level().0,
        }
    }

    fn root(&self) -> Option<&Link> {
        unsafe {
            let bump_ptr = self.root.get()?.as_ptr();
            Some(&*bump_ptr)
        }
    }
}

/// A type representing a failure while allocating
/// a `MemBump`.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub(crate) struct RawAllocError {
    allocation_size: usize,
    kind: RawAllocFailure,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
enum RawAllocFailure {
    Exhausted,
    Layout,
}

impl Link {
    /// Override the next pointer.
    ///
    /// ## Safety
    /// It must point to a valid link. Furthermore, the old link is dropped!
    pub(crate) unsafe fn set_next(&self, next: LinkPtr) {
        if let Some(next) = self.next.replace(next) {
            let _ = Box::from_raw(next.as_ptr());
        }
    }

    /// Take over the control over the tail.
    pub(crate) fn take_next(&self) -> Option<Box<Link>> {
        let ptr = self.next.take()?;
        Some(unsafe { Box::from_raw(ptr.as_ptr()) })
    }

    pub(crate) fn as_bump(&self) -> &MemBump {
        &self.bump
    }

    pub(crate) fn layout_from_size(size: usize) -> Result<Layout, LayoutErr> {
        Layout::new::<Cell<LinkPtr>>()
            .extend(MemBump::layout_from_size(size)?)
            .map(|layout| layout.0)
    }

    unsafe fn alloc_raw(layout: Layout) -> Result<NonNull<u8>, RawAllocError> {
        let ptr = alloc_zeroed(layout);
        NonNull::new(ptr)
            .ok_or_else(|| RawAllocError::new(layout.size(), RawAllocFailure::Exhausted))
    }

    /// Allocates a MemBump and returns it.
    pub(crate) fn alloc(capacity: usize) -> Result<NonNull<Self>, RawAllocError> {
        let layout = Self::layout_from_size(capacity)
            .map_err(|_| RawAllocError::new(capacity, RawAllocFailure::Layout))?;

        unsafe {
            let raw = Link::alloc_raw(layout)?;
            let raw_mut: *mut [Cell<MaybeUninit<u8>>] =
                ptr::slice_from_raw_parts_mut(raw.cast().as_ptr(), capacity);
            Ok(NonNull::new_unchecked(raw_mut as *mut Link))
        }
    }
}

impl RawAllocError {
    const fn new(allocation_size: usize, kind: RawAllocFailure) -> Self {
        Self {
            allocation_size,
            kind,
        }
    }

    pub(crate) const fn allocation_size(&self) -> usize {
        self.allocation_size
    }
}

/// Chain drops iteratively, so that we do not stack overflow.
impl Drop for Chain {
    fn drop(&mut self) {
        let mut current = self.root.take();
        while let Some(non_null) = current {
            // Drop as a box.
            let link = unsafe { Box::from_raw(non_null.as_ptr()) };
            current = link.next.take();
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let mut current = self.take_next();
        while let Some(link) = current {
            current = link.take_next();
        }
    }
}
