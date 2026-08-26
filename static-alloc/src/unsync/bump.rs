use core::{
    alloc::{Layout, LayoutError},
    cell::{Cell, UnsafeCell},
    mem::{self, MaybeUninit},
    ops,
    ptr::{self, NonNull},
};

use alloc_traits::AllocTime;

use crate::bump::{Allocation, Failure, Level};
use crate::leaked::LeakBox;

/// A bump allocator whose storage capacity and alignment is given by `T`.
///
/// This type dereferences to the generic `BumpSlice` that implements the allocation behavior. Note
/// that `BumpSlice` is an unsized type. In contrast this type is sized so it is possible to
/// construct an instance on the stack or leak one from another bump allocator such as a global
/// one.
///
/// # Usage
///
/// For on-stack usage this works the same as [`Bump`]. Note that it is not possible to use as a
/// global allocator though.
///
/// [`Bump`]: ../bump/struct.Bump.html
///
/// One interesting use case for this struct is as scratch space for subroutines. This ensures good
/// locality and cache usage. It can also allows such subroutines to use a dynamic amount of space
/// without the need to actually allocate. Contrary to other methods where the caller provides some
/// preallocated memory it will also not 'leak' private data types. This could be used in handling
/// web requests.
///
/// ```
/// use static_alloc::unsync::Bump;
/// # use static_alloc::unsync::BumpSlice;
/// # fn subroutine_one(_: &BumpSlice) {}
/// # fn subroutine_two(_: &BumpSlice) {}
///
/// let mut stack_buffer: Bump<[usize; 64]> = Bump::uninit();
/// subroutine_one(&stack_buffer);
/// stack_buffer.reset();
/// subroutine_two(&stack_buffer);
/// ```
///
/// Note that you need not use the stack for the `Bump` itself. Indeed, you could allocate a large
/// contiguous instance from the global (synchronized) allocator and then do subsequent allocations
/// from the `Bump` you've obtained. This avoids potential contention on a lock of the global
/// allocator, especially in case you must do many small allocations. If you're writing an
/// allocator yourself you might use this technique as an internal optimization.
///
#[cfg_attr(feature = "alloc", doc = "```")]
#[cfg_attr(not(feature = "alloc"), doc = "```ignore")]
/// use static_alloc::unsync::{Bump, BumpSlice};
/// # struct Request;
/// # fn handle_request(_: &BumpSlice, _: Request) {}
/// # fn iterate_recv() -> Option<Request> { None }
/// let mut local_page: Box<Bump<[u64; 64]>> = Box::new(Bump::uninit());
///
/// for request in iterate_recv() {
///     local_page.reset();
///     handle_request(&local_page, request);
/// }
/// ```
///
/// ## Coercion into [`BumpSlice`]
///
/// This allocator nominally implements [`Deref`](core::ops::Deref) into [`BumpSlice`]. However, the
/// layout of these two structs is equivalent only for types that have at most an alignment of
/// [`usize`] (e.g. arrays of `u8`, `u16`, or more integers depending on the platform pointer size).
///
/// Warning: An attempt to use this dereference with an invalid type will trigger a
/// post-monomorphization error! This choice was made to avoid complicated encoding of the
/// precondition into a viral trait bound and considering you're likely to use very concrete
/// instances that either work, or would have been UB.
///
/// For instance, this will *fail* to compile:
///
/// ```compile_fail
/// use static_alloc::unsync::{Bump, BumpSlice};
///
/// #[repr(align(32))]
/// struct HighlyAligned([u8; 128]);
///
/// let mut arena: Bump<HighlyAligned> = Bump::uninit();
/// // Fails here, attempting to resolve `impl Deref for Bump<HighlyAligned>`.
/// let _ = arena.get::<u32>();
/// ```
#[repr(C)]
pub struct Bump<T> {
    /// The index used in allocation.
    header: Header,
    /// The backing storage for raw allocated data.
    _data: UnsafeCell<MaybeUninit<T>>,
    // Warning: when changing the data layout, you must change `BumpSlice` as well.
}

/// An error used when one could not re-use raw memory for a bump allocator.
#[derive(Debug)]
pub struct FromMemError {
    _inner: (),
}

/// A dynamically sized allocation block in which any type can be allocated.
#[repr(C)]
pub struct BumpSlice {
    header: Header,

    /// The data slice of a node. This slice
    /// may be of any arbitrary size. We use
    /// a Cell<MaybeUninit> to allow modification
    /// trough a &self reference, and to allow
    /// writing uninit padding bytes.
    /// Note that the underlying memory is in one
    /// contiguous `UnsafeCell`, it's only represented
    /// here to make it easier to slice.
    data: UnsafeCell<[MaybeUninit<u8>]>,
}

impl<T> Bump<T> {
    /// Create an allocator with uninitialized memory.
    ///
    /// All allocations coming from the allocator will need to be initialized manually.
    pub fn uninit() -> Self {
        Bump {
            header: Header::empty(),
            _data: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Create an allocator with zeroed memory.
    ///
    /// The caller can rely on all allocations to be zeroed.
    pub fn zeroed() -> Self {
        Bump {
            header: Header::empty(),
            _data: UnsafeCell::new(MaybeUninit::zeroed()),
        }
    }

    /// Construct a bump allocator into an uninitialized memory location.
    ///
    /// This fills in only a constant sized header. The rest of the allocation is left-as, i.e. if
    /// remains initialized exactly in those spots the caller may have initialized with external
    /// means.
    ///
    /// Note that this method is `const` (though this is not particularly useful yet as of `0.3.0`).
    ///
    /// # Usage
    ///
    /// This method allows `Bump` to be used together with interfaces that require an outer
    /// `MaybeUninit` for their safety proofs, e.g. [`Box::new_uninit_slice`].
    ///
    /// ```
    /// # use static_alloc::unsync::Bump;
    /// type Allocator = Bump<[u32; 128]>;
    ///
    /// # let num_components = 4;
    /// // 4 independent allocators, e.g. for four components of your software.
    /// // Still guaranteed to live in consecutive memory.
    /// let mut allocators = Box::<[Allocator]>::new_uninit_slice(num_components);
    ///
    /// // The index here might be a runtime address.
    /// // Now this arena can be used without initializing the others already.
    /// let c0 = Bump::from_maybe_uninit(&mut allocators[0]);
    /// // Etc. Use this temporary stack allocator.
    /// let _ = c0.bump_box::<usize>();
    /// ```
    pub const fn from_maybe_uninit(data: &mut MaybeUninit<Self>) -> &'_ mut Self {
        // Safety: dereferencing a pointer into a `&mut MaybeUninit`.
        let header = unsafe { &raw mut (*data.as_mut_ptr()).header };
        // Safety: pointer points into a `MaybeUninit` which we have derived a mutable provenance
        // pointer into.
        unsafe { core::ptr::write(header, Header::empty()) };
        // Safety: only the header field requires initialization. The storage is a no-op.
        unsafe { data.assume_init_mut() }
    }
}

#[cfg(feature = "alloc")]
impl BumpSlice {
    /// Allocate some space to use for a bump allocator.
    pub fn new(capacity: usize) -> alloc::boxed::Box<Self> {
        let layout = Self::layout_from_size(capacity).expect("Bad layout");
        // NOTE: if std allows, we'd very much like to use `Vec<Header>::try_with_capacity` here
        // instead. But currently we can't leak that into a `Box<[MaybeUninit<Header>]>` which makes
        // it unfortunately inert.
        let ptr = NonNull::new(unsafe { alloc::alloc::alloc(layout) })
            .unwrap_or_else(|| alloc::alloc::handle_alloc_error(layout));
        let ptr = ptr::slice_from_raw_parts_mut(ptr.as_ptr(), capacity);
        // Safety: `layout_from_size` ensures at least the header fits, and the allocation was
        // obviously successful as just seen.
        unsafe { ptr::write(ptr as *mut Header, Header::empty()) };
        unsafe { alloc::boxed::Box::from_raw(ptr as *mut BumpSlice) }
    }
}

impl BumpSlice {
    /// Initialize a bump allocator from existing memory.
    ///
    /// # Usage
    ///
    /// ```
    /// use core::mem::MaybeUninit;
    /// use static_alloc::unsync::BumpSlice;
    ///
    /// let mut backing = [MaybeUninit::new(0); 128];
    /// let alloc = BumpSlice::from_mem(&mut backing)?;
    ///
    /// # Ok::<(), static_alloc::unsync::FromMemError>(())
    /// ```
    pub fn from_mem(mem: &mut [MaybeUninit<u8>]) -> Result<LeakBox<'_, Self>, FromMemError> {
        let header = Self::header_layout();
        let offset = mem.as_ptr().align_offset(header.align());
        // Align the memory for the header.
        let mem = mem.get_mut(offset..).ok_or(FromMemError { _inner: () })?;
        let hdr = mem
            .get_mut(..header.size())
            .ok_or(FromMemError { _inner: () })?;
        // Safety: `mem` is a mutable ref, and we just verified the size and align. We'd consider
        // MaybeUninit::as_bytes` and copy instead but it's not stable.
        unsafe { ptr::write(hdr.as_mut_ptr().cast(), Header::empty()) };
        // Safety: we just verified the size, and pivoted to the correct alignment.
        Ok(unsafe { Self::from_mem_unchecked(mem) })
    }

    /// Construct a bump allocator from existing memory without reinitializing.
    ///
    /// This allows the caller to (unsafely) fallback to manual borrow checking of the memory
    /// region between regions of allocator use.
    ///
    /// # Safety
    ///
    /// The memory must contain data that has been previously wrapped as a `BumpSlice`, exactly. The
    /// only endorsed sound form of obtaining such memory is [`BumpSlice::into_mem`].
    ///
    /// Warning: Any _use_ of the memory will have invalidated all pointers to allocated objects,
    /// more specifically the provenance of these pointers is no longer valid! You _must_ derive
    /// new pointers based on their offsets.
    pub unsafe fn from_mem_unchecked(mem: &mut [MaybeUninit<u8>]) -> LeakBox<'_, Self> {
        // Safety: memory already valid, according to the caller.
        let raw = unsafe { Self::reinterpret_aligned_mem(mem) };
        // Safety: we own this value in the sense that `Drop` is not called by the caller.
        unsafe { LeakBox::from_mut_unchecked(raw) }
    }

    /// Cast pre-initialized, aligned memory into a bump allocator.
    #[allow(unused_unsafe)]
    unsafe fn reinterpret_aligned_mem(mem: &mut [MaybeUninit<u8>]) -> &mut Self {
        // Safety: supposedly guaranteed by the caller.
        unsafe { core::hint::assert_unchecked(mem.as_ptr().cast::<Header>().is_aligned()) };

        let header = Self::header_layout();
        // debug_assert!(mem.len() >= header.size());
        // debug_assert!(mem.as_ptr().align_offset(header.align()) == 0);

        let datasize = mem.len() - header.size();
        // Round down to the header alignment! The whole struct will occupy memory according to its
        // natural alignment. We must be prepared fro the `pad_to_align` so to speak.
        let datasize = datasize - datasize % header.align();
        debug_assert!(Self::layout_from_size(datasize).is_ok_and(|l| l.size() <= mem.len()));

        let raw = mem.as_mut_ptr() as *mut u8;
        // Turn it into a fat pointer with correct metadata for a `BumpSlice`.
        // Safety:
        // - The data is writable as we owned
        unsafe { &mut *(ptr::slice_from_raw_parts_mut(raw, datasize) as *mut BumpSlice) }
    }

    /// Unwrap the memory owned by an unsized bump allocator.
    ///
    /// This releases the memory used by the allocator, similar to `Box::leak`, with the difference
    /// of operating on unique references instead. It is necessary to own the bump allocator due to
    /// internal state contained within the memory region that the caller can subsequently
    /// invalidate.
    ///
    /// # Example
    ///
    /// ```rust
    /// use core::mem::MaybeUninit;
    /// use static_alloc::unsync::BumpSlice;
    ///
    /// # let mut backing = [MaybeUninit::new(0); 128];
    /// # let alloc = BumpSlice::from_mem(&mut backing)?;
    /// let memory: &mut [_] = BumpSlice::into_mem(alloc);
    /// assert!(memory.len() <= 128, "Not guaranteed to use all memory");
    ///
    /// // Safety: We have not touched the memory itself.
    /// unsafe { BumpSlice::from_mem_unchecked(memory) };
    /// # Ok::<(), static_alloc::unsync::FromMemError>(())
    /// ```
    pub fn into_mem<'lt>(this: LeakBox<'lt, Self>) -> &'lt mut [MaybeUninit<u8>] {
        let layout = Layout::for_value(&*this);
        let mem_pointer = LeakBox::into_raw(this) as *mut MaybeUninit<u8>;
        unsafe { &mut *ptr::slice_from_raw_parts_mut(mem_pointer, layout.size()) }
    }

    /// Returns the layout for the `header` of a `BumpSlice`.
    /// The definition of `header` in this case is all the
    /// fields that come **before** the `data` field.
    /// If any of the fields of a BumpSlice are modified,
    /// this function likely has to be modified too.
    fn header_layout() -> Layout {
        Layout::new::<Cell<usize>>()
    }

    /// Returns the layout for an array with the size of `size`
    fn data_layout(size: usize) -> Result<Layout, LayoutError> {
        Layout::array::<UnsafeCell<MaybeUninit<u8>>>(size)
    }

    /// Returns a layout for a BumpSlice where the length of the data field is `size`.
    /// This relies on the two functions defined above.
    pub(crate) fn layout_from_size(size: usize) -> Result<Layout, LayoutError> {
        let data_tail = Self::data_layout(size)?;
        let (layout, _) = Self::header_layout().extend(data_tail)?;
        Ok(layout.pad_to_align())
    }

    /// Returns capacity of this `BumpSlice`.
    /// This is how many *bytes* can be allocated
    /// within this node.
    pub const fn capacity(&self) -> usize {
        self.data.get().len()
    }

    /// Get a raw pointer to the data.
    ///
    /// Note that *any* use of the pointer must be done with extreme care as it may invalidate
    /// existing references into the allocated region. Furthermore, bytes may not be initialized.
    /// The length of the valid region is [`BumpSlice::capacity`].
    ///
    /// Prefer [`BumpSlice::get_unchecked`] for reconstructing a prior allocation.
    pub fn data_ptr(&self) -> NonNull<u8> {
        NonNull::new(self.data.get() as *mut u8).expect("from a reference")
    }

    /// Allocate a region of memory.
    ///
    /// This is a safe alternative to [GlobalAlloc::alloc](#impl-GlobalAlloc).
    ///
    /// # Panics
    /// This function will panic if the requested layout has a size of `0`. For the use in a
    /// `GlobalAlloc` this is explicitely forbidden to request and would allow any behaviour but we
    /// instead strictly check it.
    ///
    /// FIXME(breaking): this could well be a `Result<_, Failure>`.
    pub fn alloc(&self, layout: Layout) -> Option<NonNull<u8>> {
        Some(self.try_alloc(layout)?.ptr)
    }

    /// Try to allocate some layout with a precise base location.
    ///
    /// The base location is the currently consumed byte count, without correction for the
    /// alignment of the allocation. This will succeed if it can be allocate exactly at the
    /// expected location.
    ///
    /// # Panics
    /// This function may panic if the provided `level` is from a different slab.
    pub fn alloc_at(&self, layout: Layout, level: Level) -> Result<NonNull<u8>, Failure> {
        let Allocation { ptr, .. } = self.try_alloc_at(layout, level.0)?;
        Ok(ptr)
    }

    /// Get an allocation for a specific type.
    ///
    /// It is not yet initialized but provides an interface for that initialization.
    ///
    /// ## Usage
    ///
    /// ```
    /// # use static_alloc::unsync::Bump;
    /// use core::cell::{Ref, RefCell};
    ///
    /// let slab: Bump<[Ref<'static, usize>; 1]> = Bump::uninit();
    /// let data = RefCell::new(0xff);
    ///
    /// // We can place a `Ref` here but we did not yet.
    /// let alloc = slab.get::<Ref<usize>>().unwrap();
    /// let cell_ref = unsafe {
    ///     alloc.leak(data.borrow())
    /// };
    ///
    /// assert_eq!(**cell_ref, 0xff);
    /// ```
    ///
    /// FIXME(breaking): this could well be a `Result<_, Failure>`.
    pub fn get<V>(&self) -> Option<Allocation<'_, V>> {
        let alloc = self.try_alloc(Layout::new::<V>())?;
        Some(Allocation {
            lifetime: alloc.lifetime,
            level: alloc.level,
            ptr: alloc.ptr.cast(),
        })
    }

    /// Get an allocation for a specific type at a specific level.
    ///
    /// See [`get`] for usage. This can be used to ensure that data is contiguous in concurrent
    /// access to the allocator.
    ///
    /// [`get`]: #method.get
    pub fn get_at<V>(&self, level: Level) -> Result<Allocation<'_, V>, Failure> {
        let alloc = self.try_alloc_at(Layout::new::<V>(), level.0)?;
        Ok(Allocation {
            lifetime: alloc.lifetime,
            level: alloc.level,
            ptr: alloc.ptr.cast(),
        })
    }

    /// Reacquire an allocation that has been performed previously.
    ///
    /// This call won't invalidate any other allocations.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that no other pointers to this prior allocation are alive, or can
    /// be created. This is guaranteed if the allocation was performed previously, has since been
    /// discarded, and `reset` can not be called (for example, the caller holds a shared
    /// reference).
    ///
    /// # Usage
    ///
    /// ```
    /// # use core::mem::MaybeUninit;
    /// # use static_alloc::unsync::BumpSlice;
    /// # let mut backing = [MaybeUninit::new(0); 128];
    /// # let alloc = BumpSlice::from_mem(&mut backing).unwrap();
    /// // Create an initial allocation.
    /// let level = alloc.level();
    /// let allocation = alloc.get_at::<usize>(level)?;
    /// let address = allocation.ptr.as_ptr() as usize;
    /// // pretend to lose the owning pointer of the allocation.
    /// let _ = { allocation };
    ///
    /// // Restore our access.
    /// let renewed = unsafe { alloc.get_unchecked::<usize>(level) };
    /// assert_eq!(address, renewed.ptr.as_ptr() as usize);
    /// # Ok::<_, static_alloc::bump::Failure>(())
    /// ```
    ///
    /// Crucially, you can rely on *other* allocations to stay valid. The caller is responsible of
    /// using the returning pointer to only refer to allocations that are not referenced through
    /// any other way.
    ///
    /// ```
    /// # use core::mem::MaybeUninit;
    /// # use static_alloc::{leaked::LeakBox, unsync::BumpSlice};
    /// # let mut backing = [MaybeUninit::new(0); 128];
    /// # let alloc = BumpSlice::from_mem(&mut backing).unwrap();
    /// let level = alloc.level();
    /// alloc.get_at::<usize>(level)?;
    ///
    /// let other_val = alloc.bump_box()?;
    /// let other_val = LeakBox::write(other_val, 0usize);
    ///
    /// let renew = unsafe { alloc.get_unchecked::<usize>(level) };
    /// assert_eq!(*other_val, 0); // Not UB!
    /// # Ok::<_, static_alloc::bump::Failure>(())
    /// ```
    pub unsafe fn get_unchecked<V>(&self, level: Level) -> Allocation<'_, V> {
        debug_assert!(level.0 < self.capacity());

        debug_assert!(
            level <= self.level(),
            "Tried to access an allocation that does not yet exist"
        );

        let base_ptr = self.data_ptr().as_ptr();
        // SAFETY: `level.0` is in bounds as assert above, or by the caller by having provided an
        // existing allocation—all allocations we hand out are in bounds.
        let alloc = unsafe { base_ptr.add(level.0) };
        let ptr = NonNull::new(alloc).unwrap().cast::<V>();

        debug_assert!(
            ptr.as_ptr().is_aligned(),
            "Tried to access an allocation with improper type"
        );

        Allocation {
            level,
            lifetime: AllocTime::default(),
            ptr,
        }
    }

    /// Allocate space for one `T` without initializing it.
    ///
    /// Note that the returned `MaybeUninit` can be unwrapped from `LeakBox`. Or you can store an
    /// arbitrary value and ensure it is safely dropped before the borrow ends.
    ///
    /// ## Usage
    ///
    /// ```
    /// # use static_alloc::unsync::Bump;
    /// use core::cell::RefCell;
    /// use static_alloc::leaked::LeakBox;
    ///
    /// let slab: Bump<[usize; 4]> = Bump::uninit();
    /// let data = RefCell::new(0xff);
    ///
    /// let slot = slab.bump_box().unwrap();
    /// let cell_box = LeakBox::write(slot, data.borrow());
    ///
    /// assert_eq!(**cell_box, 0xff);
    /// drop(cell_box);
    ///
    /// assert!(data.try_borrow_mut().is_ok());
    /// ```
    ///
    /// FIXME(breaking): should return evidence of the level (observed, and post). Something
    /// similar to `Allocation` but containing a `LeakBox<T>` instead? Introduce that to the sync
    /// `Bump` allocator as well.
    ///
    /// FIXME(breaking): align with sync `Bump::get` (probably rename get to bump_box).
    pub fn bump_box<'bump, T: 'bump>(
        &'bump self,
    ) -> Result<LeakBox<'bump, MaybeUninit<T>>, Failure> {
        let allocation = self.get_at(self.level())?;
        Ok(unsafe { allocation.uninit() }.into())
    }

    /// Allocate space for a slice of `T`s without initializing any.
    ///
    /// Retrieve individual `MaybeUninit` elements and wrap them as a `LeakBox` to store values. Or
    /// use the slice as backing memory for one of the containers from `without-alloc`. Or manually
    /// initialize them.
    ///
    /// ## Usage
    ///
    /// Quicksort, implemented recursively, requires a maximum of `log n` stack frames in the worst
    /// case when implemented optimally. Since each frame is quite large this is wasteful. We can
    /// use a properly sized buffer instead and implement an iterative solution. (Left as an
    /// exercise to the reader, or see the examples for `without-alloc` where we use such a dynamic
    /// allocation with an inline vector as our stack).
    pub fn bump_array<'bump, T: 'bump>(
        &'bump self,
        n: usize,
    ) -> Result<LeakBox<'bump, [MaybeUninit<T>]>, Failure> {
        let layout = Layout::array::<T>(n).map_err(|_| Failure::Exhausted)?;
        let raw = self.alloc(layout).ok_or(Failure::Exhausted)?;
        let slice = ptr::slice_from_raw_parts_mut(raw.cast().as_ptr(), n);
        let uninit = unsafe { &mut *slice };
        Ok(uninit.into())
    }

    /// Get the number of already allocated bytes.
    pub fn level(&self) -> Level {
        Level(self.header.index.get())
    }

    /// Reset the bump allocator.
    ///
    /// This requires a unique reference to the allocator hence no allocation can be alive at this
    /// point. It will reset the internal count of used bytes to zero.
    pub fn reset(&mut self) {
        self.header.index.set(0)
    }

    fn try_alloc(&self, layout: Layout) -> Option<Allocation<'_>> {
        let consumed = self.header.index.get();
        match self.try_alloc_at(layout, consumed) {
            Ok(alloc) => Some(alloc),
            Err(Failure::Exhausted) => None,
            Err(Failure::Mismatch { observed: _ }) => {
                unreachable!("Count in Cell concurrently modified, this UB")
            }
        }
    }

    fn try_alloc_at(
        &self,
        layout: Layout,
        expect_consumed: usize,
    ) -> Result<Allocation<'_>, Failure> {
        assert!(layout.size() > 0);
        let length = mem::size_of_val(&self.data);
        // We want to access contiguous slice, so cast to a single cell.
        let base_ptr = self.data.get().cast::<u8>();

        let alignment = layout.align();
        let requested = layout.size();

        // Ensure no overflows when calculating offets within.
        assert!(expect_consumed <= length, "{}/{}", expect_consumed, length);

        let available = length.checked_sub(expect_consumed).unwrap();
        let ptr_to = base_ptr.wrapping_add(expect_consumed);
        let offset = ptr_to.align_offset(alignment);

        if Some(requested) > available.checked_sub(offset) {
            return Err(Failure::Exhausted); // exhausted
        }

        // `size` can not be zero, saturation will thus always make this true.
        assert!(offset < available);
        let at_aligned = expect_consumed.checked_add(offset).unwrap();
        let new_consumed = at_aligned.checked_add(requested).unwrap();
        // new_consumed
        //    = consumed + offset + requested  [lines above]
        //   <= consumed + available  [bail out: exhausted]
        //   <= length  [first line of loop]
        // So it's ok to store `allocated` into `consumed`.
        assert!(new_consumed <= length);
        assert!(at_aligned < length);

        // Try to actually allocate.
        match self.bump(expect_consumed, new_consumed) {
            Ok(()) => (),
            Err(observed) => {
                // Someone else was faster, if you want it then recalculate again.
                return Err(Failure::Mismatch {
                    observed: Level(observed),
                });
            }
        }

        let aligned = unsafe {
            // SAFETY:
            // * `0 <= at_aligned < length` in bounds as checked above.
            base_ptr.byte_add(at_aligned)
        };

        Ok(Allocation {
            ptr: NonNull::new(aligned).unwrap(),
            lifetime: AllocTime::default(),
            level: Level(new_consumed),
        })
    }

    fn bump(&self, expect: usize, consume: usize) -> Result<(), usize> {
        debug_assert!(consume <= self.capacity());
        debug_assert!(expect <= consume);

        let prev = self.header.index.get();
        if prev != expect {
            Err(prev)
        } else {
            self.header.index.set(consume);
            Ok(())
        }
    }
}

struct EnsureDerefIsApplicable<T>(core::marker::PhantomData<T>);

impl<T> EnsureDerefIsApplicable<T> {
    pub const ASSERT: () = {
        if mem::offset_of!(Bump<T>, _data) != mem::size_of::<Header>() {
            panic!(
                // `data` follows header directly, using the macro requires a value for unsized types.
                "This `unsync::Bump` can not be used as a `BumpSlice` since the reinterpretation changes the data layout. (Hint: its alignment must be at most `usize`).",
            );
        }
    };
}

impl<T> ops::Deref for Bump<T> {
    type Target = BumpSlice;
    fn deref(&self) -> &BumpSlice {
        // This provokes post-mono error!
        let _: () = EnsureDerefIsApplicable::<T>::ASSERT;

        let from_layout = Layout::for_value(self);
        let data_layout = Layout::new::<MaybeUninit<T>>();
        // Construct a point with the meta data of a slice to `data`, but pointing to the whole
        // struct instead. This meta data is later copied to the meta data of `bump` when cast.
        let ptr = (self as *const Self).cast::<MaybeUninit<u8>>();
        let mem: *const [MaybeUninit<u8>] = ptr::slice_from_raw_parts(ptr, data_layout.size());
        // Now we have a pointer to BumpSlice with length meta data of the data slice.
        let bump = unsafe { &*(mem as *const BumpSlice) };
        debug_assert_eq!(from_layout, Layout::for_value(bump));
        bump
    }
}

impl<T> ops::DerefMut for Bump<T> {
    fn deref_mut(&mut self) -> &mut BumpSlice {
        // This provokes post-mono error!
        let _: () = EnsureDerefIsApplicable::<T>::ASSERT;

        let from_layout = Layout::for_value(self);
        let data_layout = Layout::new::<MaybeUninit<T>>();
        // Construct a point with the meta data of a slice to `data`, but pointing to the whole
        // struct instead. This meta data is later copied to the meta data of `bump` when cast.
        let ptr = (self as *mut Self).cast::<MaybeUninit<u8>>();
        let mem: *mut [MaybeUninit<u8>] = ptr::slice_from_raw_parts_mut(ptr, data_layout.size());
        // Now we have a pointer to BumpSlice with length meta data of the data slice.
        let bump = unsafe { &mut *(mem as *mut BumpSlice) };
        debug_assert_eq!(from_layout, Layout::for_value(bump));
        bump
    }
}

struct Header {
    /// An index into the data field. This index
    /// will always be an index to an element
    /// that has not been allocated into.
    /// Again this is wrapped in a Cell,
    /// to allow modification with just a
    /// &self reference.
    index: Cell<usize>,
}

impl Header {
    const fn empty() -> Self {
        Header {
            index: Cell::new(0),
        }
    }
}

#[test]
fn mem_bump_derefs_correctly() {
    let bump = Bump::<usize>::zeroed();
    let mem: &BumpSlice = &bump;
    assert_eq!(mem::size_of_val(&bump), mem::size_of_val(mem));
}
