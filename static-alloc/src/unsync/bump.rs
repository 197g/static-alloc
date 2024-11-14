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
/// This type dereferences to the generic `MemBump` that implements the allocation behavior. Note
/// that `MemBump` is an unsized type. In contrast this type is sized so it is possible to
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
/// # use static_alloc::unsync::MemBump;
/// # fn subroutine_one(_: &MemBump) {}
/// # fn subroutine_two(_: &MemBump) {}
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
/// use static_alloc::unsync::{Bump, MemBump};
/// # struct Request;
/// # fn handle_request(_: &MemBump, _: Request) {}
/// # fn iterate_recv() -> Option<Request> { None }
/// let mut local_page: Box<Bump<[u64; 64]>> = Box::new(Bump::uninit());
///
/// for request in iterate_recv() {
///     local_page.reset();
///     handle_request(&local_page, request);
/// }
/// ```
#[repr(C)]
pub struct Bump<T> {
    /// The index used in allocation.
    _index: Cell<usize>,
    /// The backing storage for raw allocated data.
    _data: UnsafeCell<MaybeUninit<T>>,
    // Warning: when changing the data layout, you must change `MemBump` as well.
}

/// An error used when one could not re-use raw memory for a bump allocator.
#[derive(Debug)]
pub struct FromMemError {
    _inner: (),
}

/// A dynamically sized allocation block in which any type can be allocated.
#[repr(C)]
pub struct MemBump {
    /// An index into the data field. This index
    /// will always be an index to an element
    /// that has not been allocated into.
    /// Again this is wrapped in a Cell,
    /// to allow modification with just a
    /// &self reference.
    index: Cell<usize>,

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
            _index: Cell::new(0),
            _data: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Create an allocator with zeroed memory.
    ///
    /// The caller can rely on all allocations to be zeroed.
    pub fn zeroed() -> Self {
        Bump {
            _index: Cell::new(0),
            _data: UnsafeCell::new(MaybeUninit::zeroed()),
        }
    }
}

#[cfg(feature = "alloc")]
impl MemBump {
    /// Allocate some space to use for a bump allocator.
    pub fn new(capacity: usize) -> alloc::boxed::Box<Self> {
        let layout = Self::layout_from_size(capacity).expect("Bad layout");
        let ptr = NonNull::new(unsafe { alloc::alloc::alloc(layout) })
            .unwrap_or_else(|| alloc::alloc::handle_alloc_error(layout));
        let ptr = ptr::slice_from_raw_parts_mut(ptr.as_ptr(), capacity);
        unsafe { alloc::boxed::Box::from_raw(ptr as *mut MemBump) }
    }
}

impl MemBump {
    /// Initialize a bump allocator from existing memory.
    ///
    /// # Usage
    ///
    /// ```
    /// use core::mem::MaybeUninit;
    /// use static_alloc::unsync::MemBump;
    ///
    /// let mut backing = [MaybeUninit::new(0); 128];
    /// let alloc = MemBump::from_mem(&mut backing)?;
    ///
    /// # Ok::<(), static_alloc::unsync::FromMemError>(())
    /// ```
    pub fn from_mem(mem: &mut [MaybeUninit<u8>]) -> Result<LeakBox<'_, Self>, FromMemError> {
        let header = Self::header_layout();
        let offset = mem.as_ptr().align_offset(header.align());
        // Align the memory for the header.
        let mem = mem.get_mut(offset..).ok_or(FromMemError { _inner: () })?;
        mem.get_mut(..header.size())
            .ok_or(FromMemError { _inner: () })?
            .fill(MaybeUninit::new(0));
        Ok(unsafe { Self::from_mem_unchecked(mem) })
    }

    /// Construct a bump allocator from existing memory without reinitializing.
    ///
    /// This allows the caller to (unsafely) fallback to manual borrow checking of the memory
    /// region between regions of allocator use.
    ///
    /// # Safety
    ///
    /// The memory must contain data that has been previously wrapped as a `MemBump`, exactly. The
    /// only endorsed sound form of obtaining such memory is [`MemBump::into_mem`].
    ///
    /// Warning: Any _use_ of the memory will have invalidated all pointers to allocated objects,
    /// more specifically the provenance of these pointers is no longer valid! You _must_ derive
    /// new pointers based on their offsets.
    pub unsafe fn from_mem_unchecked(mem: &mut [MaybeUninit<u8>]) -> LeakBox<'_, Self> {
        let raw = Self::from_aligned_mem(mem);
        LeakBox::from_mut_unchecked(raw)
    }

    /// Cast pre-initialized, aligned memory into a bump allocator.
    #[allow(unused_unsafe)]
    unsafe fn from_aligned_mem(mem: &mut [MaybeUninit<u8>]) -> &mut Self {
        let header = Self::header_layout();
        // debug_assert!(mem.len() >= header.size());
        // debug_assert!(mem.as_ptr().align_offset(header.align()) == 0);

        let datasize = mem.len() - header.size();
        // Round down to the header alignment! The whole struct will occupy memory according to its
        // natural alignment. We must be prepared fro the `pad_to_align` so to speak.
        let datasize = datasize - datasize % header.align();
        debug_assert!(Self::layout_from_size(datasize).map_or(false, |l| l.size() <= mem.len()));

        let raw = mem.as_mut_ptr() as *mut u8;
        // Turn it into a fat pointer with correct metadata for a `MemBump`.
        // Safety:
        // - The data is writable as we owned
        unsafe { &mut *(ptr::slice_from_raw_parts_mut(raw, datasize) as *mut MemBump) }
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
    /// use static_alloc::unsync::MemBump;
    ///
    /// # let mut backing = [MaybeUninit::new(0); 128];
    /// # let alloc = MemBump::from_mem(&mut backing)?;
    /// let memory: &mut [_] = MemBump::into_mem(alloc);
    /// assert!(memory.len() <= 128, "Not guaranteed to use all memory");
    ///
    /// // Safety: We have not touched the memory itself.
    /// unsafe { MemBump::from_mem_unchecked(memory) };
    /// # Ok::<(), static_alloc::unsync::FromMemError>(())
    /// ```
    pub fn into_mem<'lt>(this: LeakBox<'lt, Self>) -> &'lt mut [MaybeUninit<u8>] {
        let layout = Layout::for_value(&*this);
        let mem_pointer = LeakBox::into_raw(this) as *mut MaybeUninit<u8>;
        unsafe { &mut *ptr::slice_from_raw_parts_mut(mem_pointer, layout.size()) }
    }

    /// Returns the layout for the `header` of a `MemBump`.
    /// The definition of `header` in this case is all the
    /// fields that come **before** the `data` field.
    /// If any of the fields of a MemBump are modified,
    /// this function likely has to be modified too.
    fn header_layout() -> Layout {
        Layout::new::<Cell<usize>>()
    }

    /// Returns the layout for an array with the size of `size`
    fn data_layout(size: usize) -> Result<Layout, LayoutError> {
        Layout::array::<UnsafeCell<MaybeUninit<u8>>>(size)
    }

    /// Returns a layout for a MemBump where the length of the data field is `size`.
    /// This relies on the two functions defined above.
    pub(crate) fn layout_from_size(size: usize) -> Result<Layout, LayoutError> {
        let data_tail = Self::data_layout(size)?;
        let (layout, _) = Self::header_layout().extend(data_tail)?;
        Ok(layout.pad_to_align())
    }

    /// Returns capacity of this `MemBump`.
    /// This is how many *bytes* can be allocated
    /// within this node.
    pub const fn capacity(&self) -> usize {
        // Safety: just gets the pointer metadata `len` without invalidating any provenance,
        // accepting the pointer use itself. This may be replaced by a safe `pointer::len` as soon
        // as stable (#71146) and const which would avoid any pointer use.
        unsafe { (*(self.data.get() as *const [UnsafeCell<u8>])).len() }
    }

    /// Get a raw pointer to the data.
    ///
    /// Note that *any* use of the pointer must be done with extreme care as it may invalidate
    /// existing references into the allocated region. Furthermore, bytes may not be initialized.
    /// The length of the valid region is [`MemBump::capacity`].
    ///
    /// Prefer [`MemBump::get_unchecked`] for reconstructing a prior allocation.
    pub fn data_ptr(&self) -> NonNull<u8> {
        NonNull::new(self.data.get() as *mut u8).expect("from a reference")
    }

impl MemBump {
    /// Wrap a raw slice of memory.
    ///
    /// This allows bump allocating from a memory resource that has been acquired through other
    /// means, such as but not limited to, from a chunk of RAM passed by a boot service, some slice
    /// allocated via `alloca`, a local arena for storing related values, etc.
    pub fn with_memory(memory: &mut [MaybeUninit<u8>]) -> Option<&mut Self> {
        let head_layout = Layout::new::<Cell<usize>>();
        let wasted_head = memory.as_ptr().align_offset(head_layout.align());
        let aligned = memory.get_mut(wasted_head..)?;

        let data_len = aligned.len().checked_sub(head_layout.size())?;
        let head = aligned.as_mut_ptr() as *mut Cell<usize>;
        // SAFETY: has room for at least `Cell<usize>` and is aligned to it.
        // * asserted by subtracting the size from total length
        // * and by manually aligning it according to offset.
        unsafe { head.write(Cell::new(0)) };

        let slice = ptr::slice_from_raw_parts_mut(aligned.as_mut_ptr(), data_len);
        // SAFETY: has the declared size, and is initialized. The data tail does not need to be
        // initialized, it only contains `MaybeUninit` data.
        let bump = unsafe { &mut *(slice as *mut MemBump) };
        debug_assert_eq!(bump.data.len(), data_len);
        Some(bump)
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
    pub fn get<V>(&self) -> Option<Allocation<V>> {
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
    pub fn get_at<V>(&self, level: Level) -> Result<Allocation<V>, Failure> {
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
    /// # use static_alloc::unsync::MemBump;
    /// # let mut backing = [MaybeUninit::new(0); 128];
    /// # let alloc = MemBump::from_mem(&mut backing).unwrap();
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
    /// Critically, you can rely on *other* allocations to stay valid.
    ///
    /// ```
    /// # use core::mem::MaybeUninit;
    /// # use static_alloc::{leaked::LeakBox, unsync::MemBump};
    /// # let mut backing = [MaybeUninit::new(0); 128];
    /// # let alloc = MemBump::from_mem(&mut backing).unwrap();
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
    pub unsafe fn get_unchecked<V>(&self, level: Level) -> Allocation<V> {
        debug_assert!(level.0 < self.capacity());
        let ptr = self.data_ptr().as_ptr();
        // Safety: guaranteed by the caller.
        let alloc = ptr.offset(level.0 as isize) as *mut V;

        Allocation {
            level,
            lifetime: AllocTime::default(),
            ptr: NonNull::new_unchecked(alloc),
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
        Level(self.index.get())
    }

    /// Reset the bump allocator.
    ///
    /// This requires a unique reference to the allocator hence no allocation can be alive at this
    /// point. It will reset the internal count of used bytes to zero.
    pub fn reset(&mut self) {
        self.index.set(0)
    }

    fn try_alloc(&self, layout: Layout) -> Option<Allocation<'_>> {
        let consumed = self.index.get();
        match self.try_alloc_at(layout, consumed) {
            Ok(alloc) => return Some(alloc),
            Err(Failure::Exhausted) => return None,
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
        let data: &UnsafeCell<[MaybeUninit<u8>]> =
            unsafe { &*(&self.data as *const _ as *const UnsafeCell<_>) };
        let base_ptr = data.get() as *mut u8;

        let alignment = layout.align();
        let requested = layout.size();

        // Ensure no overflows when calculating offets within.
        assert!(expect_consumed <= length);

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
            (base_ptr as *mut u8).add(at_aligned)
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
        let prev = self.index.get();
        if prev != expect {
            Err(prev)
        } else {
            self.index.set(consume);
            Ok(())
        }
    }
}

impl<T> ops::Deref for Bump<T> {
    type Target = MemBump;
    fn deref(&self) -> &MemBump {
        let from_layout = Layout::for_value(self);
        let data_layout = Layout::new::<MaybeUninit<T>>();
        // Construct a point with the meta data of a slice to `data`, but pointing to the whole
        // struct instead. This meta data is later copied to the meta data of `bump` when cast.
        let ptr = self as *const Self as *const MaybeUninit<u8>;
        let mem: *const [MaybeUninit<u8>] = ptr::slice_from_raw_parts(ptr, data_layout.size());
        // Now we have a pointer to MemBump with length meta data of the data slice.
        let bump = unsafe { &*(mem as *const MemBump) };
        debug_assert_eq!(from_layout, Layout::for_value(bump));
        bump
    }
}

impl<T> ops::DerefMut for Bump<T> {
    fn deref_mut(&mut self) -> &mut MemBump {
        let from_layout = Layout::for_value(self);
        let data_layout = Layout::new::<MaybeUninit<T>>();
        // Construct a point with the meta data of a slice to `data`, but pointing to the whole
        // struct instead. This meta data is later copied to the meta data of `bump` when cast.
        let ptr = self as *mut Self as *mut MaybeUninit<u8>;
        let mem: *mut [MaybeUninit<u8>] = ptr::slice_from_raw_parts_mut(ptr, data_layout.size());
        // Now we have a pointer to MemBump with length meta data of the data slice.
        let bump = unsafe { &mut *(mem as *mut MemBump) };
        debug_assert_eq!(from_layout, Layout::for_value(bump));
        bump
    }
}

#[test]
fn mem_bump_derefs_correctly() {
    let bump = Bump::<usize>::zeroed();
    let mem: &MemBump = &bump;
    assert_eq!(mem::size_of_val(&bump), mem::size_of_val(mem));
}
