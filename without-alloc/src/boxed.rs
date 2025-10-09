//! A pointer type for allocation with RAII semantics.
//!
//! See [`Box`] for more information.
//!
//! [`Box`]: struct.Box.html
use core::{borrow, cmp, fmt, hash, mem, ops, ptr};
use crate::uninit::Uninit;
use unsize::CoerciblePtr;

/// An allocated instance of a type.
///
/// ## Example
///
/// The basic usage are allocated recursive data structures. Here is a standard example using a
/// `Bump` with `'static` storage duration as the allocator:
///
/// ```
/// use without_alloc::{Box, alloc::LocalAllocLeakExt};
/// use static_alloc::Bump;
///
/// #[derive(Debug)]
/// enum List<T> {
///     Nil,
///     Cons(T, Box<'static, List<T>>),
/// }
///
/// static SLAB: Bump<[u8; 1024]> = Bump::uninit();
///
/// let base = SLAB.boxed(List::Nil).unwrap();
/// let one = SLAB.boxed(List::Cons(0, base)).unwrap();
/// let two = SLAB.boxed(List::Cons(1, one)).unwrap();
///
/// // We can destruct the value (not with `*` but comparable).
/// match Box::take(two).0 {
///     List::Cons(val, _)  => assert_eq!(val, 1), // Got the value back.
///     _ => unreachable!(),
/// }
/// ```
///
/// ## Downsides
///
/// Unfortunately, this `Box` does not yet support unsizing. This is very unfortunate as it means
/// you can't use it for type erasure.
///
/// ## Design
/// You will likely notice quickly that this has different semantics than the `std::boxed::Box`.
/// Its inner pointer may be larger and it does not allocate nor deallocate memory on its own. This
/// only wraps a fully initialized `Uninit` in a RAII/`Drop` interface.
///
/// Of course, there is a reason for this choice. The standard `Box`, storing only the raw pointer
/// (as a `Unique`), requires its underlying allocation to have the *exact* same size and align
/// (`Layout`) as the value and the layout needs to be recalculated when deallocating. Without a
/// dependency on an allocator it would seem that the underlying layout becomes less important and
/// can be thrown away but the opposite is the case. Many converters for the `std::boxed::Box` rely
/// on being able to reallocate into a suitably constructed new allocation on will. Not having this
/// luxury at our disposal means there should be a mechanism to cope with mismatching allocations
/// anyways. So we simply store the full `Uninit` provided, relying on the library user to manage
/// other aspects of allocation for us.
///
/// Instead, this `Box` can offer additional ways to manipulate and massage the underlying
/// allocation. It should be possible to restore the exact allocation `Box` semantics (albeit with
/// one `usize` more space usage) via a wrapper when an allocator is available.
pub struct Box<'a, T: ?Sized> {
    inner: Uninit<'a, T>,
}


/// Unsize a Box, for example into a dynamic trait object.
///
/// # Usage
///
/// ```
/// # use without_alloc::boxed::Box;
/// use unsize::{Coercion, CoerceUnsize};
/// use without_alloc::Uninit;
/// use core::mem::MaybeUninit;
///
/// let mut memory: MaybeUninit<usize> = MaybeUninit::uninit();
/// let boxed = Box::new(0usize, Uninit::from(&mut memory));
///
/// let debug: Box<dyn core::fmt::Debug> = unsafe {
///     boxed.unsize(Coercion::to_debug())
/// };
/// ```
unsafe impl<'a, T, U: ?Sized> CoerciblePtr<U> for Box<'a, T> {
    type Pointee = T;
    type Output = Box<'a, U>;
    fn as_sized_ptr(&mut self) -> *mut T {
        self.inner.as_ptr()
    }
    unsafe fn replace_ptr(self, new: *mut U) -> Box<'a, U> {
        let inner = Box::into_raw(self);
        let inner = inner.replace_ptr(new);
        Box::from_raw(inner)
    }
}

impl<'a, T> Box<'a, T> {
    /// Place `val` into a provided allocation.
    pub fn new(val: T, mut into: Uninit<'a, T>) -> Self {
        into.borrow_mut().init(val);

        Box {
            inner: into,
        }
    }

    /// Take out the value and return the allocation.
    ///
    /// This function is the opposite of `new`.
    pub fn take(b: Self) -> (T, Uninit<'a, T>) {
        let val = unsafe { b.inner.read() };
        (val, Self::into_raw(b))
    }
}

impl<'a, T: ?Sized> Box<'a, T> {
    /// Create a box from an pointer to an already initialized value.
    ///
    /// Ensures that an already initialized value is properly dropped at the end of the lifetime of
    /// the `Box`.
    ///
    /// ## Safety
    /// The pointed-to location must have already been initialized via external means. This is as
    /// unsafe as `init.as_mut()`.
    pub unsafe fn from_raw(init: Uninit<'a, T>) -> Self {
        Box {
            inner: init,
        }
    }

    /// Unwrap the contained `Uninit`.
    ///
    /// The value stays initialized but that information is no longer statically available. If you
    /// simply want to avoid the `Drop` call, consider `ManuallyDrop` instead.
    pub fn into_raw(b: Self) -> Uninit<'a, T> {
        let ptr = b.inner.as_non_null();
        let len = b.inner.size();
        mem::forget(b);
        unsafe {
            // SAFETY: restored the `Uninit` we just forgot.
            Uninit::new(ptr, len)
        }
    }

    /// Consumes and leaks the Box, returning a mutable reference, `&'a mut T`.
    ///
    /// Compared to a standard `Box` it should be noted that the reference alone is not enough
    /// to invoke `Box::from_raw`.
    pub fn leak(b: Self) -> &'a mut T {
        let raw = Self::into_raw(b);
        // SAFETY: still initialized
        unsafe { raw.into_mut() }
    }
}

impl<T: ?Sized> Drop for Box<'_, T> {
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(self.inner.as_ptr())
        }
    }
}

impl<T: ?Sized> ops::Deref for Box<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe {
            self.inner.as_ref()
        }
    }
}

impl<T: ?Sized> ops::DerefMut for Box<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe {
            self.inner.as_mut()
        }
    }
}

impl<'a, 'b, T: PartialEq> PartialEq<Box<'b, T>> for Box<'a, T> {
    #[inline]
    fn eq(&self, other: &Box<T>) -> bool {
        PartialEq::eq(&**self, &**other)
    }
    #[inline]
    fn ne(&self, other: &Box<T>) -> bool {
        PartialEq::ne(&**self, &**other)
    }
}

impl<'a, 'b, T: PartialOrd> PartialOrd<Box<'b, T>> for Box<'a, T> {
    #[inline]
    fn partial_cmp(&self, other: &Box<T>) -> Option<cmp::Ordering> {
        PartialOrd::partial_cmp(&**self, &**other)
    }
    #[inline]
    fn lt(&self, other: &Box<T>) -> bool {
        PartialOrd::lt(&**self, &**other)
    }
    #[inline]
    fn le(&self, other: &Box<T>) -> bool {
        PartialOrd::le(&**self, &**other)
    }
    #[inline]
    fn ge(&self, other: &Box<T>) -> bool {
        PartialOrd::ge(&**self, &**other)
    }
    #[inline]
    fn gt(&self, other: &Box<T>) -> bool {
        PartialOrd::gt(&**self, &**other)
    }
}

impl<T: Ord> Ord for Box<'_, T> {
    #[inline]
    fn cmp(&self, other: &Box<T>) -> cmp::Ordering {
        Ord::cmp(&**self, &**other)
    }
}

impl<T: Eq> Eq for Box<'_, T> { }

impl<T: hash::Hash> hash::Hash for Box<'_, T> {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

impl<T: fmt::Debug> fmt::Debug for Box<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T> fmt::Pointer for Box<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.inner.as_ptr(), f)
    }
}

impl<T> borrow::Borrow<T> for Box<'_, T> {
    fn borrow(&self) -> &T {
        &**self
    }
}

impl<T> borrow::BorrowMut<T> for Box<'_, T> {
    fn borrow_mut(&mut self) -> &mut T {
        &mut **self
    }
}

impl<T> AsRef<T> for Box<'_, T> {
    fn as_ref(&self) -> &T {
        &**self
    }
}

impl<T> AsMut<T> for Box<'_, T> {
    fn as_mut(&mut self) -> &mut T {
        &mut **self
    }
}

#[cfg(test)]
mod tests {
    use super::Box;
    use crate::alloc::LocalAllocLeakExt;
    use static_alloc::Bump;

   #[test]
    fn leak_with_smaller_lifetime() {
        static SLAB: Bump<[usize; 1]> = Bump::uninit();
        let local = 0;

        // Box is `'static` but variable is not.
        let boxed: Box<'static, _> = SLAB.boxed(&local).unwrap();
        // Check that we can leak with appropriate lifetime.
        let _: & mut _ = Box::leak(boxed);
        // local needs to be live for the time before the box is leaked
        let _ = local;
    }
}
