//! This module contains an owning wrapper of a leaked struct.
//!
//! FIXME(breaking): Naming. `leaking` implies the `Drop` of the value as well but we do the
//! precise opposite.
use core::pin::Pin;
use alloc_traits::AllocTime;

use core::{
    alloc::Layout,
    fmt,
    hash,
    marker::PhantomData,
    mem::{ManuallyDrop, MaybeUninit},
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
};

/// Zero-sized marker struct that allows running one or several methods.
///
/// This ensures that allocation does not exceed certain limits that would likely blow the stack
/// and run into Rust's canary, this aborting the process.
pub struct Alloca<T> {
    marker: PhantomData<[T]>,
    len: usize,
}

impl<T> Alloca<T> {
    /// Try to create a representation, that allows functions with dynamically stack-allocated
    /// slices.
    pub fn new(len: usize) -> Option<Self> {
        // Check that it's okay to create the padded layout. This is pure so it will again work
        // when we try during `run`.
        let _padded_layout = Layout::array::<T>(len + 1).ok()?;
        Some(Alloca {
            marker: PhantomData,
            len,
        })
    }

    fn padded_layout(&self) -> Layout {
        Layout::array::<T>(self.len + 1).expect("Checked this in the constructor")
    }

    /// Allocate a slice of elements.
    ///
    /// Please note that instantiating this method relies on the optimizer, to an extent. In
    /// particular we will create stack slots of differing sizes depending on the internal size.
    /// This shouldn't have an effect other than moving the stack pointer for various amounts and
    /// should never have more than one `T` in overhead. However, we can't enforce this. In theory
    /// llvm might still reserve stack space for all variants including a probe and thus
    /// prematurely assume we have hit the bottom of the available stack space. This is not very
    /// likely to occur in practice.
    pub fn run<R>(
        &self,
        run: impl FnOnce(&mut [MaybeUninit<T>]) -> R
    ) -> R {
        // Required size to surely have enough space for an aligned allocation.
        let required_size = self.padded_layout().size();

        if required_size <= 8 {
            self.run_with::<[u64; 1], _, _>(run)
        } else if required_size <= 16 {
            self.run_with::<[u64; 2], _, _>(run)
        } else if required_size <= 32 {
            self.run_with::<[u64; 4], _, _>(run)
        } else if required_size <= 64 {
            self.run_with::<[u64; 8], _, _>(run)
        } else if required_size <= 128 {
            self.run_with::<[u64; 16], _, _>(run)
        } else if required_size <= 256 {
            self.run_with::<[u64; 32], _, _>(run)
        } else if required_size <= 512 {
            self.run_with::<[u64; 64], _, _>(run)
        } else if required_size <= 1024 {
            self.run_with::<[u64; 128], _, _>(run)
        } else if required_size <= 2048 {
            self.run_with::<[u64; 256], _, _>(run)
        } else if required_size <= (1 << 12) {
            self.run_with::<[u64; 512], _, _>(run)
        } else if required_size <= (1 << 13) {
            self.run_with::<[u64; 1 << 10], _, _>(run)
        } else if required_size <= (1 << 14) {
            self.run_with::<[u64; 1 << 11], _, _>(run)
        } else if required_size <= (1 << 15) {
            self.run_with::<[u64; 1 << 12], _, _>(run)
        } else if required_size <= (1 << 16) {
            self.run_with::<[u64; 1 << 13], _, _>(run)
        } else if required_size <= (1 << 17) {
            self.run_with::<[u64; 1 << 14], _, _>(run)
        } else if required_size <= (1 << 18) {
            self.run_with::<[u64; 1 << 15], _, _>(run)
        } else if required_size <= (1 << 19) {
            self.run_with::<[u64; 1 << 16], _, _>(run)
        } else if required_size <= (1 << 20) {
            self.run_with::<[u64; 1 << 17], _, _>(run)
        } else {
            panic!("Stack allocation is too big");
        }
    }

    fn run_with<I, R, F:FnOnce(&mut [MaybeUninit<T>]) -> R>(
        &self,
        run: F
    ) -> R {
        use crate::unsync::Bump;
        let mem = Bump::<I>::uninit();
        let slot = mem.bump_array::<T>(self.len).unwrap();
        run(LeakBox::leak(slot))
    }
}

/// Represents an allocation within a Bump.
///
/// This is an owning pointer comparable to `Box`. It drops the contained value when it is dropped
/// itself. The difference is that no deallocation logic is ever executed.
///
/// FIXME(non-breaking): the name is rather confusing. Maybe it should be `BumpBox` or `RefBox`?
/// Not `StackBox` because the value's location in memory is not the defining feature.
///
/// # Usage
///
/// This box can be used to manage one valid instance constructed within the memory provided by a
/// `MaybeUninit` instance.
///
/// ```
/// use core::mem::MaybeUninit;
/// use static_alloc::leaked::LeakBox;
///
/// let mut storage = MaybeUninit::uninit();
/// let leak_box = LeakBox::from(&mut storage);
/// // The string itself is not managed by `static_alloc`.
/// let mut instance = LeakBox::write(leak_box, String::new());
///
/// instance.push_str("Hello world!");
/// ```
///
/// This box is the result of allocating from one of the `Bump` allocators using its explicit API.
///
/// Being a box-like type, an `Option` has the same size.
///
/// ```
/// use core::mem::size_of;
/// use static_alloc::leaked::LeakBox;
///
/// type Boxed = LeakBox<'static, usize>;
/// type Optional = Option<Boxed>;
///
/// assert_eq!(size_of::<Boxed>(), size_of::<Optional>());
/// ```
///
/// TODO: On nightly the inner type should be [unsizable][unsize-coercion].
///
/// [unsize-coercion]: https://doc.rust-lang.org/reference/type-coercions.html#coercion-types
pub struct LeakBox<'ctx, T: ?Sized> {
    #[allow(unused)]
    lifetime: AllocTime<'ctx>,
    // Covariance should be OK.
    pointer: NonNull<T>,
}

impl<'ctx, T> LeakBox<'ctx, T> {
    /// Construct from a raw pointer.
    ///
    /// # Safety
    ///
    /// The allocation must be valid for a write of the value. The memory must also outlive the
    /// lifetime `'ctx` and pointer must not be aliased by any other reference for that scope.
    pub(crate) unsafe fn new_from_raw_non_null(
        pointer: NonNull<T>,
        val: T,
        lifetime: AllocTime<'ctx>,
    ) -> Self {
        // SAFETY:
        // * `ptr` points to an allocation with correct layout for `V`.
        // * It is valid for write as it is the only pointer to it.
        // * The allocation lives for at least `'ctx`.
        core::ptr::write(pointer.as_ptr(), val);
        Self { pointer, lifetime, }
    }
}

impl<'ctx, T: ?Sized> LeakBox<'ctx, T> {
    /// Retrieve the raw pointer wrapped by this box.
    ///
    /// After this method the caller is responsible for managing the value in the place behind the
    /// pointer. It will need to be dropped manually.
    ///
    /// # Usage
    ///
    /// You might manually drop the contained instance at a later point.
    ///
    /// ```
    /// use static_alloc::{Bump, leaked::LeakBox};
    ///
    /// # fn fake() -> Option<()> {
    /// let bump: Bump<[usize; 128]> = Bump::uninit();
    /// let leak_box = bump.leak_box(String::from("Hello"))?;
    /// let ptr = LeakBox::into_raw(leak_box);
    ///
    /// unsafe {
    ///     core::ptr::drop_in_place(ptr);
    /// }
    /// # Some(()) }
    /// ```
    ///
    /// An alternative is to later re-wrap the pointer
    ///
    /// ```
    /// use static_alloc::{Bump, leaked::LeakBox};
    ///
    /// # fn fake() -> Option<()> {
    /// let bump: Bump<[usize; 128]> = Bump::uninit();
    /// let leak_box = bump.leak_box(String::from("Hello"))?;
    /// let ptr = LeakBox::into_raw(leak_box);
    ///
    /// unsafe {
    ///     let _ = LeakBox::from_raw(ptr);
    /// };
    /// # Some(()) }
    /// ```
    pub fn into_raw(this: Self) -> *mut T {
        let this = ManuallyDrop::new(this);
        this.pointer.as_ptr()
    }

    /// Wrap a raw pointer.
    ///
    /// The most immediate use is to rewrap a pointer returned from [`into_raw`].
    ///
    /// [`into_raw`]: #method.into_raw
    ///
    /// # Safety
    ///
    /// The pointer must point to a valid instance of `T` that is not aliased by any other
    /// reference for the lifetime `'ctx`. In particular it must be valid aligned and initialized.
    /// Dropping this `LeakBox` will drop the instance, which the caller must also guarantee to be
    /// sound.
    pub unsafe fn from_raw(pointer: *mut T) -> Self {
        debug_assert!(!pointer.is_null(), "Null pointer passed to LeakBox::from_raw");
        LeakBox {
            lifetime: AllocTime::default(),
            pointer: NonNull::new_unchecked(pointer),
        }
    }

    /// Wrap a mutable reference to a complex value as if it were owned.
    ///
    /// # Safety
    ///
    /// The value must be owned by the caller. That is, the mutable reference must not be used
    /// after the `LeakBox` is dropped. In particular the value must not be dropped by the caller.
    ///
    /// # Example
    ///
    /// ```rust
    /// use core::mem::ManuallyDrop;
    /// use static_alloc::leaked::LeakBox;
    ///
    /// fn with_stack_drop<T>(val: T) {
    ///     let mut val = ManuallyDrop::new(val);
    ///     // Safety:
    ///     // - Shadows the variable, rendering the prior inaccessible.
    ///     // - Dropping is now the responsibility of `LeakBox`.
    ///     let val = unsafe { LeakBox::from_mut_unchecked(&mut *val) };
    /// }
    ///
    /// // Demonstrate that it is correctly dropped.
    /// let variable = core::cell::RefCell::new(0);
    /// with_stack_drop(variable.borrow_mut());
    /// assert!(variable.try_borrow_mut().is_ok());
    /// ```
    #[allow(unused_unsafe)]
    pub unsafe fn from_mut_unchecked(val: &'ctx mut T) -> Self {
        // SAFETY:
        // * Is valid instance
        // * Not aliased as by mut reference
        // * Dropping soundness is guaranteed by the caller.
        // * We don't invalidate any value, nor can the caller.
        unsafe { LeakBox::from_raw(val) }
    }

    /// Leak the instances as a mutable reference.
    ///
    /// After calling this method the value is no longer managed by `LeakBox`. Its Drop impl will
    /// not be automatically called.
    ///
    /// # Usage
    ///
    /// ```
    /// use static_alloc::{Bump, leaked::LeakBox};
    ///
    /// # fn fake() -> Option<()> {
    /// let bump: Bump<[usize; 128]> = Bump::uninit();
    /// let leak_box = bump.leak_box(String::from("Hello"))?;
    ///
    /// let st: &mut String = LeakBox::leak(leak_box);
    /// # Some(()) }
    /// ```
    ///
    /// You can't leak past the lifetime of the allocator.
    ///
    /// ```compile_fail
    /// # use static_alloc::{Bump, leaked::LeakBox};
    /// # fn fake() -> Option<()> {
    /// let bump: Bump<[usize; 128]> = Bump::uninit();
    /// let leak_box = bump.leak_box(String::from("Hello"))?;
    /// let st: &mut String = LeakBox::leak(leak_box);
    ///
    /// drop(bump);
    /// // error[E0505]: cannot move out of `bump` because it is borrowed
    /// st.to_lowercase();
    /// //-- borrow later used here
    /// # Some(()) }
    /// ```
    pub fn leak<'a>(this: Self) -> &'a mut T
        where 'ctx: 'a
    {
        let pointer = LeakBox::into_raw(this);
        // SAFETY:
        // * The LeakBox type guarantees this is initialized and not mutably aliased.
        // * For the lifetime 'a which is at most 'ctx.
        unsafe { &mut *pointer }
    }
}

impl<T: 'static> LeakBox<'static, T> {
    /// Pin an instance that's leaked for the remaining program runtime.
    ///
    /// After calling this method the value can only safely be referenced mutably if it is `Unpin`,
    /// otherwise it is only accessible behind a `Pin`. Note that this does _not_ imply that the
    /// `Drop` glue, or explicit `Drop`-impl, is guaranteed to run.
    ///
    /// # Usage
    ///
    /// A decent portion of futures must be _pinned_ before the can be awaited inside another
    /// future. In particular this is required for self-referential futures that store pointers
    /// into their own object's memory. This is the case for the future type of an `asnyc fn` if
    /// there are potentially any stack references when it is suspended/waiting on another future.
    /// Consider this example:
    ///
    /// ```compile_fail
    /// use static_alloc::{Bump, leaked::LeakBox}; 
    ///
    /// async fn example(x: usize) -> usize {
    ///     // Holding reference across yield point.
    ///     // This requires pinning to run this future.
    ///     let y = &x;
    ///     core::future::ready(()).await;
    ///     *y
    /// }
    ///
    /// static POOL: Bump<[usize; 128]> = Bump::uninit();
    /// let mut future = POOL.leak_box(example(0))
    ///     .expect("Enough space for small async fn");
    ///
    /// let usage = async move {
    /// // error[E0277]: `GenFuture<[static generator@src/leaked.rs …]>` cannot be unpinned
    ///     let _ = (&mut *future).await;
    /// };
    /// ```
    ///
    /// This method can be used to pin instances allocated from a global pool without requiring the
    /// use of a macro or unsafe on the caller's part. Now, with the correct usage of `into_pin`:
    ///
    /// ```
    /// use static_alloc::{Bump, leaked::LeakBox}; 
    ///
    /// async fn example(x: usize) -> usize {
    ///     // Holding reference across yield point.
    ///     // This requires pinning to run this future.
    ///     let y = &x;
    ///     core::future::ready(()).await;
    ///     *y
    /// }
    ///
    /// static POOL: Bump<[usize; 128]> = Bump::uninit();
    /// let future = POOL.leak_box(example(0))
    ///     .expect("Enough space for small async fn");
    ///
    /// // PIN this future!
    /// let mut future = LeakBox::into_pin(future);
    ///
    /// let usage = async move {
    ///     let _ = future.as_mut().await;
    /// };
    /// ```
    pub fn into_pin(this: Self) -> Pin<Self> {
        // SAFETY:
        // * This memory is valid for `'static` duration, independent of the fate of `this` and
        //   even when it is forgotten. This trivially implies that any Drop is called before the
        //   memory is invalidated, as required by `Pin`.
        unsafe { Pin::new_unchecked(this) }
    }
}

impl<'ctx, T> LeakBox<'ctx, T> {
    /// Remove the value, forgetting the box in the process.
    ///
    /// This is similar to dereferencing a box (`*leak_box`) but no deallocation is involved. This
    /// becomes useful when the allocator turns out to have too short of a lifetime.
    ///
    /// # Usage
    ///
    /// You may want to move a long-lived value out of the current scope where it's been allocated.
    ///
    /// ```
    /// # use core::cell::RefCell;
    /// use static_alloc::{Bump, leaked::LeakBox};
    ///
    /// let cell = RefCell::new(0usize);
    ///
    /// let guard = {
    ///     let bump: Bump<[usize; 128]> = Bump::uninit();
    ///
    ///     let mut leaked = bump.leak_box(cell.borrow_mut()).unwrap();
    ///     **leaked = 1usize;
    ///
    ///     // Take the value, allowing use independent of the lifetime of bump
    ///     LeakBox::take(leaked)
    /// };
    ///
    /// assert!(cell.try_borrow().is_err());
    /// drop(guard);
    /// assert!(cell.try_borrow().is_ok());
    /// ```
    pub fn take(this: Self) -> T {
        // Do not drop this.
        let this = ManuallyDrop::new(this);
        // SAFETY:
        // * `ptr` points to an initialized allocation according to the constructors of `LeakBox`.
        // * The old value is forgotten and no longer dropped.
        unsafe { core::ptr::read(this.pointer.as_ptr()) }
    }

    /// Wrap a mutable reference to a trivial value as if it were a box.
    ///
    /// This is safe because such values can not have any Drop code and can be duplicated at will.
    ///
    /// The usefulness of this operation is questionable but the author would be delighted to hear
    /// about any actual use case.
    pub fn from_mut(val: &'ctx mut T) -> Self
    where
        T: Copy
    {
        // SAFETY:
        // * Is valid instance
        // * Not aliased as by mut reference
        // * Dropping is a no-op
        // * We don't invalidate anyones value
        unsafe { LeakBox::from_raw(val) }
    }
}

impl<'ctx, T> LeakBox<'ctx, MaybeUninit<T>> {
    /// Write a value into this box, initializing it.
    ///
    /// This can be used to delay the computation of a value until after an allocation succeeded
    /// while maintaining all types necessary for a safe initialization.
    ///
    /// # Usage
    ///
    /// ```
    /// # fn some_expensive_operation() -> [u8; 4] { [0u8; 4] }
    /// # use core::mem::MaybeUninit;
    /// #
    /// # fn fake_main() -> Option<()> {
    /// #
    /// use static_alloc::{Bump, leaked::LeakBox};
    ///
    /// let bump: Bump<[usize; 128]> = Bump::uninit();
    /// let memory = bump.leak_box(MaybeUninit::uninit())?;
    ///
    /// let value = LeakBox::write(memory, some_expensive_operation());
    /// # Some(()) } fn main() {}
    /// ```
    pub fn write(mut this: Self, val: T) -> LeakBox<'ctx, T> {
        unsafe {
            // SAFETY: MaybeUninit<T> is valid for writing a T.
            ptr::write(this.as_mut_ptr(), val);
            // SAFETY: initialized by the write before.
            LeakBox::assume_init(this)
        }
    }

    /// Converts to `LeakBox<T>`.
    ///
    /// # Safety
    ///
    /// The value must have been initialized as required by `MaybeUninit::assume_init`. Calling
    /// this when the content is not yet fully initialized causes immediate undefined behavior.
    pub unsafe fn assume_init(this: Self) -> LeakBox<'ctx, T> {
        LeakBox {
            pointer: this.pointer.cast(),
            lifetime: this.lifetime,
        }
    }
}

impl<'ctx, T: ?Sized> Deref for LeakBox<'ctx, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: constructor guarantees this is initialized and not mutably aliased.
        unsafe { self.pointer.as_ref() }
    }
}

impl<'ctx, T: ?Sized> DerefMut for LeakBox<'ctx, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: constructor guarantees this is initialized and not aliased.
        unsafe { self.pointer.as_mut() }
    }
}

impl<T: ?Sized> Drop for LeakBox<'_, T> {
    fn drop(&mut self) {
        // SAFETY: constructor guarantees this was initialized.
        unsafe { ptr::drop_in_place(self.pointer.as_ptr()) }
    }
}

/// Construct a LeakBox to an existing MaybeUninit.
///
/// The MaybeUninit type is special in that we can treat any unique reference to an owned value as
/// an owned value itself since it has no representational invariants.
impl<'ctx, T> From<&'ctx mut MaybeUninit<T>> for LeakBox<'ctx, MaybeUninit<T>> {
    fn from(uninit: &'ctx mut MaybeUninit<T>) -> Self {
        // SAFETY:
        // * An instance of MaybeUninit is always valid.
        // * The mut references means it can not be aliased.
        // * Dropping a MaybeUninit is a no-op and can not invalidate any validity or security
        //   invariants of this MaybeUninit or the contained T.
        unsafe { LeakBox::from_raw(uninit) }
    }
}

/// Construct a LeakBox to an existing slice of MaybeUninit.
impl<'ctx, T> From<&'ctx mut [MaybeUninit<T>]> for LeakBox<'ctx, [MaybeUninit<T>]> {
    fn from(uninit: &'ctx mut [MaybeUninit<T>]) -> Self {
        // SAFETY:
        // * An instance of MaybeUninit is always valid.
        // * The mut references means it can not be aliased.
        // * Dropping a MaybeUninit is a no-op and can not invalidate any validity or security
        //   invariants of this MaybeUninit or the contained T.
        unsafe { LeakBox::from_raw(uninit) }
    }
}

impl<T: ?Sized> AsRef<T> for LeakBox<'_, T> {
    fn as_ref(&self) -> &T {
        &**self
    }
}

impl<T: ?Sized> AsMut<T> for LeakBox<'_, T> {
    fn as_mut(&mut self) -> &mut T {
        &mut **self
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for LeakBox<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for LeakBox<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl<T: ?Sized> fmt::Pointer for LeakBox<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.pointer.fmt(f)
    }
}

impl<T: hash::Hash + ?Sized> hash::Hash for LeakBox<'_, T> {
    fn hash<H: hash::Hasher>(&self, h: &mut H) {
        self.as_ref().hash(h)
    }
}

// TODO: iterators, read, write?
