//! Contains the `FixedVec` implementation.
//!
//! [See `FixedVec` for the main information][`FixedVec`].
//!
//! [`FixedVec`]: struct.FixedVec.html
use core::{borrow, cmp, hash, iter, ops, ptr, slice};
use crate::uninit::Uninit;

/// A `Vec`-like structure that does not manage its allocation.
///
/// This vector type will never (re-)allocate but it can also not free underused memory. As opposed
/// to other similar crates, it does require and additional bounds on its type parameter as it
/// truly manages uninitialized memory to store instances.
///
/// # Basic Usage
///
/// It is easy to use a local array or slice of `MaybeUninit` for the storage of a `FixedVec`. Note
/// that, similar to the allocated standard `Vec`, the underlying memory not being stored inline
/// makes moves and splitting much cheaper.
///
/// ```
/// use core::mem::MaybeUninit;
/// use without_alloc::FixedVec;
///
/// let mut memory: [MaybeUninit<usize>; 15] = [MaybeUninit::uninit(); 15];
/// let mut stack = FixedVec::new((&mut memory[..]).into());
///
/// stack.push(1);
/// stack.push(2);
/// stack.push(3);
/// while let Some(top) = stack.pop() {
///     // Prints 3, 2, 1
///     println!("{}", top);
/// }
/// ```
///
/// ## With `Bump`
///
/// One focus of the library is composability. It should not be surprising that `FixedVec`
/// interacts with the [`LocalAlloc`] allocators, which implements a specialized interface
/// providing the [`Uninit`] type instead of a raw `*const u8`, through an extension trait. Hence,
/// the `FixedVec` can use this instead of its own local stack variables.
///
/// ```
/// use static_alloc::Bump;
/// use without_alloc::{FixedVec, alloc::LocalAllocLeakExt};
///
/// let alloc: Bump<[u8; 1 << 12]> = Bump::uninit();
/// let some_usize = alloc.leak(0_usize).unwrap();
///
/// // Allocate a vector with capacity `1` from the slab.
/// let mut vec = alloc.fixed_vec(1).unwrap();
///
/// // Push the reference to the other allocation.
/// vec.push(&mut *some_usize);
///
/// // … do something else
///
/// // Ensure lifetimes work out.
/// drop(vec);
///
/// // Hooray, now once again unborrowed.
/// *some_usize = 0;
/// ```
///
/// ## The [`from_unaligned`] constructor
///
/// It is possible to place a `FixedVec` into an uninitialized memory, not only the `Uninit<[T]>`
/// that the [`new`] constructor requires. This will align the underlying memory suitably and
/// substitute a dangling empty slice if that is not possible.
///
/// ```
/// use core::mem::MaybeUninit;
/// use without_alloc::{FixedVec, Uninit};
///
/// struct MyStruct {
///     // ..
/// # _private: [usize; 1],
/// }
///
/// let mut memory: MaybeUninit<[u8; 1024]> = MaybeUninit::uninit();
/// let uninit = Uninit::from(&mut memory);
///
/// // NO guarantees about space lost from required additional aligning.
/// let mut vec: FixedVec<MyStruct> = FixedVec::from_unaligned(uninit);
/// ```
///
/// [`Bump`]: ../slab/struct.Bump.html
/// [`Uninit`]: ../uninit/struct.Uninit.html
/// [`new`]: #method.new
/// [`from_unaligned`]: #method.from_unaligned
pub struct FixedVec<'a, T> {
    uninit: Uninit<'a, [T]>,
    length: usize,
}

/// An iterator removing a range of elements from a `FixedVec`.
///
/// See [`FixedVec::drain`] for more information.
///
/// [`FixedVec::drain`]: struct.FixedVec.html#method.drain
// Internal invariant: `0 <= start <= end <= tail`
pub struct Drain<'a, T> {
    /// Number of elements drained from the start of the slice.
    start: usize,
    /// The end of the elements to drain (relative to `elements`), inbounds offset for `elements`.
    end: usize,
    /// The start of the tail of elements (relative to `elements`), inbounds offset for `elements`.
    tail: usize,
    /// The length of the tail.
    tail_len: usize,
    /// Pointer to first element to drain (and to write to on `Drop`).
    elements: ptr::NonNull<T>,
    /// The length field of the underlying `FixedVec`.
    len: &'a mut usize,
}

impl<T> FixedVec<'_, T> {
    /// Shorten the vector to a maximum length.
    ///
    /// If the length is not larger than `len` this has no effect.
    ///
    /// The tail of the vector is dropped starting from the last element. This order is opposite to
    /// `.drain(len..).for_each(drop)`. `truncate` provides the extra guarantee that a `panic`
    /// during `Drop` of one element effectively stops the truncation at that point, instead of
    /// leaking unspecified other content of the vector. This means that other elements are still
    /// dropped when unwinding eventually drops the `FixedVec` itself.
    ///
    /// ## Example
    ///
    /// ```
    /// # use core::mem::MaybeUninit;
    /// # use without_alloc::{FixedVec, Uninit};
    ///
    /// let mut memory: [MaybeUninit<usize>; 4] = [MaybeUninit::uninit(); 4];
    /// let mut vec = FixedVec::new(Uninit::from(&mut memory[..]));
    ///
    /// vec.push(0usize);
    /// vec.push(1usize);
    /// vec.push(2usize);
    ///
    /// assert_eq!(vec.as_slice(), [0, 1, 2]);
    /// vec.truncate(1);
    /// assert_eq!(vec.as_slice(), [0]);
    /// ```
    pub fn truncate(&mut self, len: usize) {
        struct SetLenOnDrop<'a> {
            len: &'a mut usize,
            local_len: usize,
        }

        impl Drop for SetLenOnDrop<'_> {
            fn drop(&mut self) {
                *self.len = self.local_len;
            }
        }

        let mut ptr = self.end_mut_ptr();
        let current_length = self.length;
        let mut set_len = SetLenOnDrop { len: &mut self.length, local_len: current_length };

        for _ in len..current_length {
            set_len.local_len -= 1;

            unsafe {
                ptr = ptr.offset(-1);
                ptr::drop_in_place(ptr);
            }
        }
    }

    /// Extracts a slice containing the entire vector.
    pub fn as_slice(&self) -> &[T] {
        unsafe {
            // SAFETY: length is the number of initialized elements.
            slice::from_raw_parts(self.uninit.as_begin_ptr(), self.length)
        }
    }

    /// Extracts the mutable slice containing the entire vector.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe {
            // SAFETY:
            // * length is the number of initialized elements.
            // * unaliased since we take ourselves by `mut` and `uninit` does the rest.
            slice::from_raw_parts_mut(self.uninit.as_begin_ptr(), self.length)
        }
    }

    /// Remove all elements.
    ///
    /// This is an alias for [`truncate(0)`][truncate].
    ///
    /// [truncate]: #method.truncate
    pub fn clear(&mut self) {
        self.truncate(0)
    }

    /// Returns the number of elements in the vector.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Set the raw length.
    ///
    /// ## Safety
    /// * `new_len` must be smaller or equal `self.capacity()`
    /// * The caller must ensure that all newly referenced elements are properly initialized.
    pub unsafe fn set_len(&mut self, new_len: usize) {
        self.length = new_len;
    }

    /// Returns the number of elements the vector can hold.
    pub fn capacity(&self) -> usize {
        self.uninit.capacity()
    }

    /// Returns `true` if the vector contains no elements.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Appends an element to the back of a collection.
    ///
    /// Return `Err(val)` if it is not possible to append the element.
    ///
    /// ```
    /// use core::mem::MaybeUninit;
    /// use without_alloc::{FixedVec, Uninit};
    ///
    /// // Only enough storage for one element.
    /// let mut allocation: [MaybeUninit<u32>; 1] = [MaybeUninit::uninit()];
    /// let mut vec = FixedVec::new(Uninit::from(&mut allocation[..]));
    ///
    /// // First push succeeds.
    /// assert_eq!(vec.push(1), Ok(()));
    ///
    /// // The second push fails.
    /// assert_eq!(vec.push(2), Err(2));
    ///
    /// ```
    pub fn push(&mut self, val: T) -> Result<(), T> {
        if self.length == usize::max_value() {
            return Err(val);
        }

        let init = match self.head_tail_mut().1.split_first() {
            Some(init) => init,
            None => return Err(val),
        };

        init.init(val);
        self.length += 1;

        Ok(())
    }

    /// Removes the last element from a vector and returns it, or `None` if it is empty.
    pub fn pop(&mut self) -> Option<T> {
        if self.length == 0 {
            return None;
        }

        let last = self.head_tail_mut().0.split_last().unwrap();
        let val = unsafe {
            // SAFETY: initialized and no reference of any kind exists to it.
            ptr::read(last.as_ptr())
        };

        self.length -= 1;
        Some(val)
    }

    /// Split the capacity into a *borrowed* other vector.
    ///
    /// The other vector borrows the underlying memory resource while it is alive.
    ///
    /// This is a specialized method not found in the standard `Vec` as it relies on `FixedVec` not
    /// owning the allocation itself. This avoids splitting the underlying allocation which would
    /// require `unsafe` to mend the parts together.
    ///
    /// ## Panics
    /// This method panics if `at > self.capacity()`.
    ///
    /// ## Examples
    ///
    /// ```
    /// use without_alloc::{FixedVec, alloc::LocalAllocLeakExt};
    /// use static_alloc::Bump;
    ///
    /// let mut memory: Bump<[usize; 8]> = Bump::uninit();
    /// let mut vec = memory.fixed_vec::<usize>(8).unwrap();
    /// vec.fill(0..7);
    ///
    /// // Can use like a vector:
    /// let mut part = vec.split_borrowed(4);
    /// assert!(part.push(7).is_ok());
    /// assert!((4..8).eq(part.drain(..)));
    ///
    /// // Drop to rescind the borrow on `vec`.
    /// drop(part);
    ///
    /// // All split elements are gone
    /// assert_eq!(vec.len(), 4);
    /// // But retained all capacity
    /// assert_eq!(vec.capacity(), 8);
    /// ```
    #[must_use = "Elements in the split tail will be dropped. Prefer `truncate(at)` or `drain(at..)` if there is no other use."]
    pub fn split_borrowed(&mut self, at: usize) -> FixedVec<'_, T> {
        assert!(at <= self.capacity(), "`at` out of bounds");
        let new_uninit = self.uninit.borrow_mut().split_at(at).unwrap();
        let new_len = self.length.saturating_sub(at);
        self.length -= new_len;
        FixedVec {
            uninit: new_uninit,
            length: new_len,
        }
    }

    /// Split the capacity of the collection into two at a given index.
    ///
    /// In contrast to `Vec::split_off` calling this method reduces the capacity of `self` to `at`.
    ///
    /// ## Panics
    /// This method panics if `at > self.capacity()`.
    pub fn split_and_shrink_to(&mut self, at: usize) -> Self {
        assert!(at <= self.capacity(), "`at` out of bounds");
        // `self.length` is always smaller than `split_at`.
        let new_uninit = self.uninit.split_at(at).unwrap();
        // The first `at` elements stay in this vec.
        let new_len = self.length.saturating_sub(at);
        self.length -= new_len;
        FixedVec {
            uninit: new_uninit,
            length: new_len,
        }
    }

    /// Extend the vector with as many elements as fit.
    ///
    /// Returns the iterator with all elements that were not pushed into the vector.
    pub fn fill<I: IntoIterator<Item = T>>(&mut self, iter: I)
        -> I::IntoIter
    {
        let unused = self.capacity() - self.len();
        let mut iter = iter.into_iter();
        for item in iter.by_ref().take(unused) {
            unsafe {
                // SAFETY:
                //  `capacity != len` so this is strictly in bounds. Also, this is behind the
                //  vector so there can not be any references to it currently.
                ptr::write(self.end_mut_ptr(), item);
                // SAFETY:
                //  Just initialized one more element past the end. By the way, this can not
                //  overflow since the result is at most `self.capacity()`.
                self.set_len(self.len() + 1);
            }
        }
        iter
    }

    /// Creates a draining iterator that yields and removes elements a given range.
    ///
    /// It is unspecified which elements are removed if the `Drain` is never dropped. If you
    /// require precise semantics even in this case you might be able to swap the range to the back
    /// of the vector and invoke [`split_and_shrink_to`].
    ///
    /// [`split_and_shrink_to`]: #method.split_and_shrink_to
    pub fn drain<R>(&mut self, range: R) -> Drain<'_, T>
        where R: ops::RangeBounds<usize>
    {
        let len = self.len();
        let start = match range.start_bound() {
            ops::Bound::Included(&n) => n,
            ops::Bound::Excluded(&n) => n + 1,
            ops::Bound::Unbounded    => 0,
        };
        let end = match range.end_bound() {
            ops::Bound::Included(&n) => n + 1,
            ops::Bound::Excluded(&n) => n,
            ops::Bound::Unbounded    => len,
        };
        assert!(start <= end);
        assert!(end <= len);

        let elements = unsafe {
            // SAFETY:
            //  Within allocation since `start <= len` and len is at most the
            //  one-past-the-end pointer. Pointer within are also never null.
            //
            //  In particular we can shorten the length. We initially shorten
            //  the length until all elements are drained. The Drain will
            //  increase the length by `len - end` elements which will still be
            //  within the bounds of the allocation as `start <= end`.
            self.set_len(start);
            ptr::NonNull::new_unchecked(self.as_mut_ptr().add(start))
        };

        Drain {
            // Internal invariant: `count <= tail`.
            start: 0,
            // Relative to `elements`. inbounds of original `as_mut_ptr()`.
            end: end - start,
            tail: end - start,
            tail_len: len - end,
            elements,
            len: &mut self.length,
        }
    }

    fn head_tail_mut(&mut self) -> (Uninit<'_, [T]>, Uninit<'_, [T]>) {
        // Borrow, do not affect the actual allocation by throwing away possible elements.
        let mut all = self.uninit.borrow_mut();
        // This must always be possible. `self.length` is nevery greater than the capacity.
        let tail = all.split_at(self.length).unwrap();
        (all, tail)
    }

    /// Get a read-only pointer, valid for the initialized and uninitialized portion of the raw
    /// vector.
    pub fn as_ptr(&mut self) -> *const T {
        self.uninit.as_begin_ptr()
    }

    /// Get a mutable pointer, valid for the initialized and uninitialized portion of the raw
    /// vector.
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.uninit.as_begin_ptr()
    }

    fn end_mut_ptr(&mut self) -> *mut T {
        unsafe { self.as_mut_ptr().add(self.length) }
    }
}

impl<'a, T> FixedVec<'a, T> {
    /// Create a `FixedVec` in a pre-allocated region.
    ///
    /// The capacity will be that of the underlying allocation.
    pub fn new(uninit: Uninit<'a, [T]>) -> Self {
        FixedVec {
            uninit,
            length: 0,
        }
    }

    /// Create a `FixedVec` with as large of a capacity as available.
    ///
    /// When no aligned slice can be create within the provided memory then the constructor will
    /// fallback to an empty dangling slice.
    ///
    /// This is only a utility function which may be lossy as data before the alignment is
    /// discarded. Prefer an explicit [`Uninit::cast_slice`] followed by error handling if this is
    /// undesirable.
    ///
    /// [`Uninit::cast_slice`]: ../uninit/struct.Uninit.html#method.cast_slice
    pub fn from_unaligned<U: ?Sized>(generic: Uninit<'a, U>) -> Self {
        let mut uninit = generic.as_memory();
        let slice = uninit.split_slice().unwrap_or_else(Uninit::empty);
        Self::new(slice)
    }

    /// Return trailing bytes that can not be used by the `FixedVec`.
    ///
    /// This operation is idempotent.
    pub fn shrink_to_fit(&mut self) -> Uninit<'a, ()> {
        self.uninit.shrink_to_fit()
    }
}

impl<T> Drain<'_, T> {
    /// View the remaining data as a slice.
    ///
    /// Similar to `slice::Iter::as_slice` but you are not allowed to use the iterator as it will
    /// invalidate the pointees. This is an extended form of `Peekable::peek`.
    pub fn as_slice(&self) -> &[T] {
        unsafe {
            // SAFETY: all indices up to `tail` are inbounds. Internal invariant guarantees `start`
            // is smaller.
            slice::from_raw_parts(
                self.elements.as_ptr().add(self.start),
                self.len())
        }
    }

    /// View the remaining data as a mutable slice.
    ///
    /// This is `Peekable::peek` on steroids.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe {
            // SAFETY: all indices up to `tail` are inbounds. Internal invariant guarantees `start`
            // is smaller. Not aliased as it mutably borrows the `Drain`.
            slice::from_raw_parts_mut(
                self.elements.as_ptr().add(self.start),
                self.len())
        }
    }

    /// The count of remaining elements to drain.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// If there are any elements remaining.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

impl<T> ops::Deref for FixedVec<'_, T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> ops::DerefMut for FixedVec<'_, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T> Drop for FixedVec<'_, T> {
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(self.as_mut_slice())
        }
    }
}

impl<T, I> ops::Index<I> for FixedVec<'_, T>
    where I: slice::SliceIndex<[T]>,
{
    type Output = I::Output;

    fn index(&self, idx: I) -> &I::Output {
        ops::Index::index(&**self, idx)
    }
}

impl<T, I> ops::IndexMut<I> for FixedVec<'_, T>
    where I: slice::SliceIndex<[T]>,
{
    fn index_mut(&mut self, idx: I) -> &mut I::Output {
        ops::IndexMut::index_mut(&mut**self, idx)
    }
}

impl<'a, 'b, T: PartialEq> PartialEq<FixedVec<'b, T>> for FixedVec<'a, T> {
    #[inline]
    fn eq(&self, other: &FixedVec<T>) -> bool {
        PartialEq::eq(&**self, &**other)
    }
    #[inline]
    fn ne(&self, other: &FixedVec<T>) -> bool {
        PartialEq::ne(&**self, &**other)
    }
}

impl<'a, 'b, T: PartialOrd> PartialOrd<FixedVec<'b, T>> for FixedVec<'a, T> {
    #[inline]
    fn partial_cmp(&self, other: &FixedVec<T>) -> Option<cmp::Ordering> {
        PartialOrd::partial_cmp(&**self, &**other)
    }
    #[inline]
    fn lt(&self, other: &FixedVec<T>) -> bool {
        PartialOrd::lt(&**self, &**other)
    }
    #[inline]
    fn le(&self, other: &FixedVec<T>) -> bool {
        PartialOrd::le(&**self, &**other)
    }
    #[inline]
    fn ge(&self, other: &FixedVec<T>) -> bool {
        PartialOrd::ge(&**self, &**other)
    }
    #[inline]
    fn gt(&self, other: &FixedVec<T>) -> bool {
        PartialOrd::gt(&**self, &**other)
    }
}

impl<T: Ord> Ord for FixedVec<'_, T> {
    #[inline]
    fn cmp(&self, other: &FixedVec<T>) -> cmp::Ordering {
        Ord::cmp(&**self, &**other)
    }
}

impl<T: Eq> Eq for FixedVec<'_, T> { }

impl<T: hash::Hash> hash::Hash for FixedVec<'_, T> {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        hash::Hash::hash(&**self, state)
    }
}

impl<T> borrow::Borrow<[T]> for FixedVec<'_, T> {
    fn borrow(&self) -> &[T] {
        &**self
    }
}

impl<T> borrow::BorrowMut<[T]> for FixedVec<'_, T> {
    fn borrow_mut(&mut self) -> &mut [T] {
        &mut **self
    }
}

impl<T> AsRef<[T]> for FixedVec<'_, T> {
    fn as_ref(&self) -> &[T] {
        &**self
    }
}

impl<T> AsMut<[T]> for FixedVec<'_, T> {
    fn as_mut(&mut self) -> &mut [T] {
        &mut **self
    }
}

impl<T> Iterator for Drain<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if Drain::is_empty(self) {
            return None;
        }

        let t = unsafe {
            // SAFETY: `count <= self.tail` and `tail` is always in bounds.
            ptr::read(self.elements.as_ptr().add(self.start))
        };

        self.start += 1;
        Some(t)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.start..self.end).size_hint()
    }
}

impl<T> DoubleEndedIterator for Drain<'_, T> {
    fn next_back(&mut self) -> Option<T> {
        if Drain::is_empty(self) {
            return None;
        }

        let t = unsafe {
            // SAFETY: `end <= self.tail` and `tail` is always in bounds.
            ptr::read(self.elements.as_ptr().add(self.end - 1))
        };

        self.end -= 1;
        Some(t)
    }
}

impl<T> ExactSizeIterator for Drain<'_, T> {
    fn len(&self) -> usize {
        Drain::len(self)
    }
}

impl<T> iter::FusedIterator for Drain<'_, T> { }

impl<T> Drop for Drain<'_, T> {
    fn drop(&mut self) {
        self.for_each(drop);

        if self.tail_len != 0 {
            unsafe {
                let source = self.elements.as_ptr().add(self.tail);
                ptr::copy(source, self.elements.as_ptr(), self.tail_len);
            }
            // Restore the tail to the vector.
            *self.len += self.tail_len;
        }
    }
}

/// Extend the vector to the extent the allocation allows it.
///
/// Appends elements from the iterator and panics if the iterator contains more elements than the
/// capacity of the vector. See [`fill`] for a specialized method that does not further exhaust the
/// iterator and does not panic.
///
/// ## Examples
///
/// To avoid panicking you can limit the iterator to the remaining capacity. Depending on the
/// underlying iterator it will exhaust itself further when the iterator itself is dropped.
///
/// ```
/// # use core::mem::MaybeUninit;
/// # use without_alloc::FixedVec;
/// let mut memory: [MaybeUninit<usize>; 15] = [MaybeUninit::uninit(); 15];
/// let mut source_vec = FixedVec::new((&mut memory[..]).into());
/// source_vec.extend(0..15);
///
/// let mut memory: [MaybeUninit<usize>; 3] = [MaybeUninit::uninit(); 3];
/// let mut target = FixedVec::new((&mut memory[..]).into());
/// target.extend(source_vec.drain(..).take(3));
///
/// assert!(source_vec.is_empty());
/// assert_eq!(target.len(), target.capacity());
/// ```
///
/// If the iterator is not limited to the number of elements then this method will panic.
///
/// ```should_panic
/// # use core::mem::MaybeUninit;
/// # use without_alloc::FixedVec;
/// let mut memory: [MaybeUninit<usize>; 3] = [MaybeUninit::uninit(); 3];
/// let mut vec = FixedVec::new((&mut memory[..]).into());
/// vec.extend(0..);
/// ```
///
/// [`fill`]: #method.fill
impl<T> iter::Extend<T> for FixedVec<'_, T> {
    fn extend<I>(&mut self, iter: I)
        where I: IntoIterator<Item=T>,
    {
        for _spill in self.fill(iter) {
            panic!("FixedVec memory exhausted");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FixedVec;
    use crate::Uninit;

    use core::cell::Cell;
    use core::mem::MaybeUninit;
    use core::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct Trigger<'a> {
        panic_on_drop: bool,
        dropped_counter: &'a Cell<usize>,
    }

    impl Drop for Trigger<'_> {
        fn drop(&mut self) {
            if self.panic_on_drop { panic!("Trigger triggered") }
            // Record this as a normal drop.
            self.dropped_counter.set(self.dropped_counter.get() + 1);
        }
    }

    struct AbortMismatchedDropCount<'a> {
        counter: &'a Cell<usize>,
        expected: usize,
    }

    impl Drop for AbortMismatchedDropCount<'_> {
        fn drop(&mut self) {
            struct ForceDupPanic;

            impl Drop for ForceDupPanic {
                fn drop(&mut self) { panic!() }
            }

            if self.expected != self.counter.get() {
                // For duplicate panic, and thus abort
                let _x = ForceDupPanic;
                panic!();
            }
        }
    }

    #[test]
    fn init_and_use() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        struct Foo(usize);

        const CAPACITY: usize = 30;

        let mut allocation: [MaybeUninit<Foo>; 30] = [MaybeUninit::uninit(); 30];
        let mut vec = FixedVec::new((&mut allocation[..]).into());

        assert_eq!(vec.capacity(), CAPACITY);
        assert_eq!(vec.len(), 0);
        for i in 0..CAPACITY {
            assert_eq!(vec.push(Foo(i)), Ok(()));
        }

        assert_eq!(vec.capacity(), CAPACITY);
        assert_eq!(vec.len(), CAPACITY);

        for i in (0..CAPACITY).rev() {
            assert_eq!(vec.pop(), Some(Foo(i)));
        }

        assert_eq!(vec.capacity(), CAPACITY);
        assert_eq!(vec.len(), 0);
    }

    #[test]
    fn zst_drop() {
        const COUNT: usize = 30;
        static DROP_COUNTER: AtomicUsize = AtomicUsize::new(0);
        struct HasDrop(usize);

        impl Drop for HasDrop {
            fn drop(&mut self) {
                DROP_COUNTER.fetch_add(1, Ordering::SeqCst);
            }
        }


        let mut allocation: MaybeUninit<[HasDrop; COUNT]> = MaybeUninit::uninit();
        let uninit = Uninit::from_maybe_uninit(&mut allocation);
        let mut vec = FixedVec::new(uninit.cast_slice().unwrap());

        for i in 0..COUNT {
            assert!(vec.push(HasDrop(i)).is_ok());
        }

        drop(vec);
        assert_eq!(DROP_COUNTER.load(Ordering::SeqCst), COUNT);
    }

    #[test]
    fn zst() {
        struct Zst;
        let vec = FixedVec::<Zst>::new(Uninit::empty());
        assert_eq!(vec.capacity(), usize::max_value());
    }

    #[test]
    fn split_and_shrink() {
        // Zeroed because we want to test the contents.
        let mut allocation: MaybeUninit<[u16; 8]> = MaybeUninit::zeroed();

        let mut aligned = Uninit::from(&mut allocation).as_memory();
        let _ = aligned.split_at_byte(15);

        let mut vec = FixedVec::new(aligned.cast_slice().unwrap());
        let mut second = vec.split_and_shrink_to(4);
        let tail = second.shrink_to_fit();

        assert_eq!(vec.capacity(), 4);
        assert_eq!(vec.shrink_to_fit().size(), 0);
        assert_eq!(second.capacity(), 3);
        assert_eq!(second.shrink_to_fit().size(), 0);
        assert_eq!(tail.size(), 1);

        let _ = tail.cast::<u8>().unwrap().init(0xFF);
        (0_u16..4).for_each(|v| assert!(vec.push(v).is_ok()));
        (4..7).for_each(|v| assert!(second.push(v).is_ok()));

        assert_eq!(vec.len(), 4);
        assert_eq!(second.len(), 3);

        drop(vec);
        drop(second);
        assert_eq!(
            &unsafe { *allocation.as_ptr() }[..7],
            [0, 1, 2, 3, 4, 5, 6]);
    }

    /// Tests panics during truncation behave as expected.
    ///
    /// Unwinding started in a panic during truncation should not effect `Drop` calls when the
    /// `Vec` itself is hit by the unwinding. We test this by voluntarily triggering an unwinding
    /// and counting the number of values which have been dropped regularly (that is, during the
    /// `Drop` of `Vec` when it is unwound).
    ///
    /// Note that this test is already `should_panic` and the observable failure is thus an abort
    /// from a double panic!
    #[test]
    #[should_panic = "Trigger triggered"]
    fn drop_safe_in_truncation() {
        let mut allocation: MaybeUninit<[Trigger<'static>; 3]> = MaybeUninit::zeroed();
        let drops = Cell::new(0);

        // Is `Drop`ed *after* the Vec, and will record the number of usually dropped Triggers.
        let _abort_mismatch_raii = AbortMismatchedDropCount {
            counter: &drops,
            expected: 2,
        };

        let uninit = Uninit::from(&mut allocation).as_memory();
        let mut vec = FixedVec::new(uninit.cast_slice().unwrap());

        vec.push(Trigger { panic_on_drop: false, dropped_counter: &drops }).unwrap();
        // This one is within the truncated tail but is not dropped until unwind as truncate
        // panics. If we were to skip dropping all values of the tail in unwind we'd notice.
        vec.push(Trigger { panic_on_drop: false, dropped_counter: &drops }).unwrap();
        vec.push(Trigger { panic_on_drop: true, dropped_counter: &drops }).unwrap();

        // Trigger!
        vec.truncate(1);
    }

    #[test]
    fn fill_drops() {
        let mut allocation: MaybeUninit<[Trigger<'static>; 2]> = MaybeUninit::zeroed();
        let drops = Cell::new(0);

        // Is `Drop`ed *after* the Vec, and will record the number of usually dropped Triggers.
        let _abort_mismatch_raii = AbortMismatchedDropCount {
            counter: &drops,
            expected: 2
        };

        let uninit = Uninit::from(&mut allocation).as_memory();
        let mut vec = FixedVec::new(uninit.cast_slice().unwrap());

        vec.push(Trigger { panic_on_drop: false, dropped_counter: &drops }).unwrap();
        // This should fill the single remaining slot in the Vec. Only one element is
        // instantiated.
        let _ = vec.fill(core::iter::repeat_with(
            || Trigger { panic_on_drop: false, dropped_counter: &drops }));
    }
}
