//! The bump allocator.
//!
//! Basics of usage and the connection between the structs is discussed in the documentation of the
//! [`Bump`] itself.
//!
//! [`Bump`]: struct.Bump.html
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::mem::{self, MaybeUninit};
use core::ptr::{null_mut, NonNull};

#[cfg(not(feature = "polyfill"))]
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "polyfill")]
use atomic_polyfill::{AtomicUsize, Ordering};

use crate::leaked::LeakBox;
use alloc_traits::{AllocTime, LocalAlloc, NonZeroLayout};

/// Allocator drawing from an inner, statically sized memory resource.
///
/// The type parameter `T` is used only to annotate the required size and alignment of the region
/// and has no futher use. Note that in particular there is no safe way to retrieve or unwrap an
/// inner instance even if the `Bump` was not constructed as a shared global static. Nevertheless,
/// the choice of type makes it easier to reason about potentially required extra space due to
/// alignment padding.
///
/// This type is *always* `Sync` to allow creating `static` instances. This works only because
/// there is no actual instance of `T` contained inside.
///
/// ## Usage as global allocator
///
/// You can use the stable rust attribute to use an instance of this type as the global allocator.
///
/// ```rust,no_run
/// use static_alloc::Bump;
///
/// #[global_allocator]
/// static A: Bump<[u8; 1 << 16]> = Bump::uninit();
///
/// fn main() { }
/// ```
///
/// Take care, some runtime features of Rust will allocate some memory before or after your own
/// code. In particular, it was found to be be tricky to predict the usage of the builtin test
/// framework which seemingly allocates some structures per test.
///
/// ## Usage as a non-dropping local allocator
///
/// It is also possible to use a `Bump` as a stack local allocator or a specialized allocator. The
/// interface offers some utilities for allocating values from references to shared or unshared
/// instances directly. **Note**: this will never call the `Drop` implementation of the allocated
/// type. In particular, it would almost surely not be safe to `Pin` the values, except if there is
/// a guarantee for the `Bump` itself to not be deallocated either.
///
/// ```rust
/// use static_alloc::Bump;
///
/// let local: Bump<[u64; 3]> = Bump::uninit();
///
/// let one = local.leak(0_u64).unwrap();
/// let two = local.leak(1_u64).unwrap();
/// let three = local.leak(2_u64).unwrap();
///
/// // Exhausted the space.
/// assert!(local.leak(3_u64).is_err());
/// ```
///
/// Mind that the supplied type parameter influenced *both* size and alignment and a `[u8; 24]`
/// does not guarantee being able to allocation three `u64` even though most targets have a minimum
/// alignment requirement of 16 and it works fine on those.
///
/// ```rust
/// # use static_alloc::Bump;
/// // Just enough space for `u128` but no alignment requirement.
/// let local: Bump<[u8; 16]> = Bump::uninit();
///
/// // May or may not return an err.
/// let _ = local.leak(0_u128);
/// ```
///
/// Instead use the type parameter to `Bump` as a hint for the best alignment.
///
/// ```rust
/// # use static_alloc::Bump;
/// // Enough space and align for `u128`.
/// let local: Bump<[u128; 1]> = Bump::uninit();
///
/// assert!(local.leak(0_u128).is_ok());
/// ```
///
/// ## Usage as a (local) bag of bits
///
/// It is of course entirely possible to use a local instance instead of a single global allocator.
/// For example you could utilize the pointer interface directly to build a `#[no_std]` dynamic
/// data structure in an environment without `extern lib alloc`. This feature was the original
/// motivation behind the crate but no such data structures are provided here so a quick sketch of
/// the idea must do:
///
/// ```
/// use core::alloc;
/// use static_alloc::Bump;
///
/// #[repr(align(4096))]
/// struct PageTable {
///     // some non-trivial type.
/// #   _private: [u8; 4096],
/// }
///
/// impl PageTable {
///     /// Avoid stack allocation of the full struct.
///     pub unsafe fn new(into: *mut u8) -> &'static mut Self {
///         // ...
/// #       &mut *(into as *mut Self)
///     }
/// }
///
/// // Allocator for pages for page tables. Provides 64 pages. When the
/// // program/kernel is provided as an ELF the bootloader reserves
/// // memory for us as part of the loading process that we can use
/// // purely for page tables. Replaces asm `paging: .BYTE <size>;`
/// static Paging: Bump<[u8; 1 << 18]> = Bump::uninit();
///
/// fn main() {
///     let layout = alloc::Layout::new::<PageTable>();
///     let memory = Paging.alloc(layout).unwrap();
///     let table = unsafe {
///         PageTable::new(memory.as_ptr())
///     };
/// }
/// ```
///
/// A similar structure would of course work to allocate some non-`'static' objects from a
/// temporary `Bump`.
///
/// ## More insights
///
/// The ordering used is currently `SeqCst`. This enforces a single global sequence of observed
/// effects on the slab level. The author is fully aware that this is not strictly necessary. In
/// fact, even `AcqRel` may not be required as the monotonic bump allocator does not synchronize
/// other memory itself. If you bring forward a PR with a formalized reasoning for relaxing the
/// requirements to `Relaxed` (llvm `Monotonic`) it will be greatly appreciated (even more if you
/// demonstrate performance gains).
///
/// WIP: slices.
pub struct Bump<T> {
    /// While in shared state, an monotonic atomic counter of consumed bytes.
    ///
    /// While shared it is only mutated in `bump` which guarantees its invariants. In the mutable
    /// reference state it is modified arbitrarily.
    consumed: AtomicUsize,

    /// Outer unsafe cell due to thread safety.
    /// Inner MaybeUninit because we padding may destroy initialization invariant
    /// on the bytes themselves, and hence drop etc must not assumed inited.
    storage: UnsafeCell<MaybeUninit<T>>,
}

/// A value could not be moved into a slab allocation.
///
/// The error contains the value for which the allocation failed. Storing the value in the error
/// keeps it alive in all cases. This prevents the `Drop` implementation from running and preserves
/// resources which may otherwise not be trivial to restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LeakError<T> {
    val: T,
    failure: Failure,
}

/// Specifies an amount of consumed space of a slab.
///
/// Each allocation of the `Bump` increases the current level as they must not be empty. By
/// ensuring that an allocation is performed at a specific level it is thus possible to check that
/// multiple allocations happened in succession without other intermediate allocations. This
/// ability in turns makes it possible to group allocations together, for example to initialize a
/// `#[repr(C)]` struct member-by-member or to extend a slice.
///
/// ## Usage
///
/// The main use is successively allocating a slice without requiring all data to be present at
/// once. Other similar interface often require an internal locking mechanism but `Level` leaves
/// the choice to the user. This is not yet encapsulate in a safe API yet `Level` makes it easy to
/// reason about.
///
/// See [`MemBump::get_unchecked`][crate::unsync::MemBump] for redeeming a value.
///
/// ## Unsound usage.
///
/// FIXME: the below is UB because we don't gain provenance over the complete array, only each
/// individual element. Instead, we must derive a new pointer from the allocator!
///
/// ```
/// # use core::slice;
/// # use static_alloc::bump::{Level, Bump};
/// static BUMP: Bump<[u64; 4]> = Bump::uninit();
///
/// /// Gathers as much data as possible.
/// ///
/// /// An arbitrary amount of data, can't stack allocate!
/// fn gather_data(mut iter: impl Iterator<Item=u64>) -> &'static mut [u64] {
///     let first = match iter.next() {
///         Some(item) => item,
///         None => return &mut [],
///     };
///
///     let mut level: Level = BUMP.level();
///     let mut start: Level = BUMP.level();
///     let mut count;
///
///     match BUMP.leak_at(first, level) {
///         Ok((_first, first_level)) => {
///             // Note we must throw away the first pointer. Its provenance does not
///             // cover the other fields. Only its level can be used.
///             level = first_level;
///             count = 1;
///         },
///         _ => return &mut [],
///     }
///
///     let _ = iter.try_for_each(|value: u64| {
///         match BUMP.leak_at(value, level) {
///             Err(err) => return Err(err),
///             Ok((_, new_level)) => level = new_level,
///         };
///         count += 1;
///         Ok(())
///     });
///
///     unsafe {
///         // Safety: we have an allocation here
///         let begin = BUMP.get_unchecked(start);
///         // SAFETY: all `count` allocations are contiguous, begin is well aligned and no
///         // reference is currently pointing at any of the values. The lifetime is `'static` as
///         // the BUMP itself is static.
///         slice::from_raw_parts_mut(begin.ptr.as_ptr(), count)
///     }
/// }
///
/// fn main() {
///     // There is no other thread running, so this succeeds.
///     let slice = gather_data(0..=3);
///     assert_eq!(slice, [0, 1, 2, 3]);
/// }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Level(pub(crate) usize);

/// A successful allocation and current [`Level`].
///
/// [`Level`]: struct.Level.html
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Allocation<'a, T = u8> {
    /// Pointer to the uninitialized region with specified layout.
    pub ptr: NonNull<T>,

    /// The lifetime of the allocation.
    pub lifetime: AllocTime<'a>,

    /// The observed amount of consumed bytes after the allocation.
    pub level: Level,
}

/// Reason for a failed allocation at an exact [`Level`].
///
/// [`Level`]: struct.Level.html
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Failure {
    /// No space left for that allocation.
    Exhausted,

    /// The allocation would not have used the expected base location.
    ///
    /// Reports the location that was observed. When only levels from the same slab are used (which
    /// should normally be the case) then the observed level is monotonically increasing.
    Mismatch {
        /// The observed level that was different from the requested one.
        observed: Level,
    },
}

impl<T> Bump<T> {
    /// Make a new allocatable slab of certain byte size and alignment.
    ///
    /// The storage will contain uninitialized bytes.
    pub const fn uninit() -> Self {
        Bump {
            consumed: AtomicUsize::new(0),
            storage: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Make a new allocatable slab of certain byte size and alignment.
    ///
    /// The storage will contain zeroed bytes. This is not *yet* available
    /// as a `const fn` which currently limits its potential usefulness
    /// but there is no good reason not to provide it regardless.
    pub fn zeroed() -> Self {
        Bump {
            consumed: AtomicUsize::new(0),
            storage: UnsafeCell::new(MaybeUninit::zeroed()),
        }
    }

    /// Make a new allocatable slab provided with some bytes it can hand out.
    ///
    /// Note that `storage` will never be dropped and there is no way to get it back.
    pub const fn new(storage: T) -> Self {
        Bump {
            consumed: AtomicUsize::new(0),
            storage: UnsafeCell::new(MaybeUninit::new(storage)),
        }
    }

    /// Reset the bump allocator.
    ///
    /// Requires a mutable reference, as no allocations can be active when doing it. This behaves
    /// as if a fresh instance was assigned but it does not overwrite the bytes in the backing
    /// storage. (You can unsafely rely on this).
    ///
    /// ## Usage
    ///
    /// ```
    /// # use static_alloc::Bump;
    /// let mut stack_buf = Bump::<usize>::uninit();
    ///
    /// let bytes = stack_buf.leak(0usize.to_be_bytes()).unwrap();
    /// // Now the bump allocator is full.
    /// assert!(stack_buf.leak(0u8).is_err());
    ///
    /// // We can reuse if we are okay with forgetting the previous value.
    /// stack_buf.reset();
    /// let val = stack_buf.leak(0usize).unwrap();
    /// ```
    ///
    /// Trying to use the previous value does not work, as the stack is still borrowed. Note that
    /// any user unsafely tracking the lifetime must also ensure this through proper lifetimes that
    /// guarantee that borrows are alive for appropriate times.
    ///
    /// ```compile_fail
    /// // error[E0502]: cannot borrow `stack_buf` as mutable because it is also borrowed as immutable
    /// # use static_alloc::Bump;
    /// let mut stack_buf = Bump::<usize>::uninit();
    ///
    /// let bytes = stack_buf.leak(0usize).unwrap();
    /// //          --------- immutably borrow occurs here
    /// stack_buf.reset();
    /// // ^^^^^^^ mutable borrow occurs here.
    /// let other = stack_buf.leak(0usize).unwrap();
    ///
    /// *bytes += *other;
    /// // ------------- immutable borrow later used here
    /// ```
    pub fn reset(&mut self) {
        *self.consumed.get_mut() = 0;
    }

    /// Allocate a region of memory.
    ///
    /// This is a safe alternative to [GlobalAlloc::alloc](#impl-GlobalAlloc).
    ///
    /// # Panics
    /// This function will panic if the requested layout has a size of `0`. For the use in a
    /// `GlobalAlloc` this is explicitely forbidden to request and would allow any behaviour but we
    /// instead strictly check it.
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
    pub fn alloc_at(&self, layout: Layout, level: Level) -> Result<Allocation, Failure> {
        let Allocation {
            ptr,
            lifetime,
            level,
        } = self.try_alloc_at(layout, level.0)?;

        Ok(Allocation {
            ptr: ptr.cast(),
            lifetime,
            level,
        })
    }

    /// Get an allocation with detailed layout.
    ///
    /// Provides an [`Uninit`] wrapping several aspects of initialization in a safe interface,
    /// bound by the lifetime of the reference to the allocator.
    ///
    /// [`Uninit`]: ../uninit/struct.Uninit.html
    pub fn get_layout(&self, layout: Layout) -> Option<Allocation<'_>> {
        self.try_alloc(layout)
    }

    /// Get an allocation with detailed layout at a specific level.
    ///
    /// Provides an [`Uninit`] wrapping several aspects of initialization in a safe interface,
    /// bound by the lifetime of the reference to the allocator.
    ///
    /// Since the underlying allocation is the same, it would be `unsafe` but justified to fuse
    /// this allocation with the preceding or succeeding one.
    ///
    /// [`Uninit`]: ../uninit/struct.Uninit.html
    pub fn get_layout_at(&self, layout: Layout, at: Level) -> Result<Allocation<'_>, Failure> {
        self.try_alloc_at(layout, at.0)
    }

    /// Get an allocation for a specific type.
    ///
    /// It is not yet initialized but provides a safe interface for that initialization.
    ///
    /// ## Usage
    ///
    /// ```
    /// # use static_alloc::Bump;
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
    pub fn get<V>(&self) -> Option<Allocation<V>> {
        if mem::size_of::<V>() == 0 {
            return Some(self.zst_fake_alloc());
        }

        let layout = Layout::new::<V>();
        let Allocation {
            ptr,
            lifetime,
            level,
        } = self.try_alloc(layout)?;

        Some(Allocation {
            ptr: ptr.cast(),
            lifetime,
            level,
        })
    }

    /// Get an allocation for a specific type at a specific level.
    ///
    /// See [`get`] for usage.
    ///
    /// [`get`]: #method.get
    pub fn get_at<V>(&self, level: Level) -> Result<Allocation<V>, Failure> {
        if mem::size_of::<V>() == 0 {
            let fake = self.zst_fake_alloc();
            // Note: zst_fake_alloc is a noop on the level, we may as well check after.
            if fake.level != level {
                return Err(Failure::Mismatch {
                    observed: fake.level,
                });
            }
            return Ok(fake);
        }

        let layout = Layout::new::<V>();
        let Allocation {
            ptr,
            lifetime,
            level,
        } = self.try_alloc_at(layout, level.0)?;

        Ok(Allocation {
            // It has exactly size and alignment for `V` as requested.
            ptr: ptr.cast(),
            lifetime,
            level,
        })
    }

    /// Move a value into an owned allocation.
    ///
    /// For safely initializing a value _after_ a successful allocation, see [`LeakBox::write`].
    ///
    /// [`LeakBox::write`]: ../leaked/struct.LeakBox.html#method.write
    ///
    /// ## Usage
    ///
    /// This can be used to push the value into a caller provided stack buffer where it lives
    /// longer than the current stack frame. For example, you might create a linked list with a
    /// dynamic number of values living in the frame below while still being dropped properly. This
    /// is impossible to do with a return value.
    ///
    /// ```
    /// # use static_alloc::Bump;
    /// # use static_alloc::leaked::LeakBox;
    /// fn rand() -> usize { 4 }
    ///
    /// enum Chain<'buf, T> {
    ///    Tail,
    ///    Link(T, LeakBox<'buf, Self>),
    /// }
    ///
    /// fn make_chain<Buf, T>(buf: &Bump<Buf>, mut new_node: impl FnMut() -> T)
    ///     -> Option<Chain<'_, T>>
    /// {
    ///     let count = rand();
    ///     let mut chain = Chain::Tail;
    ///     for _ in 0..count {
    ///         let node = new_node();
    ///         chain = Chain::Link(node, buf.leak_box(chain)?);
    ///     }
    ///     Some(chain)
    /// }
    ///
    /// struct Node (usize);
    /// impl Drop for Node {
    ///     fn drop(&mut self) {
    ///         println!("Dropped {}", self.0);
    ///     }
    /// }
    /// let mut counter = 0..;
    /// let new_node = || Node(counter.next().unwrap());
    ///
    /// let buffer: Bump<[u8; 128]> = Bump::uninit();
    /// let head = make_chain(&buffer, new_node).unwrap();
    ///
    /// // Prints the message in reverse order.
    /// // Dropped 3
    /// // Dropped 2
    /// // Dropped 1
    /// // Dropped 0
    /// drop(head);
    /// ```
    pub fn leak_box<V>(&self, val: V) -> Option<LeakBox<'_, V>> {
        let Allocation { ptr, lifetime, .. } = self.get::<V>()?;
        Some(unsafe { LeakBox::new_from_raw_non_null(ptr, val, lifetime) })
    }

    /// Move a value into an owned allocation.
    ///
    /// See [`leak_box`] for usage.
    ///
    /// [`leak_box`]: #method.leak_box
    pub fn leak_box_at<V>(&self, val: V, level: Level) -> Result<LeakBox<'_, V>, Failure> {
        let Allocation { ptr, lifetime, .. } = self.get_at::<V>(level)?;
        Ok(unsafe { LeakBox::new_from_raw_non_null(ptr, val, lifetime) })
    }

    /// Observe the current level.
    ///
    /// Keep in mind that concurrent usage of the same slab may modify the level before you are
    /// able to use it in `alloc_at`. Calling this method provides also no other guarantees on
    /// synchronization of memory accesses, only that the values observed by the caller are a
    /// monotonically increasing seequence while a shared reference exists.
    pub fn level(&self) -> Level {
        Level(self.consumed.load(Ordering::SeqCst))
    }

    /// Get a pointer to an existing allocation at a specific level.
    ///
    /// The resulting pointer may be used to access an arbitrary allocation starting at the pointer
    /// (i.e. including additional allocations immediately afterwards) but the caller is
    /// responsible for ensuring that these accesses do not overlap other accesses. There must be
    /// no more life [`LeakBox`] to any allocation being accessed this way.
    ///
    /// # Safety
    ///
    /// - The level must refer to an existing allocation, i.e. it must previously have been
    ///   returned in [`Allocation::level`].
    /// - As a corollary, particular it must be in-bounds of the allocator's memory.
    /// - Another consequence, the result pointer must be aligned for the requested type.
    pub unsafe fn get_unchecked<V>(&self, level: Level) -> Allocation<'_, V> {
        debug_assert!(level.0 <= mem::size_of_val(&self.storage));

        debug_assert!(
            level <= self.level(),
            "Tried to access an allocation that does not yet exist"
        );

        let base_ptr = self.storage.get() as *mut T as *mut u8;
        let alloc = base_ptr.add(level.0);
        let ptr = NonNull::new_unchecked(alloc).cast::<V>();

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

    fn try_alloc(&self, layout: Layout) -> Option<Allocation<'_>> {
        // Guess zero, this will fail when we try to access it and it isn't.
        let mut consumed = 0;
        loop {
            match self.try_alloc_at(layout, consumed) {
                Ok(alloc) => return Some(alloc),
                Err(Failure::Exhausted) => return None,
                Err(Failure::Mismatch { observed }) => consumed = observed.0,
            }
        }
    }

    /// Try to allocate some layout with a precise base location.
    ///
    /// The base location is the currently consumed byte count, without correction for the
    /// alignment of the allocation. This will succeed if it can be allocate exactly at the
    /// expected location.
    ///
    /// # Panics
    /// This function panics if `expect_consumed` is larger than `length`.
    fn try_alloc_at(
        &self,
        layout: Layout,
        expect_consumed: usize,
    ) -> Result<Allocation<'_>, Failure> {
        assert!(layout.size() > 0);
        let length = mem::size_of::<T>();
        let base_ptr = self.storage.get() as *mut T as *mut u8;

        let alignment = layout.align();
        let requested = layout.size();

        // Ensure no overflows when calculating offets within.
        assert!(expect_consumed <= length);

        let available = length.checked_sub(expect_consumed).unwrap();
        let ptr_to = base_ptr.wrapping_add(expect_consumed);
        let offset = ptr_to.align_offset(alignment);

        if requested > available.saturating_sub(offset) {
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

    /// Allocate a value for the lifetime of the allocator.
    ///
    /// The value is leaked in the sense that
    ///
    /// 1. the drop implementation of the allocated value is never called;
    /// 2. reusing the memory for another allocation in the same `Bump` requires manual unsafe code
    ///    to handle dropping and reinitialization.
    ///
    /// However, it does not mean that the underlying memory used for the allocated value is never
    /// reclaimed. If the `Bump` itself is a stack value then it will get reclaimed together with
    /// it.
    ///
    /// ## Safety notice
    ///
    /// It is important to understand that it is undefined behaviour to reuse the allocation for
    /// the *whole lifetime* of the returned reference. That is, dropping the allocation in-place
    /// while the reference is still within its lifetime comes with the exact same unsafety caveats
    /// as [`ManuallyDrop::drop`].
    ///
    /// ```
    /// # use static_alloc::Bump;
    /// #[derive(Debug, Default)]
    /// struct FooBar {
    ///     // ...
    /// # _private: [u8; 1],
    /// }
    ///
    /// let local: Bump<[FooBar; 3]> = Bump::uninit();
    /// let one = local.leak(FooBar::default()).unwrap();
    ///
    /// // Dangerous but justifiable.
    /// let one = unsafe {
    ///     // Ensures there is no current mutable borrow.
    ///     core::ptr::drop_in_place(&mut *one);
    /// };
    /// ```
    ///
    /// ## Usage
    ///
    /// ```
    /// use static_alloc::Bump;
    ///
    /// let local: Bump<[u64; 3]> = Bump::uninit();
    ///
    /// let one = local.leak(0_u64).unwrap();
    /// assert_eq!(*one, 0);
    /// *one = 42;
    /// ```
    ///
    /// ## Limitations
    ///
    /// Only sized values can be allocated in this manner for now, unsized values are blocked on
    /// stabilization of [`ptr::slice_from_raw_parts`]. We can not otherwise get a fat pointer to
    /// the allocated region.
    ///
    /// [`ptr::slice_from_raw_parts`]: https://github.com/rust-lang/rust/issues/36925
    /// [`ManuallyDrop::drop`]: https://doc.rust-lang.org/beta/std/mem/struct.ManuallyDrop.html#method.drop
    ///
    /// TODO: will be deprecated sooner or later in favor of a method that does not move the
    /// resource on failure.
    // #[deprecated = "Use leak_box and initialize it with the value. This does not move the value in the failure case."]
    pub fn leak<V>(&self, val: V) -> Result<&mut V, LeakError<V>> {
        match self.get::<V>() {
            // SAFETY: Just allocated this for a `V`.
            Some(alloc) => Ok(unsafe { alloc.leak(val) }),
            None => Err(LeakError::new(val, Failure::Exhausted)),
        }
    }

    /// Allocate a value with a precise location.
    ///
    /// See [`leak`] for basics on allocation of values.
    ///
    /// The level is an identifer for a base location (more at [`level`]). This will succeed if it
    /// can be allocate exactly at the expected location.
    ///
    /// This method will return the new level of the slab allocator. A next allocation at the
    /// returned level will be placed next to this allocation, only separated by necessary padding
    /// from alignment. In particular, this is the same strategy as applied for the placement of
    /// `#[repr(C)]` struct members. (Except for the final padding at the last member to the full
    /// struct alignment.)
    ///
    /// ## Usage
    ///
    /// ```
    /// use static_alloc::Bump;
    ///
    /// let local: Bump<[u64; 3]> = Bump::uninit();
    ///
    /// let base = local.level();
    /// let (one, level) = local.leak_at(1_u64, base).unwrap();
    /// // Will panic when an allocation happens in between.
    /// let (two, _) = local.leak_at(2_u64, level).unwrap();
    ///
    /// assert_eq!((one as *const u64).wrapping_offset(1), two);
    /// ```
    ///
    /// [`leak`]: #method.leak
    /// [`level`]: #method.level
    ///
    /// TODO: will be deprecated sooner or later in favor of a method that does not move the
    /// resource on failure.
    ///
    // #[deprecated = "Use leak_box_at and initialize it with the value. This does not move the value in the failure case."]
    pub fn leak_at<V>(&self, val: V, level: Level) -> Result<(&mut V, Level), LeakError<V>> {
        let alloc = match self.get_at::<V>(level) {
            Ok(alloc) => alloc,
            Err(err) => return Err(LeakError::new(val, err)),
        };

        // SAFETY: Just allocated this for a `V`.
        let level = alloc.level;
        let mutref = unsafe { alloc.leak(val) };
        Ok((mutref, level))
    }

    /// 'Allocate' a ZST.
    fn zst_fake_alloc<Z>(&self) -> Allocation<'_, Z> {
        Allocation::for_zst(self.level())
    }

    /// Try to bump the monotonic, atomic consume counter.
    ///
    /// This is the only place doing shared modification to `self.consumed`.
    ///
    /// Returns `Ok` if the consume counter was as expected. Monotonicty and atomicity guarantees
    /// to the caller that no overlapping range can succeed as well. This allocates the range to
    /// the caller.
    ///
    /// Returns the observed consume counter in an `Err` if it was not as expected.
    ///
    /// ## Panics
    /// This function panics if either argument exceeds the byte length of the underlying memory.
    /// It also panics if the expected value is larger than the new value.
    fn bump(&self, expect_consumed: usize, new_consumed: usize) -> Result<(), usize> {
        assert!(expect_consumed <= new_consumed);
        assert!(new_consumed <= mem::size_of::<T>());

        self.consumed
            .compare_exchange(
                expect_consumed,
                new_consumed,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .map(drop)
    }
}

impl<'alloc, T> Allocation<'alloc, T> {
    /// Write a value into the allocation and leak it.
    ///
    /// ## Safety
    ///
    /// Must have been allocated for a layout that fits the layout of T previously. The pointer
    /// must not be aliased.
    ///
    /// ## Usage
    ///
    /// Consider the alternative [`Bump::leak`] to safely allocate and directly leak a value.
    ///
    /// [`Bump::leak`]: struct.Bump.html#method.leak
    pub unsafe fn leak(self, val: T) -> &'alloc mut T {
        // The pointer is not borrowed and valid as guaranteed by the caller.
        core::ptr::write(self.ptr.as_ptr(), val);
        &mut *self.ptr.as_ptr()
    }

    /// Write a value into the allocation and own it.
    ///
    /// ## Safety
    ///
    /// Must have been allocated for a layout that fits the layout of T previously. The pointer
    /// must not be aliased.
    ///
    /// ## Usage
    ///
    /// Consider the alternative [`Bump::leak`] to safely allocate and directly leak a value.
    ///
    /// [`Bump::leak`]: struct.Bump.html#method.leak
    pub unsafe fn boxed(self, val: T) -> LeakBox<'alloc, T> {
        // The pointer is not aliased and valid as guaranteed by the caller.
        core::ptr::write(self.ptr.as_ptr(), val);
        // Safety: the instance is valid, was just initialized.
        LeakBox::from_raw(self.ptr.as_ptr())
    }

    /// Convert this into a mutable reference to an uninitialized slot.
    ///
    /// ## Safety
    ///
    /// Must have been allocated for a layout that fits the layout of T previously.
    pub unsafe fn uninit(self) -> &'alloc mut MaybeUninit<T> {
        &mut *self.ptr.cast().as_ptr()
    }

    /// An 'allocation' for an arbitrary ZST, at some arbitrary level.
    pub(crate) fn for_zst(level: Level) -> Self {
        assert!(mem::size_of::<T>() == 0);
        // If `Z` is a ZST, then the stride of any array is equal to 0. Thus, all arrays and slices
        // havee the same layout which only depends on the alignment. If we need a storage for this
        // ZST we just take one of those as our base 'allocation' which can also never be aliased.
        let alloc: &[T; 0] = &[];

        Allocation {
            ptr: NonNull::from(alloc).cast(),
            lifetime: AllocTime::default(),
            level,
        }
    }
}

impl<T> LeakError<T> {
    fn new(val: T, failure: Failure) -> Self {
        LeakError { val, failure }
    }

    /// Inspect the cause of this error.
    pub fn kind(&self) -> Failure {
        self.failure
    }

    /// Retrieve the value that could not be allocated.
    pub fn into_inner(self) -> T {
        self.val
    }
}

// SAFETY: at most one thread gets a pointer to each chunk of data.
unsafe impl<T> Sync for Bump<T> {}

unsafe impl<T> GlobalAlloc for Bump<T> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Bump::alloc(self, layout)
            .map(NonNull::as_ptr)
            .unwrap_or_else(null_mut)
    }

    unsafe fn realloc(&self, ptr: *mut u8, current: Layout, new_size: usize) -> *mut u8 {
        let current = NonZeroLayout::from_layout(current.into()).unwrap();
        // As guaranteed, `new_size` is greater than 0.
        let new_size = core::num::NonZeroUsize::new_unchecked(new_size);

        let target = match layout_reallocated(current, new_size) {
            Some(target) => target,
            None => return core::ptr::null_mut(),
        };

        // Construct an allocation. This is not safe in general but the lifetime is not important.
        let fake = alloc_traits::Allocation {
            ptr: NonNull::new_unchecked(ptr),
            layout: current,
            lifetime: AllocTime::default(),
        };

        alloc_traits::LocalAlloc::realloc(self, fake, target)
            .map(|alloc| alloc.ptr.as_ptr())
            .unwrap_or_else(core::ptr::null_mut)
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // We are a slab allocator and do not deallocate.
    }
}

fn layout_reallocated(
    layout: NonZeroLayout,
    target: core::num::NonZeroUsize,
) -> Option<NonZeroLayout> {
    // This may not be a valid layout.
    let layout = Layout::from_size_align(target.get(), layout.align()).ok()?;
    // This must succeed though, as the size was non-zero.
    Some(NonZeroLayout::from_layout(layout.into()).unwrap())
}

unsafe impl<'alloc, T> LocalAlloc<'alloc> for Bump<T> {
    fn alloc(&'alloc self, layout: NonZeroLayout) -> Option<alloc_traits::Allocation<'alloc>> {
        let raw_alloc = Bump::get_layout(self, layout.into())?;
        Some(alloc_traits::Allocation {
            ptr: raw_alloc.ptr,
            layout,
            lifetime: AllocTime::default(),
        })
    }

    // TODO: alloc zeroed if the constructor was `Self::zeroed()`

    /// Reallocates if the layout is strictly smaller and the allocation aligned.
    ///
    /// Note that this may succeed spuriously if the previous allocation is incidentally aligned to
    /// a larger alignment than had been request.
    ///
    /// Also not, reallocating to a smaller layout is NOT useless.
    ///
    /// It confirms that this allocator does not need the allocated layout to re/deallocate.
    /// Otherwise, even reallocating to a strictly smaller layout would be impossible without
    /// storing the prior layout.
    unsafe fn realloc(
        &'alloc self,
        alloc: alloc_traits::Allocation<'alloc>,
        layout: NonZeroLayout,
    ) -> Option<alloc_traits::Allocation<'alloc>> {
        if alloc.ptr.as_ptr() as usize % layout.align() == 0 && alloc.layout.size() >= layout.size()
        {
            // Obvious fit, nothing to do.
            return Some(alloc_traits::Allocation {
                ptr: alloc.ptr,
                layout,
                lifetime: alloc.lifetime,
            });
        }

        // TODO: we could try to allocate at the exact level that the allocation ends. If this
        // succeeds, there is no copying necessary. This was the point of `Level` anyways.

        let new_alloc = LocalAlloc::alloc(self, layout)?;
        core::ptr::copy_nonoverlapping(
            alloc.ptr.as_ptr(),
            new_alloc.ptr.as_ptr(),
            layout.size().min(alloc.layout.size()).into(),
        );
        // No dealloc.
        return Some(new_alloc);
    }

    unsafe fn dealloc(&'alloc self, _: alloc_traits::Allocation<'alloc>) {
        // We are a slab allocator and do not deallocate.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zst_no_drop() {
        #[derive(Debug)]
        struct PanicOnDrop;

        impl Drop for PanicOnDrop {
            fn drop(&mut self) {
                panic!("No instance of this should ever get dropped");
            }
        }

        let alloc = Bump::<()>::uninit();
        let _ = alloc.leak(PanicOnDrop).unwrap();
    }
}
