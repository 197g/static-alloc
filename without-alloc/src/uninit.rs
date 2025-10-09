//! Safe abstractions around pointing at uninitialized memory without references.
//!
//! This solves two issues beyond the standard library: Firstly, `MaybeUninit` does not permitted
//! unsized types. Secondly, a strict interpretation of pointer provenance implies that once a
//! reference is created our access is restricted the memory referred to in it. This spoils our
//! access to any memory in a potential tail of the allocation, which is wasteful.
//!
//! It is potentially **UB** to have references to uninitialized memory even if such a reference is
//! not 'used' in any particular manner. See [the discussion of the unsafe working group][wg-ref].
//!
//! TODO: In some next version we'd like to switch to `&'a UnsafeCell<MaybeUninit<T>>` here, or
//! even replace the UnsafeCell once we've replace the `view` attribute in Uninit. But alas we're
//! not permitted to have an unsized parameter to `MaybeUninit`.
//!
//! More work using unsized type parameter would in particular make it redundant to store the
//! length as we could refer to memory with a wrapper `Byte(u8)` and the `T = [Byte]` instantiation
//! for the type parameter of `Uninit`/`UninitView`. Storing additional bytes would be a wrapper
//! around the main interface.
//!
//! [wg-ref]: https://github.com/rust-lang/unsafe-code-guidelines/issues/77
use core::alloc::Layout;
use core::marker::PhantomData;
use core::{fmt, mem, ptr, slice};

use crate::boxed::Box;
use unsize::CoerciblePtr;

/// Points to an uninitialized place but would otherwise be a valid reference.
///
/// This is a `&mut`-like struct that is somewhat of a pendant to `MaybeUninit`. It makes it
/// possible to deal with uninitialized allocations without requiring an `unsafe` block
/// initializing them and offers a much safer interface for partial initialization and layout
/// calculations than raw pointers.
///
/// Note that it also supports slices which means it does not use `MaybeUninit` internally but
/// offers conversion where necessary.
///
/// ## Usage
///
/// The basic usage is also interacting with `MaybeUninit`:
///
/// ```
/// # #[derive(Default)]
/// # struct MyStruct { };
/// use core::mem::MaybeUninit;
/// use without_alloc::Uninit;
///
/// let mut alloc: MaybeUninit<MyStruct> = MaybeUninit::uninit();
/// let uninit = Uninit::from_maybe_uninit(&mut alloc);
///
/// // notice: no unsafe
/// let instance: &mut MyStruct = uninit.init(MyStruct::default());
/// ```
///
/// But since we are working on arbitrary uninitialized memory it is also possible to reuse the
/// structure for completely arbitrary other types. Just note that there is no integrated mechanis
/// for calling `Drop`.
///
#[cfg_attr(miri, doc = "```no_run")] // This can not work under miri yet, it relies on align_offset
#[cfg_attr(not(miri), doc = "```")]
/// use core::mem::MaybeUninit;
/// use without_alloc::Uninit;
///
/// // Just a generic buffer.
/// let mut alloc: MaybeUninit<[u32; 1024]> = MaybeUninit::uninit();
/// let uninit = Uninit::from_maybe_uninit(&mut alloc);
///
/// // Now use the first `u32` for a counter:
/// let mut counter = uninit.cast().expect("valid align");
/// let mut tail = counter.split_to_fit();
/// let counter: &mut u32 = counter.init(0);
///
/// // And some more for a few `u64`.
/// // Note that these are not trivially aligned, but `Uninit` does that for us.
/// let mut values = tail.split_cast().expect("enough space for split");
/// // No more use, so don't bother with `split_to_fit` and just `init`.
/// let values: &mut [u64; 2] = values.init([0xdead, 0xbeef]);
/// ```
#[must_use = "This is a pointer-like type that has no effect on its own. Use `init` to insert a value."]
pub struct Uninit<'a, T: ?Sized> {
    /// The underlying view.
    ///
    /// Uninit additional imposes on it that the underlying memory is mutable.
    view: UninitView<'a, T>,

    /// Reminder for every construction.
    mutable: PhantomData<&'a mut ()>,
}

/// A non-mutable view on a region used in an [`Uninit`].
///
/// Makes it possible to utilize the traversal methods (`split*`, `cast*`, ..) without requiring a
/// mutable reference to the original `Uninit`. It will also never expose mutable pointers or
/// accidentally offer an aliased mutable reference. Prefer this to instead avoiding the borrow of
/// the `Uninit` and manually managing pointers to the region.
///
/// [`Uninit`]: ./struct.Uninit.html
#[must_use = "This is a pointer-like type that has no effect on its own."]
pub struct UninitView<'a, T: ?Sized> {
    /// Pointer to the start of the region.
    ///
    /// Note that `len` is always at least as large as the (minimum) size of `T`. Furthermore, the
    /// pointer is always correctly aligned to a `T`.
    ptr: ptr::NonNull<T>,

    /// The actual length *in bytes*.
    ///
    /// May be larger than required.
    len: usize,

    /// Virtual lifetime to make this behave more similar to references.
    ///
    /// This borrows structures that hand out `Uninit` allocations.
    lifetime: PhantomData<&'a ()>,

    /// We'll be holding an actual `NonNull<T>` in the future (when dynamically sized pointers to
    /// slices are more ergonomic). For now, just type ourselves.
    typed: PhantomData<*mut T>,
}

impl Uninit<'_, ()> {
    /// Create a uninit pointer from raw memory.
    ///
    /// ## Safety
    /// A valid allocation must exist at the pointer with length at least `len`. There must be *no*
    /// references aliasing the memory location, and it must be valid to write uninitialized bytes
    /// into arbitrary locations of the region.
    ///
    /// In particular, it is **UB** to create this from a reference to a variable of a type for
    /// which a completely uninitialized content is not valid. The standard type for avoiding the
    /// UB is `core::mem::MaybeUninit`.
    ///
    /// When in doubt, refactor code such that utilization of `from_maybe_uninit` is possible.
    pub unsafe fn from_memory(ptr: ptr::NonNull<u8>, len: usize) -> Self {
        Uninit::from_presumed_mutable_view(UninitView {
            ptr: ptr.cast(),
            len,
            lifetime: PhantomData,
            typed: PhantomData,
        })
    }

    /// Split so that the second part fits the layout.
    ///
    /// Return `Ok` if this is possible in-bounds and `Err` if it is not.
    pub fn split_layout(&mut self, layout: Layout) -> Option<Self> {
        self.view
            .split_layout(layout)
            .map(Self::from_presumed_mutable_view)
    }
}

impl<'a> Uninit<'a, ()> {
    fn decast<T: ?Sized>(uninit: Uninit<'a, T>) -> Self {
        Uninit::from_presumed_mutable_view(UninitView {
            ptr: uninit.view.ptr.cast(),
            len: uninit.view.len,
            lifetime: PhantomData,
            typed: PhantomData,
        })
    }

    /// Split so that the tail is aligned and valid for a `U`.
    ///
    /// Return `Ok` if this is possible in-bounds (aligned and enough room for at least one `U`)
    /// and `Err` if it is not. The first tuple element is the `Uninit` pointing to the skipped
    /// memory.
    pub fn split_cast<U>(&mut self) -> Option<Uninit<'a, U>> {
        let split = self.split_layout(Layout::new::<U>())?;
        let cast = split.cast::<U>().unwrap();
        Some(cast)
    }

    /// Split so that the tail is aligned for a slice `[U]`.
    ///
    /// Return `Ok` if this is possible in-bounds and `Err` if it is not. The first tuple element
    /// is the `Uninit` pointing to the skipped memory.
    ///
    /// The length of the slice is the arbitrary amount that fits into the tail of the allocation.
    /// Note that the length always fulfills the safety requirements for `slice::from_raw_parts`
    /// since the `Uninit` must be contained in a single allocation.
    pub fn split_slice<U>(&mut self) -> Option<Uninit<'a, [U]>> {
        let layout = Layout::for_value::<[U]>(&[]);
        let split = self.split_layout(layout)?;
        let cast = split.cast_slice::<U>().unwrap();
        Some(cast)
    }
}

impl<T> Uninit<'_, T> {
    /// Invent a new uninit allocation for a zero-sized type (ZST).
    ///
    /// # Panics
    /// This method panics when the type parameter is not a zero sized type.
    pub fn invent_for_zst() -> Self {
        // SAFETY: zst is always unaliased.
        unsafe { Uninit::from_view(UninitView::invent_for_zst()) }
    }
}

impl<'a, T> Uninit<'a, T> {
    /// Create an `uninit` from a view.
    ///
    /// ## Safety
    /// The caller must prove that the pointed-to memory is mutable and that it is unaliased.
    pub unsafe fn from_view(view: UninitView<'a, T>) -> Self {
        Self::from_presumed_mutable_view(view)
    }

    /// Create an initializable pointer to the inner bytes of a `MaybeUninit`.
    pub fn from_maybe_uninit(mem: &'a mut mem::MaybeUninit<T>) -> Self {
        let ptr = ptr::NonNull::new(mem.as_mut_ptr()).unwrap();
        let raw = unsafe {
            // SAFETY:
            // * unaliased as we had a mutable reference
            // * can write uninitialized bytes as much as we want
            Uninit::from_memory(ptr.cast(), mem::size_of_val(mem))
        };
        raw.cast().unwrap()
    }

    /// Split the uninit slice at a byte boundary.
    ///
    /// Return `Ok` if the location is in-bounds and `Err` if it is out of bounds.
    pub fn split_at_byte(&mut self, at: usize) -> Option<Uninit<'a, ()>> {
        self.view
            .split_at_byte(at)
            .map(Uninit::from_presumed_mutable_view)
    }

    /// Try to cast to an `Uninit` for another type.
    ///
    /// Return `Ok` if the current `Uninit` is suitably aligned and large enough to hold at least
    /// one `U` and `Err` if it is not. Note that the successful result points to unused remaining
    /// memory behind where the instance can be placed.
    ///
    /// Use [`split_to_fit`] to get rid of surplus memory at the end.
    ///
    /// [`split_to_fit`]: #method.split_to_fit
    pub fn cast<U>(self) -> Result<Uninit<'a, U>, Self> {
        self.view
            .cast()
            .map(Uninit::from_presumed_mutable_view)
            .map_err(Self::from_presumed_mutable_view)
    }

    /// Try to cast to an `Uninit` for a slice type.
    ///
    /// Return `Ok` if the current `Uninit` is suitably aligned and large enough to hold at least
    /// one `U` and `Err` if it is not. Note that the successful result points to unused remaining
    /// memory behind where the instances can be placed.
    pub fn cast_slice<U>(self) -> Result<Uninit<'a, [U]>, Self> {
        self.view
            .cast_slice::<U>()
            .map(Uninit::from_presumed_mutable_view)
            .map_err(Self::from_presumed_mutable_view)
    }

    /// Split off the tail that is not required for holding an instance of `T`.
    ///
    /// This operation is idempotent.
    pub fn split_to_fit(&mut self) -> Uninit<'a, ()> {
        self.split_at_byte(mem::size_of::<T>()).unwrap()
    }
}

impl<'a, T: ?Sized> Uninit<'a, T> {
    /// Acquires the underlying *mut pointer.
    pub const fn as_ptr(&self) -> *mut T {
        self.view.ptr.as_ptr()
    }

    /// Acquires the underlying pointer as a `NonNull`.
    pub const fn as_non_null(&self) -> ptr::NonNull<T> {
        self.view.ptr
    }

    /// Dereferences the content.
    ///
    /// The resulting lifetime is bound to self so this behaves "as if" it were actually an
    /// instance of T that is getting borrowed. If a longer lifetime is needed, use `into_ref`.
    ///
    /// # Safety
    /// The pointee must have been initialized through other means.
    pub unsafe fn as_ref(&self) -> &T {
        self.view.as_ref()
    }

    /// Mutably dereferences the content.
    ///
    /// The resulting lifetime is bound to self so this behaves "as if" it were actually an
    /// instance of T that is getting borrowed. If a longer lifetime is needed, use `into_mut`.
    ///
    /// # Safety
    /// The pointee must have been initialized through other means.
    pub unsafe fn as_mut(&mut self) -> &mut T {
        &mut *self.as_ptr()
    }

    /// Turn this into a reference to the content.
    ///
    /// # Safety
    /// The pointee must have been initialized through other means.
    pub unsafe fn into_ref(self) -> &'a T {
        &*self.as_ptr()
    }

    /// Turn this into a mutable reference to the content.
    ///
    /// # Safety
    /// The pointee must have been initialized through other means.
    pub unsafe fn into_mut(self) -> &'a mut T {
        &mut *self.as_ptr()
    }
}

impl<'a, T> Uninit<'a, T> {
    /// Initialize the place and return a reference to the value.
    pub fn init(self, val: T) -> &'a mut T {
        let ptr = self.as_ptr();
        unsafe {
            // SAFETY:
            // * can only create instances where layout of `T` 'fits'
            // * valid for lifetime `'a` (as per `UninitView`).
            // * unaliased for lifetime `'a` (as per own invariant from unsafe constructor). No
            //   other method duplicates the pointer or allows a second `Uninit` without borrowing
            //   the first.
            ptr::write(ptr, val);
            &mut *ptr
        }
    }

    /// Turn this into a reference to standard `MaybeUninit`.
    ///
    /// This is mainly useful for interfacing with other consumers which expect standard library
    /// types. It may also improve ergonomics for writing to the pointee partially initialized
    /// instances of `T` that are obtained via other means.
    ///
    /// Note that the sequence `from_maybe_uninit`, `into_maybe_uninit` is a no-op. The converse is
    /// however not the case, as it will potentially discard unused padding present in the original
    /// `Uninit`.
    pub fn into_maybe_uninit(self) -> &'a mut mem::MaybeUninit<T> {
        // SAFETY: MaybeUninit is a transparent wrapper and need not be initialized.
        unsafe { &mut *(self.as_ptr() as *mut mem::MaybeUninit<T>) }
    }

    /// Read a value from the uninit place without moving it.
    ///
    /// The `Uninit` ensures that the inner pointer is correctly aligned, non-null, and points to a
    /// large enough region for reading a `T`.
    ///
    /// ## Safety
    /// Caller must ensure that the memory is initialized as a valid `T`. It must also avoid double
    /// `Drop`. Basically, a new instance is created.
    pub unsafe fn read(&self) -> T {
        ptr::read(self.as_ptr())
    }

    /// Utilize this `Uninit` allocation for a boxed value.
    ///
    /// Stores the value at the pointed-to location and utilizes the `Box` as a RAII-guard to
    /// properly drop the value when the box itself is dropped.
    pub fn into_box(self, val: T) -> Box<'a, T> {
        Box::new(val, self)
    }
}

impl<'a, T> Uninit<'a, [T]> {
    /// Creates a pointer to an empty slice.
    pub fn empty() -> Self {
        Uninit::from_presumed_mutable_view(UninitView {
            ptr: {
                let base = ptr::NonNull::<T>::dangling().as_ptr();
                let slice = ptr::slice_from_raw_parts_mut(base, 0);
                ptr::NonNull::new(slice).unwrap()
            },
            len: 0,
            lifetime: PhantomData,
            typed: PhantomData,
        })
    }

    /// Create an initializable pointer to the inner bytes of a `MaybeUninit`.
    pub fn from_maybe_uninit_slice(mem: &'a mut [mem::MaybeUninit<T>]) -> Self {
        let size = mem::size_of_val(mem);
        let ptr = ptr::NonNull::from(mem);
        let raw = unsafe {
            // SAFETY:
            // * unaliased as we had a mutable reference
            // * can write uninitialized bytes as much as we want
            Uninit::from_memory(ptr.cast(), size)
        };
        raw.cast_slice().unwrap()
    }

    /// Get the pointer to the first element of the slice.
    ///
    /// If the slice would be empty then the pointer may be the past-the-end pointer as well.
    pub const fn as_begin_ptr(&self) -> *mut T {
        self.view.ptr.as_ptr() as *mut T
    }

    /// Calculate the theoretical capacity of a slice in the pointed-to allocation.
    pub fn capacity(&self) -> usize {
        self.view.capacity()
    }

    /// Split the slice at an index.
    ///
    /// This is the pointer equivalent of `slice::split_at`.
    pub fn split_at(&mut self, at: usize) -> Option<Self> {
        self.view.split_at(at).map(Self::from_presumed_mutable_view)
    }

    /// Get the trailing bytes behind the slice.
    ///
    /// The underlying allocation need not be a multiple of the slice element size which may leave
    /// unusable bytes. This splits these unusable bytes into an untyped `Uninit` which can be
    /// reused arbitrarily.
    ///
    /// This operation is idempotent.
    pub fn shrink_to_fit(&mut self) -> Uninit<'a, ()> {
        Uninit::decast(self.split_at(self.capacity()).unwrap())
    }

    /// Split the first element from the slice.
    ///
    /// This is the pointer equivalent of `slice::split_first`.
    pub fn split_first(&mut self) -> Option<Uninit<'a, T>> {
        let mut part = self.split_at(1)?;
        // Now we are the first part, but we wanted the first to be split off.
        mem::swap(self, &mut part);
        // If it is a valid slice of length 1 it is a valid `T`.
        Some(Uninit::decast(part).cast().unwrap())
    }

    /// Split the last element from the slice.
    ///
    /// This is the pointer equivalent of `slice::split_last`.
    pub fn split_last(&mut self) -> Option<Uninit<'a, T>> {
        // Explicitely wrap here: If capacity is 0 then `0 < size_of::<T> ` and the split will fail.
        let split = self.capacity().wrapping_sub(1);
        let part = self.split_at(split)?;
        // If it is a valid slice of length 1 it is a valid `T`.
        Some(Uninit::decast(part).cast().unwrap())
    }

    /// Turn this into a slice of standard `MaybeUninit`s.
    ///
    /// This is mainly useful for interfacing with other consumers which expect standard library
    /// types. It may also improve ergonomics for writing to the pointee partially initialized
    /// instances of `T` that are obtained via other means.
    ///
    /// Note that the sequence `from_maybe_uninit_slice`, `into_maybe_uninit_slice` is a no-op. The
    /// converse is however not the case, as it will potentially discard unused padding present in
    /// the original `Uninit`.
    pub fn into_maybe_uninit_slice(self) -> &'a mut [mem::MaybeUninit<T>] {
        unsafe {
            // SAFETY: MaybeUninit is a transparent wrapper and need not be initialized.
            slice::from_raw_parts_mut(
                self.as_begin_ptr() as *mut mem::MaybeUninit<T>,
                self.capacity(),
            )
        }
    }
}

impl<'a, T: ?Sized> Uninit<'a, T> {
    /// Create a view to typed uninitialized memory.
    ///
    /// It is given a capacity of memory to which it refers in *bytes*.
    ///
    /// ## Safety
    ///
    /// The `ptr` must describe a valid, *sized* region. Refer to `Layout::for_value_raw` for
    /// details. This criteria is trivially fulfilled for any sized `T`.
    ///
    /// A valid allocation must exist at the pointer with length at least `len`.
    ///
    /// In particular, it is **UB** to create this from a reference to a variable of a type for
    /// which a completely uninitialized content is not valid. The standard type for avoiding the
    /// UB is `core::mem::MaybeUninit`.
    ///
    /// When in doubt, refactor code such that utilization of `from_maybe_uninit` is possible.
    pub unsafe fn new(ptr: ptr::NonNull<T>, len: usize) -> Self {
        Uninit {
            view: UninitView::new(ptr, len),
            mutable: PhantomData,
        }
    }

    /// Return the number of bytes this may view.
    pub fn byte_capacity(&self) -> usize {
        self.view.byte_capacity()
    }

    /// Check if the view fits some layout.
    ///
    /// The `cast` to a type of the provided layout will work without error.
    pub fn fits(&self, layout: Layout) -> bool {
        self.view.fits(layout)
    }

    /// View the same uninit as untyped memory.
    pub fn as_memory(self) -> Uninit<'a, ()> {
        Uninit::decast(self)
    }

    /// A private version of the unsafe `from_view`.
    ///
    /// This must never be exposed.
    fn from_presumed_mutable_view(view: UninitView<'a, T>) -> Self {
        Uninit {
            view,
            mutable: PhantomData,
        }
    }

    /// Borrow a view of the `Uninit` region.
    ///
    /// This is the equivalent of `&*mut_ref as *const _` but never runs afoul of accidentally
    /// creating an actual reference.
    pub fn borrow(&self) -> UninitView<'_, T> {
        self.view
    }

    /// Borrow the `Uninit` region for a shorter duration.
    ///
    /// This is the equivalent of `&mut *mut_ref as *mut _` but never runs afoul of accidentally
    /// creating an actual reference.
    pub fn borrow_mut(&mut self) -> Uninit<'_, T> {
        Uninit::from_presumed_mutable_view(self.view)
    }

    /// Get the byte size of the total allocation.
    pub const fn size(&self) -> usize {
        self.view.size()
    }
}
impl UninitView<'_, ()> {
    /// Create a uninit view from raw memory.
    ///
    /// ## Safety
    /// A valid allocation must exist at the pointer with length at least `len`.
    ///
    /// In particular, it is **UB** to create this from a reference to a variable of a type for
    /// which a completely uninitialized content is not valid. The standard type for avoiding the
    /// UB is `core::mem::MaybeUninit`.
    ///
    /// When in doubt, refactor code such that utilization of `from_maybe_uninit` is possible.
    pub unsafe fn from_memory(ptr: ptr::NonNull<u8>, len: usize) -> Self {
        UninitView {
            ptr: ptr.cast(),
            len,
            lifetime: PhantomData,
            typed: PhantomData,
        }
    }

    /// Split so that the second part fits the layout.
    ///
    /// See [`Uninit::split_layout`] for more details.
    ///
    /// [`Uninit::split_layout`]: ./struct.Uninit.html#method.split_layout
    pub fn split_layout(&mut self, layout: Layout) -> Option<Self> {
        let align = self.ptr.as_ptr().align_offset(layout.align());

        let aligned_len = self
            .len
            .checked_sub(align)
            .and_then(|len| len.checked_sub(layout.size()));

        if aligned_len.is_none() {
            return None;
        }

        let aligned = self.split_at_byte(align)?;
        assert!(aligned.fits(layout));
        Some(aligned)
    }
}

impl<'a> UninitView<'a, ()> {
    fn decast<T: ?Sized>(view: UninitView<'a, T>) -> Self {
        UninitView {
            ptr: view.ptr.cast(),
            len: view.len,
            lifetime: PhantomData,
            typed: PhantomData,
        }
    }

    /// Split so that the tail is aligned and valid for a `U`.
    pub fn split_cast<U>(&mut self) -> Option<UninitView<'a, U>> {
        let split = self.split_layout(Layout::new::<U>())?;
        let cast = split.cast::<U>().unwrap();
        Some(cast)
    }

    /// Split so that the tail is aligned for a slice `[U]`.
    pub fn split_slice<U>(&mut self) -> Option<UninitView<'a, [U]>> {
        let layout = Layout::for_value::<[U]>(&[]);
        let split = self.split_layout(layout)?;
        let cast = split.cast_slice::<U>().unwrap();
        Some(cast)
    }
}

impl<T> UninitView<'_, T> {
    /// Invent a new uninit allocation for a zero-sized type (ZST).
    ///
    /// # Panics
    /// This method panics when the type parameter is not a zero sized type.
    pub fn invent_for_zst() -> Self {
        assert_eq!(
            mem::size_of::<T>(),
            0,
            "Invented ZST uninit invoked with non-ZST"
        );
        let dangling = ptr::NonNull::<T>::dangling();
        // SAFETY: all bytes are within the allocation.
        let raw = unsafe { UninitView::from_memory(dangling.cast(), 0) };
        raw.cast().unwrap()
    }
}

impl<'a, T> UninitView<'a, T> {
    /// Split the uninit view at a byte boundary.
    ///
    /// See [`Uninit::split_at_byte`] for more details.
    ///
    /// [`Uninit::split_at_byte`]: ./struct.Uninit.html#method.split_at_byte
    pub fn split_at_byte(&mut self, at: usize) -> Option<UninitView<'a, ()>> {
        if self.len < at || at < mem::size_of::<T>() {
            return None;
        }

        let base = self.ptr.as_ptr();
        // SAFETY: by `from_memory`, all offsets `< len` are within the allocation.
        // In particular, no pointer within or one-past-the-end is null.
        let next_base = unsafe { ptr::NonNull::new_unchecked(base.add(at)) };
        let next_len = self.len - at;
        self.len = at;

        // SAFETY: within one allocation, namely the one we are in.
        let other = unsafe { UninitView::from_memory(next_base.cast(), next_len) };
        Some(other)
    }

    /// Create an view to the inner bytes of a `MaybeUninit`.
    ///
    /// This is hardly useful on its own but since `UninitView` mirrors the traversal methods of
    /// `Uninit` it can be used to get pointers to already initialized elements in an immutable
    /// context.
    pub fn from_maybe_uninit(mem: &'a mem::MaybeUninit<T>) -> Self {
        let ptr = ptr::NonNull::new(mem.as_ptr() as *mut T).unwrap();
        let raw = unsafe {
            // SAFETY:
            // * unaliased as we had a mutable reference
            // * we will not write through the pointer created
            UninitView::from_memory(ptr.cast(), mem::size_of_val(mem))
        };
        raw.cast().unwrap()
    }

    /// Try to cast to an `UninitView` for a slice type.
    pub fn cast_slice<U>(self) -> Result<UninitView<'a, [U]>, Self> {
        let empty = Layout::for_value::<[U]>(&[]);

        if !self.fits(empty) {
            return Err(self);
        }

        Ok(UninitView {
            ptr: {
                let base = self.ptr.as_ptr() as *mut T;
                let element = Layout::new::<U>();
                let slice_len = if element.size() == 0 {
                    usize::MAX
                } else {
                    self.len / element.size()
                };
                let slice = ptr::slice_from_raw_parts_mut(base as *mut U, slice_len);
                ptr::NonNull::new(slice).unwrap()
            },
            len: self.len,
            lifetime: PhantomData,
            typed: PhantomData,
        })
    }

    /// Split off the tail that is not required for holding an instance of `T`.
    pub fn split_to_fit(&mut self) -> UninitView<'a, ()> {
        self.split_at_byte(mem::size_of::<T>()).unwrap()
    }

    /// Turn this into a reference to standard `MaybeUninit`.
    ///
    /// This is mainly useful for interfacing with other consumers which expect standard library
    /// types and to mirror `Uninit`.
    ///
    /// Note that the sequence `from_maybe_uninit`, `into_maybe_uninit` is a no-op. The converse is
    /// however not the case, as it will potentially discard unused padding present in the original
    /// `Uninit`.
    pub fn into_maybe_uninit(self) -> &'a mem::MaybeUninit<T> {
        // SAFETY: MaybeUninit is a transparent wrapper and need not be initialized.
        unsafe { &*(self.as_ptr() as *const mem::MaybeUninit<T>) }
    }
}

impl<'a, T: ?Sized> UninitView<'a, T> {
    /// Create a reference to typed uninitialized memory.
    ///
    /// It is given a capacity of memory to which it refers in *bytes*.
    ///
    /// ## Safety
    ///
    /// The `ptr` must describe a valid, *sized* region. Refer to `Layout::for_value_raw` for
    /// details. This criteria is trivially fulfilled for any sized `T`.
    ///
    /// A valid allocation must exist at the pointer with length at least `len`.
    ///
    /// In particular, it is **UB** to create this from a reference to a variable of a type for
    /// which a completely uninitialized content is not valid. The standard type for avoiding the
    /// UB is `core::mem::MaybeUninit`.
    ///
    /// When in doubt, refactor code such that utilization of `from_maybe_uninit` is possible.
    pub unsafe fn new(ptr: ptr::NonNull<T>, len: usize) -> Self {
        UninitView {
            ptr,
            len,
            lifetime: PhantomData,
            typed: PhantomData,
        }
    }

    /// Return the number of bytes this may refer to.
    pub fn byte_capacity(&self) -> usize {
        self.len
    }

    /// Try to cast to an `UninitView` for another type.
    pub fn cast<U>(self) -> Result<UninitView<'a, U>, Self> {
        if !self.fits(Layout::new::<U>()) {
            return Err(self);
        }

        Ok(UninitView {
            ptr: self.ptr.cast(),
            len: self.len,
            lifetime: PhantomData,
            typed: PhantomData,
        })
    }

    /// Acquires the underlying `*const T` pointer.
    pub const fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr() as *const T
    }

    /// Acquires the underlying pointer as a `NonNull`.
    pub fn as_non_null(&self) -> ptr::NonNull<T> {
        self.ptr
    }

    /// Dereferences the content.
    ///
    /// The resulting lifetime is bound to self so this behaves "as if" it were actually an
    /// instance of T that is getting borrowed. If a longer lifetime is needed, use `into_ref`.
    ///
    /// ## Safety
    /// The caller must ensure that the content has already been initialized.
    pub unsafe fn as_ref(&self) -> &T {
        self.into_ref()
    }

    /// Turn this into a reference to the content.
    ///
    /// ## Safety
    /// The caller must ensure that the content has already been initialized.
    pub unsafe fn into_ref(self) -> &'a T {
        &*self.as_ptr()
    }
}

impl<'a, T> UninitView<'a, [T]> {
    /// Creates a pointer to an empty slice.
    ///
    /// Note that it will **not** be a mutable empty slice which means that it would be **UB** to
    /// use it as an `Uninit`.
    pub fn empty() -> Self {
        UninitView {
            ptr: {
                let base = ptr::NonNull::<T>::dangling().as_ptr();
                let slice = ptr::slice_from_raw_parts_mut(base, 0);
                ptr::NonNull::new(slice).unwrap()
            },
            len: 0,
            lifetime: PhantomData,
            typed: PhantomData,
        }
    }

    /// Create an view on potentially uninitialized memory bytes of a slice of `MaybeUninit`.
    pub fn from_maybe_uninit_slice(mem: &'a [mem::MaybeUninit<T>]) -> Self {
        let ptr = ptr::NonNull::from(mem);
        let raw = unsafe {
            // SAFETY:
            // * can write uninitialized bytes as much as we want
            UninitView::from_memory(ptr.cast(), mem::size_of_val(mem))
        };
        raw.cast_slice().unwrap()
    }

    /// Get the pointer to the first element of the slice.
    pub fn as_begin_ptr(&self) -> *const T {
        self.ptr.as_ptr() as *const T
    }

    /// Calculate the theoretical capacity of a slice in the pointed-to allocation.
    pub fn capacity(&self) -> usize {
        self.size()
            .checked_div(mem::size_of::<T>())
            .unwrap_or_else(usize::max_value)
    }

    /// Split the slice at an index.
    pub fn split_at(&mut self, at: usize) -> Option<Self> {
        // NOTE: Slice pointers are blocked by Rust stabilization we can not create one from a real
        // reference to slice as that would restrict us to the memory covered by the reference.
        // NOTE: Tracked here https://github.com/rust-lang/rust/issues/36925
        let bytes = match at.checked_mul(mem::size_of::<T>()) {
            None => return None,
            Some(byte) if byte > self.len => return None,
            Some(byte) => byte,
        };

        let next_len = self.len - bytes;
        self.len = bytes;

        let base = self.ptr.as_ptr() as *mut u8;
        // SAFETY: was previously in bounds.
        let next_base = unsafe { ptr::NonNull::new_unchecked(base.add(bytes)) };

        // SAFETY: total allocation length at least `self.len + next_len`.
        let other = unsafe { UninitView::from_memory(next_base, next_len) };
        Some(other.cast_slice().unwrap())
    }

    /// Get the trailing bytes behind the slice.
    ///
    /// The underlying allocation need not be a multiple of the slice element size which may leave
    /// unusable bytes. This splits these unusable bytes into an untyped `Uninit` which can be
    /// reused arbitrarily.
    ///
    /// This operation is idempotent.
    pub fn shrink_to_fit(&mut self) -> UninitView<'a, ()> {
        UninitView::decast(self.split_at(self.capacity()).unwrap())
    }

    /// Split the first element from the slice.
    pub fn split_first(&mut self) -> Option<UninitView<'a, T>> {
        let mut part = self.split_at(1)?;
        // Now we are the first part, but we wanted the first to be split off.
        mem::swap(self, &mut part);
        // If it is a valid slice of length 1 it is a valid `T`.
        Some(UninitView::decast(part).cast().unwrap())
    }

    /// Split the last element from the slice.
    pub fn split_last(&mut self) -> Option<UninitView<'a, T>> {
        // Explicitely wrap here: If capacity is 0 then `0 < size_of::<T> ` and the split will fail.
        let split = self.capacity().wrapping_sub(1);
        let part = self.split_at(split)?;
        // If it is a valid slice of length 1 it is a valid `T`.
        Some(UninitView::decast(part).cast().unwrap())
    }

    /// Turn this into a slice of standard `MaybeUninit`s.
    ///
    /// This is mainly useful for interfacing with other consumers which expect standard library
    /// types and to mirror `Uninit`.
    ///
    /// Note that the sequence `from_maybe_uninit_slice`, `into_maybe_uninit_slice` is a no-op. The
    /// converse is however not the case, as it will potentially discard unused padding present in
    /// the original `Uninit`.
    pub fn into_maybe_uninit_slice(self) -> &'a [mem::MaybeUninit<T>] {
        unsafe {
            // SAFETY: MaybeUninit is a transparent wrapper and need not be initialized.
            slice::from_raw_parts(
                self.as_begin_ptr() as *const mem::MaybeUninit<T>,
                self.capacity(),
            )
        }
    }
}

impl<'a, T: ?Sized> UninitView<'a, T> {
    /// Check if the view fits some layout.
    ///
    /// The `cast` to a type of the provided layout will work without error.
    pub fn fits(&self, layout: Layout) -> bool {
        self.ptr.as_ptr().cast::<u8>().align_offset(layout.align()) == 0
            && layout.size() <= self.len
    }

    /// Borrow another view of the `Uninit` region.
    pub fn borrow(&self) -> UninitView<'_, T> {
        *self
    }

    /// Get the byte size of the total allocation.
    pub const fn size(&self) -> usize {
        self.len
    }
}

impl<'a, T> From<&'a mut mem::MaybeUninit<T>> for Uninit<'a, T> {
    fn from(mem: &'a mut mem::MaybeUninit<T>) -> Self {
        Uninit::<T>::from_maybe_uninit(mem)
    }
}

impl<'a, T> From<&'a mut [mem::MaybeUninit<T>]> for Uninit<'a, [T]> {
    fn from(mem: &'a mut [mem::MaybeUninit<T>]) -> Self {
        Uninit::<[T]>::from_maybe_uninit_slice(mem)
    }
}

impl<'a, T> From<&'a mem::MaybeUninit<T>> for UninitView<'a, T> {
    fn from(mem: &'a mem::MaybeUninit<T>) -> Self {
        UninitView::from_maybe_uninit(mem)
    }
}

impl<'a, T> From<&'a [mem::MaybeUninit<T>]> for UninitView<'a, [T]> {
    fn from(mem: &'a [mem::MaybeUninit<T>]) -> Self {
        UninitView::<[T]>::from_maybe_uninit_slice(mem)
    }
}

impl<T: ?Sized> fmt::Debug for Uninit<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Uninit")
            .field(&self.view.ptr)
            .field(&self.view.len)
            .finish()
    }
}

impl<T: ?Sized> fmt::Debug for UninitView<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("UninitView")
            .field(&self.ptr)
            .field(&self.len)
            .finish()
    }
}

impl<'a, T> From<Uninit<'a, T>> for UninitView<'a, T> {
    fn from(uninit: Uninit<'a, T>) -> Self {
        uninit.view
    }
}

impl<T> Default for Uninit<'_, [T]> {
    fn default() -> Self {
        Uninit::empty()
    }
}

impl<T> Default for UninitView<'_, [T]> {
    fn default() -> Self {
        UninitView::empty()
    }
}

impl<T: ?Sized> Clone for UninitView<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for UninitView<'_, T> {}

unsafe impl<'a, T, U: ?Sized> CoerciblePtr<U> for UninitView<'a, T> {
    type Pointee = T;
    type Output = UninitView<'a, U>;
    fn as_sized_ptr(&mut self) -> *mut T {
        self.as_ptr() as *mut T
    }
    unsafe fn replace_ptr(self, new: *mut U) -> UninitView<'a, U> {
        let length = self.byte_capacity();
        debug_assert_eq!(self.as_ptr() as *const u8, new as *const u8);
        // SAFETY: caller guarantees this is equal to our pointer, which is non-null
        let new = ptr::NonNull::new_unchecked(new);
        UninitView::new(new, length)
    }
}

unsafe impl<'a, T, U: ?Sized> CoerciblePtr<U> for Uninit<'a, T> {
    type Pointee = T;
    type Output = Uninit<'a, U>;

    fn as_sized_ptr(&mut self) -> *mut T {
        self.as_ptr()
    }

    unsafe fn replace_ptr(self, new: *mut U) -> Uninit<'a, U> {
        let length = self.byte_capacity();
        debug_assert_eq!(self.as_ptr() as *const u8, new as *const u8);
        // SAFETY: caller guarantees this is equal to our pointer, which is non-null
        let new = ptr::NonNull::new_unchecked(new);
        Uninit::new(new, length)
    }
}

#[cfg(test)]
mod tests {
    use super::Uninit;

    #[test]
    fn lifetime_longer() {
        fn _long<'a, T>(_: Uninit<'a, &'static T>) {}
    }

    #[test]
    fn lifetime_shorter() {
        fn _short<'a, T>(_: Uninit<'static, &'a T>) {}
    }

    #[test]
    fn in_a_struct() {
        enum _List<T> {
            Nil,
            Cons(T, Uninit<'static, T>),
        }
    }
}
