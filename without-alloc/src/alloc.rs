//! Allocator extension traits.
use core::{alloc, mem};
use alloc_traits::{NonZeroLayout, LocalAlloc};
use super::{
    boxed::Box,
    fixed_vec::FixedVec,
    rc::Rc,
    uninit::Uninit,
};

/// Values of for some allocation including the [`Uninit`].
///
/// See [`Uninit`] for a better picture of the potential usage of this result.
///
/// [`Uninit`]: ../uninit/struct.Uninit.html
#[derive(Debug)]
pub struct LeakedAllocation<'a, T: ?Sized=()> {
    /// Uninit pointer to the region with specified layout.
    pub uninit: Uninit<'a, T>,
}

/// Leak allocations into uninit regions.
pub trait LocalAllocLeakExt<'alloc>: LocalAlloc<'alloc> {
    /// Leak an allocation with detailed layout.
    ///
    /// Provides an [`Uninit`] wrapping several aspects of initialization in a safe interface,
    /// bound by the lifetime of the reference to the allocator.
    ///
    /// [`Uninit`]: ../uninit/struct.Uninit.html
    fn alloc_layout(&'alloc self, layout: NonZeroLayout)
        -> Option<LeakedAllocation<'alloc>>
    {
        let alloc = self.alloc(layout)?;
        let uninit = unsafe {
            Uninit::from_memory(alloc.ptr, alloc.layout.size().into())
        };

        Some(LeakedAllocation {
            uninit,
        })
    }

    /// Leak an allocation for a specific type.
    ///
    /// It is not yet initialized but provides a safe interface for that initialization. Note that
    /// the type **can** be a ZST in which case a dangling pointer is substituted for the true
    /// allocation.
    ///
    /// ## Usage
    ///
    /// ```
    /// # use static_alloc::Bump;
    /// # use without_alloc::alloc::LocalAllocLeakExt;
    /// use core::cell::{Ref, RefCell};
    ///
    /// let slab: Bump<[Ref<'static, usize>; 1]> = Bump::uninit();
    /// let data = RefCell::new(0xff);
    ///
    /// // We can place a `Ref` here but we did not yet.
    /// let alloc = slab.alloc_t::<Ref<usize>>().unwrap();
    /// let cell_ref = alloc.uninit.init(data.borrow());
    ///
    /// assert_eq!(**cell_ref, 0xff);
    /// ```
    fn alloc_t<V>(&'alloc self) -> Option<LeakedAllocation<'alloc, V>> {
        match NonZeroLayout::new::<V>() {
            None => Some(LeakedAllocation::zst_fake_alloc()),
            Some(alloc) => {
                let allocation = self.alloc_layout(alloc)?;
                let right_type = allocation.cast().unwrap();
                Some(right_type)
            },
        }
    }

    /// Allocate a [`Box`].
    ///
    /// This will allocate some memory with the correct layout for a [`Box`], then place the
    /// provided value into the allocation by constructing an [`Box`].
    ///
    /// [`Box`]: ../boxed/struct.Box.html
    fn boxed<V>(&'alloc self, val: V) -> Option<Box<'alloc, V>> {
        let alloc = self.alloc_t::<V>()?;
        Some(Box::new(val, alloc.uninit))
    }

    /// Allocate a [`FixedVec`].
    ///
    /// This will allocate some memory with the correct layout for a [`FixedVec`] of the given
    /// capacity (in elements) and wrap it. Returns `None` if it is not possible to allocate the
    /// layout.
    ///
    /// [`FixedVec`]: ../fixed_vec/struct.FixedVec.html
    fn fixed_vec<V>(&'alloc self, capacity: usize) -> Option<FixedVec<'alloc, V>> {
        let size = mem::size_of::<V>().checked_mul(capacity)?;
        let layout = alloc::Layout::from_size_align(size, mem::align_of::<V>()).ok()?;

        let uninit = match NonZeroLayout::from_layout(layout.into()) {
            None => Uninit::empty(),
            Some(layout) => {
                let allocation = self.alloc_layout(layout)?;
                let right_type = allocation.cast_slice().unwrap();
                right_type.uninit
            }
        };

        Some(FixedVec::new(uninit))
    }

    /// Allocate an [`Rc`].
    ///
    /// This will allocate some memory with the correct layout for an [`Rc`], then place the
    /// provided value into the allocation by constructing an [`Rc`].
    ///
    /// [`Rc`]: ../rc/struct.Rc.html
    fn rc<V>(&'alloc self, val: V) -> Option<Rc<'alloc, V>> {
        let layout = Rc::<V>::layout();
        // Unwrap since this is surely never an empty layout, always have counter.
        let layout = NonZeroLayout::from_layout(layout.into()).unwrap();
        let alloc = self.alloc_layout(layout)?;
        Some(Rc::new(val, alloc.uninit))
    }

    /// Allocate a slice of a copyable type.
    ///
    /// This will allocate some memory with the same layout as required by the slice, then copy all
    /// values into the new allocation via a byte copy.
    ///
    /// ```
    /// # use static_alloc::Bump;
    /// # use without_alloc::alloc::LocalAllocLeakExt;
    /// let slab: Bump<[usize; 16]> = Bump::uninit();
    /// let data: &[u8] = b"Hello, World!";
    ///
    /// let slice = slab.copy_slice(data).unwrap();
    /// assert_eq!(data, slice);
    /// ```
    fn copy_slice<T: Copy>(&'alloc self, slice: &[T]) -> Option<&'alloc mut [T]> {
        let layout = alloc::Layout::for_value(slice);
        let uninit = match NonZeroLayout::from_layout(layout.into()) {
            None => Uninit::empty(),
            Some(layout) => {
                let allocation = self.alloc_layout(layout)?;
                let right_type = allocation.cast_slice().unwrap();
                right_type.uninit
            }
        };

        unsafe {
            // SAFETY:
            // * the source is trivially valid for reads as it is a slice
            // * the memory is valid for the same layout as slice, so aligned and large enough
            // * both are aligned, uninit due to allocator requirements
            core::ptr::copy(slice.as_ptr(), uninit.as_begin_ptr(), slice.len());
        }

        Some(unsafe {
            // SAFETY: this is a copy of `slice` which is initialized.
            uninit.into_mut()
        })
    }

    /// Allocate a dynamically sized string.
    ///
    /// This will allocate some memory with the same layout as required by the string, then copy
    /// all characters into the new allocation via a byte copy.
    ///
    /// ```
    /// # use static_alloc::Bump;
    /// # use without_alloc::alloc::LocalAllocLeakExt;
    /// let slab: Bump<[u8; 16]> = Bump::uninit();
    /// let data: &str = "Hello, World!";
    ///
    /// let slice = slab.copy_str(data).unwrap();
    /// assert_eq!(data, slice);
    /// ```
    fn copy_str(&'alloc self, st: &str) -> Option<&'alloc str> {
        let bytes = self.copy_slice(st.as_bytes())?;

        Some(unsafe {
            // SAFETY: this is a copy of `st` which is valid utf-8
            core::str::from_utf8_unchecked(bytes)
        })
    }

    /// Allocate a copy of a generic dynamically sized type.
    ///
    /// This method takes a `ManuallyDrop<T>` wrapper instead of a `T` directly. These types are of
    /// course layout compatible and you may soundly cast one reference type to the other. However
    /// this choice forces acknowledgment that the value _must not_ be dropped by the caller
    /// afterwards and makes this reasonably more safe in case of panics.
    ///
    /// Note further that mutable access is however explicitly _not_ required in contrast to
    /// `ManuallyDrop::take`. Otherwise, the caller would have to ensure that the value is not
    /// aliased and actually mutable. Keeping these guarantees often involves moving the value into
    /// a new stack slot which is obviously not possible for dynamically sized values. This
    /// interfaces promises not to overwrite any byte which does not restrict its functionality.
    ///
    /// # Safety
    ///
    /// This is quite unsafe and relies on the nightly `set_ptr_value` feature. Furthermore this
    /// method does not require that `T` is in fact `Copy` as doing so would not be possible for
    /// dynamically sized values. You must either require this bound on the expose interface or
    /// must ensure the source value behind the pointer is not used further, not dropped and
    /// basically discarded. You should act as if `take` had been called on the supplied value.
    ///
    /// # Example
    ///
    /// ```
    /// # use static_alloc::Bump;
    /// # use without_alloc::alloc::LocalAllocLeakExt;
    /// use core::fmt::Debug;
    /// use core::mem::ManuallyDrop;
    ///
    /// let slab: Bump<[u8; 16]> = Bump::uninit();
    /// let debuggable = ManuallyDrop::new(1usize);
    /// let debug = unsafe {
    ///     slab.copy_dst::<dyn Debug>(&debuggable).unwrap()
    /// };
    /// assert_eq!(format!("{:?}", debug), "1");
    /// ```
    #[cfg(feature = "nightly_set_ptr_value")]
    #[allow(unused_unsafe)]
    unsafe fn copy_dst<T: ?Sized>(&'alloc self, val: &core::mem::ManuallyDrop<T>) -> Option<&'alloc mut T> {
        let layout = alloc::Layout::for_value(val);
        let uninit = match NonZeroLayout::from_layout(layout.into()) {
            None => Uninit::invent_for_zst(),
            Some(layout) => self.alloc_layout(layout)?.uninit,
        };

        unsafe {
            // SAFETY:
            // * the source is valid for reads for its own layout
            // * the memory is valid for the same layout as val, so aligned and large enough
            // * both are aligned, uninit due to allocator requirements
            core::ptr::copy(val as *const _ as *const u8, uninit.as_ptr() as *mut u8, layout.size());
        }

        let ptr = val as *const _ as *mut T;
        let ptr = uninit.as_ptr().with_metadata_of(ptr);
        Some(unsafe {
            // SAFETY: The byte copy above put the value into a valid state. Caller promises that
            // we can logically move the value.
            &mut *ptr
        })
    }
}

impl<'alloc, T> LocalAllocLeakExt<'alloc> for T
    where T: LocalAlloc<'alloc>,
{ }

impl<Zst> LeakedAllocation<'_, Zst> {
    /// Invent a new allocation for a zero-sized type (ZST).
    ///
    /// # Panics
    /// This method panics when the type parameter is not a zero sized type.
    pub fn zst_fake_alloc() -> Self {
        LeakedAllocation {
            uninit: Uninit::invent_for_zst(),
        }
    }
}

impl<'a, T> LeakedAllocation<'a, T> {
    fn cast<U>(self) -> Option<LeakedAllocation<'a, U>> {
        Some(LeakedAllocation {
            uninit: self.uninit.cast().ok()?,
        })
    }

    fn cast_slice<U>(self) -> Option<LeakedAllocation<'a, [U]>> {
        Some(LeakedAllocation {
            uninit: self.uninit.cast_slice().ok()?,
        })
    }
}
