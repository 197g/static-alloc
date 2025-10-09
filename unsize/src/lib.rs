//! This module encapsulates correct coercers and coercible pointers.
//!
//! The act of Coercion, a special kind of somewhat trivial pointer conversion, is not exposed as a
//! _safe_ trait. There are two unsafe traits: The first captures which structs unsized to which
//! dynamically sized types; which the second is applied to (wrappers around) pointers that can be
//! coerced implicitly.
//!
//! We do not have the luxury of compiler builtin checks to enforce that a particular pointer
//! conversion is sound, nor can we generate the tag of target fat pointer from thin air. Instead,
//! we use a trick. Note that both of these safety issues occur in the context of the pointer
//! conversion. Now, we can require the _user_ to _unsafely_ provide a function that implements the
//! conversion correctly. The _using_ this function is safe and enables any particular user-defined
//! pointer wrapper to safely transform itself. Note that for a limited selection of standard
//! traits we can even go so far as offer pre-built converters that are safe to use in general.
// Copyright 2019-2021 Aurelia Molzer
#![no_std]
#![deny(missing_docs)]

#![allow(unused_unsafe)] // Err on the side of caution.
use core::{
    alloc::Layout,
    future::Future,
    marker::PhantomData,
};

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
mod alloc_impls;

mod impls {
    //! Safety: Provenance is always the same as self, pointer target is simply passed through.
    use core::ptr::NonNull;
    use super::CoerciblePtr;

    unsafe impl<T, U: ?Sized> CoerciblePtr<U> for *const T {
        type Pointee = T;
        type Output = *const U;
        fn as_sized_ptr(self: &mut *const T) -> *mut T {
            (*self) as *const T as *mut T
        }
        unsafe fn replace_ptr(self, new: *mut U) -> *const U {
            // See the mutable version.
            super::unsize_with(self as *mut T, |_| new)
        }
    }

    unsafe impl<T, U: ?Sized> CoerciblePtr<U> for *mut T {
        type Pointee = T;
        type Output = *mut U;
        fn as_sized_ptr(self: &mut *mut T) -> *mut T {
            *self
        }
        unsafe fn replace_ptr(self, new: *mut U) -> *mut U {
            // See the mutable version.
            super::unsize_with(self, |_| new)
        }
    }

    unsafe impl<'lt, T, U: ?Sized + 'lt> CoerciblePtr<U> for &'lt T {
        type Pointee = T;
        type Output = &'lt U;
        fn as_sized_ptr(self: &mut &'lt T) -> *mut T {
            (*self) as *const T as *mut T
        }
        unsafe fn replace_ptr(self, new: *mut U) -> &'lt U {
            // See the mutable version.
            unsafe { &*super::unsize_with(self as *const T as *mut T, |_| new) }
        }
    }

    /// Safety: Provenance is always the same as self.
    unsafe impl<'lt, T, U: ?Sized + 'lt> CoerciblePtr<U> for &'lt mut T {
        type Pointee = T;
        type Output = &'lt mut U;
        fn as_sized_ptr(self: &mut &'lt mut T) -> *mut T {
            &mut **self
        }
        unsafe fn replace_ptr(self, new: *mut U) -> &'lt mut U {
            // (Explanation should apply to the const version too).
            // We want the `new` pointer with provenance of `self`. This is because in
            // `as_sized_ptr` we had only borrowed the mutably reference and the usage of passing
            // it as argument to this method has invalidated this borrow.
            // We reuse `unsize_with` to set `self` as the pointer value in `new`. This is okay
            // because `new` should already be an unsized version, we merely make use of its
            // builtin provenance copy operation.
            unsafe { &mut *super::unsize_with(self, |_| new) }
        }
    }

    unsafe impl<Ptr, U : ?Sized, T> CoerciblePtr<U> for core::pin::Pin<Ptr>
    where
        Ptr: CoerciblePtr<U, Pointee=T> + core::ops::DerefMut<Target=T>,
        Ptr::Output: core::ops::DerefMut<Target=U>,
    {
        type Pointee = T;
        type Output = core::pin::Pin<Ptr::Output>;
        fn as_sized_ptr(&mut self) -> *mut Self::Pointee {
            unsafe { self.as_mut().get_unchecked_mut() }
        }
        unsafe fn replace_ptr(self, new: *mut U) -> Self::Output {
            let inner = core::pin::Pin::into_inner_unchecked(self);
            let new = inner.replace_ptr(new);
            core::pin::Pin::new_unchecked(new)
        }
    }

    unsafe impl<T, U: ?Sized> CoerciblePtr<U> for core::ptr::NonNull<T> {
        type Pointee = T;
        type Output = NonNull<U>;
        fn as_sized_ptr(&mut self) -> *mut T {
            self.as_ptr()
        }
        unsafe fn replace_ptr(self, new: *mut U) -> NonNull<U> {
            // Safety:
            NonNull::new_unchecked(new)
        }
    }
}

/// Enables the unsizing of a sized pointer.
#[repr(C)]
pub struct Coercion<T, U : ?Sized, F : FnOnce(*const T) -> *const U = fn(*const T) -> *const U> {
    pub(crate) coerce: F,
    pub(crate) _phantom: PhantomData<fn(*const T) -> *const U>,
}

/// Common trait impls for `Coercion`.
mod coercion_impls;

impl<F, T, U: ?Sized> Coercion<T, U, F>
where
    F : FnOnce(*const T) -> *const U,
{
    /// Construct a new coercer.
    ///
    /// # Safety
    ///
    /// The method must not perform any action other than unsizing the pointer.
    ///
    /// # Usage
    ///
    /// ```
    /// use unsize::Coercion;
    /// use core::fmt::Debug;
    ///
    /// let c: Coercion<u32, dyn Debug> = unsafe {
    ///     Coercion::new(|x| x)
    /// };
    /// ```
    pub unsafe fn new(coerce: F) -> Self {
        Coercion { coerce, _phantom: PhantomData }
    }
}

macro_rules! coerce_to_dyn_trait {
    (
        $(for <$($generics:ident),* $(,)?>)?
        $(#[$attr:meta])* fn $name:ident() -> $trait_type:path
    ) => {
        impl<'lt, T: $trait_type + 'lt, $($($generics),*)?>
            Coercion<T, dyn $trait_type + 'lt>
        {
            $(#[$attr])*
            pub fn $name() -> Self {
                fn coerce_to_that_type<'lt, T: $trait_type + 'lt, $($($generics),*)?>(
                    ptr: *const T
                ) -> *const (dyn $trait_type + 'lt) {
                    ptr
                }

                unsafe { Coercion::new(coerce_to_that_type) }
            }
        }
    };

    /* TODO: figure out how to make this work.
     * Then add Iterator<Item=U>, PartialEq<Rhs>, PartialOrd<Rhs>, etc.
    ($(#[$attr:meta])* fn $name:ident<$($param:ident),*>() -> $trait_type:ty) => {
        impl<'lt, $($param),*, T: $trait_type + 'lt> Coercion<T, dyn ($trait_type + 'lt)> {
            $(#[$attr])*
            pub fn $name() -> Self {
                fn coerce_to_that_type<'lt, $($param),*, T: $trait_type + 'lt>(
                    ptr: *const T
                ) -> *const (dyn $trait_type + 'lt) {
                    ptr
                }

                Coercion { coerce: coerce_to_that_type }
            }
        }
    };
    */
}

coerce_to_dyn_trait!(
    /// Create a coercer that unsizes and keeps dynamic type information.
    ///
    /// # Usage
    ///
    /// ```
    /// use unsize::{Coercion, CoerceUnsized};
    /// use core::any::Any;
    ///
    /// fn generic<T: Any>(ptr: &T) -> &dyn Any {
    ///     ptr.unsize(Coercion::to_any())
    /// }
    /// ```
    fn to_any() -> core::any::Any
);

coerce_to_dyn_trait!(
    /// Create a coercer that unsizes a parameter to dynamically debug its fields.
    ///
    /// # Usage
    ///
    /// ```
    /// use unsize::{Coercion, CoerceUnsized};
    /// use core::fmt::Debug;
    ///
    /// fn generic<T: Debug>(ptr: &T) -> &dyn Debug {
    ///     ptr.unsize(Coercion::to_debug())
    /// }
    /// ```
    fn to_debug() -> core::fmt::Debug
);

coerce_to_dyn_trait!(
    /// Create a coercer that unsizes a parameter to display it.
    ///
    /// # Usage
    ///
    /// ```
    /// use unsize::{Coercion, CoerceUnsized};
    /// use core::fmt::Display;
    ///
    /// fn generic<T: Display>(ptr: &T) -> &dyn Display {
    ///     ptr.unsize(Coercion::to_display())
    /// }
    /// ```
    fn to_display() -> core::fmt::Display
);

#[cfg(rustc_1_51)]
impl<T, const N: usize> Coercion<[T; N], [T]> {
    /// Create a coercer that unsizes an array to a slice.
    ///
    /// # Usage
    ///
    /// ```
    /// use unsize::{Coercion, CoerceUnsized};
    /// use core::fmt::Display;
    ///
    /// fn generic<T>(ptr: &[T; 2]) -> &[T] {
    ///     ptr.unsize(Coercion::to_slice())
    /// }
    /// ```
    pub fn to_slice() -> Self {
        fn coerce<T, const N: usize>(
            ptr: *const [T; N]
        ) -> *const [T] { ptr }
        unsafe { Coercion::new(coerce) }
    }
}

macro_rules! coerce_to_dyn_fn {
    (
        $(#![$attr:meta])?
        $($arg:ident),*
    ) => {
        coerce_to_dyn_fn!(
            $(#![$attr])?
            @<$($arg,)*>:
            (dyn Fn($($arg,)*) -> T + 'lt),
            (dyn FnMut($($arg,)*) -> T + 'lt),
            (dyn FnOnce($($arg,)*) -> T + 'lt)
        );
    };
    (
        $(#![$attr:meta])?
        @<$($arg:ident,)*>: $dyn:ty, $dyn_mut:ty, $dyn_once:ty
    ) => {
        coerce_to_dyn_trait! { for<Ret, $($arg),*>
            $(#[$attr])?
            /// Create a coercer that unsizes to a dynamically dispatched function.
            ///
            /// This is implemented for function arities up to the shown one
            /// (other methods / impls are hidden in the docs for readability)
            fn to_fn() -> Fn($($arg),*) -> Ret
        }
        coerce_to_dyn_trait! { for<Ret, $($arg),*>
            $(#[$attr])?
            /// Create a coercer that unsizes to a dynamically dispatched mutable function.
            ///
            /// This is implemented for function arities up to the shown one
            /// (other methods / impls are hidden in the docs for readability)
            fn to_fn_mut() -> FnMut($($arg),*) -> Ret
        }
        coerce_to_dyn_trait! { for<Ret, $($arg),*>
            $(#[$attr])?
            /// Create a coercer that unsizes to a dynamically dispatched once function.
            ///
            /// This is implemented for function arities up to the shown one
            /// (other methods / impls are hidden in the docs for readability)
            fn to_fn_once() -> FnOnce($($arg),*) -> Ret
        }
    };
}

coerce_to_dyn_fn!(#![doc(hidden)] );
coerce_to_dyn_fn!(#![doc(hidden)] A);
coerce_to_dyn_fn!(#![doc(hidden)] A,B);
coerce_to_dyn_fn!(#![doc(hidden)] A,B,C);
coerce_to_dyn_fn!(#![doc(hidden)] A,B,C,D);
coerce_to_dyn_fn!(#![doc(hidden)] A,B,C,D,E);
coerce_to_dyn_fn!(A,B,C,D,E,G);

coerce_to_dyn_trait! {
    for<I,>
    /// Create a coercer that unsizes to a dynamically dispatched iterator.
    ///
    /// This is implemented for all iterator types. It can type-erase the concrete type to wrap an
    /// otherwise unnameable adapter in a custom smart pointer, to store it within a struct.
    ///
    /// # Usage
    ///
    /// ```
    /// # use unsize::CoerciblePtr;
    /// // A non-coercible box, for demonstration purposes
    /// struct MyBox<T: ?Sized>(Box<T>);
    ///
    /// unsafe impl<'lt, T, U: ?Sized + 'lt> CoerciblePtr<U> for MyBox<T> {
    ///     // …
    /// #   type Pointee = T;
    /// #   type Output = MyBox<U>;
    /// #   fn as_sized_ptr(self: &mut MyBox<T>) -> *mut T {
    /// #       (&mut *self.0) as *mut T
    /// #   }
    /// #   unsafe fn replace_ptr(self, new: *mut U) -> MyBox<U> {
    /// #       let raw: *mut T = Box::into_raw(self.0);
    /// #       let raw: *mut U = raw.replace_ptr(new);
    /// #       MyBox(Box::from_raw(raw))
    /// #   }
    /// }
    /// # impl<T> MyBox<T> {
    /// #    pub fn new(val: T) -> Self { MyBox(std::boxed::Box::new(val)) }
    /// # }
    ///
    /// use unsize::{Coercion, CoerceUnsized};
    /// use core::fmt::Display;
    ///
    /// fn maybe_empty<T: Clone>(item: &T) -> MyBox<dyn Iterator<Item=T> + '_> {
    ///     if core::mem::size_of::<T>() % 2 == 0 {
    ///         MyBox::new(core::iter::empty())
    ///             .unsize(Coercion::to_iterator())
    ///     } else {
    ///         MyBox::new(core::iter::repeat(item.clone()))
    ///             .unsize(Coercion::to_iterator())
    ///     }
    /// }
    /// ```
    fn to_iterator() -> Iterator<Item=I>
}

coerce_to_dyn_trait! {
    for<I,>
    /// Create a coercer that unsizes to a dynamically dispatched future.
    ///
    /// This is implemented for all iterator types. It can type-erase the concrete type to wrap an
    /// otherwise unnameable adapter in a custom smart pointer, to store it within a struct.
    ///
    /// # Usage
    ///
    /// ```
    /// # use core::future::Future;
    /// # use unsize::CoerciblePtr;
    /// // A non-coercible box, for demonstration purposes
    /// struct MyBox<T: ?Sized>(Box<T>);
    ///
    /// unsafe impl<'lt, T, U: ?Sized + 'lt> CoerciblePtr<U> for MyBox<T> {
    ///     // …
    /// #   type Pointee = T;
    /// #   type Output = MyBox<U>;
    /// #   fn as_sized_ptr(self: &mut MyBox<T>) -> *mut T {
    /// #       (&mut *self.0) as *mut T
    /// #   }
    /// #   unsafe fn replace_ptr(self, new: *mut U) -> MyBox<U> {
    /// #       let raw: *mut T = Box::into_raw(self.0);
    /// #       let raw: *mut U = raw.replace_ptr(new);
    /// #       MyBox(Box::from_raw(raw))
    /// #   }
    /// }
    /// # impl<T> MyBox<T> {
    /// #    pub fn new(val: T) -> Self { MyBox(std::boxed::Box::new(val)) }
    /// # }
    ///
    /// use unsize::{Coercion, CoerceUnsized};
    /// use core::fmt::Display;
    ///
    /// fn maybe_empty<T: 'static>(val: T) -> MyBox<dyn Future<Output=T>> {
    ///     if core::mem::size_of::<T>() % 2 == 0 {
    ///         MyBox::new(core::future::pending())
    ///             .unsize(Coercion::to_future())
    ///     } else {
    ///         MyBox::new(core::future::ready(val))
    ///             .unsize(Coercion::to_future())
    ///     }
    /// }
    /// ```
    fn to_future() -> Future<Output=I>
}

/// ```
/// use unsize::{Coercion, CoerceUnsized};
/// fn arg0<F: 'static + FnOnce()>(fptr: &F) -> &dyn FnOnce() {
///     fptr.unsize(Coercion::<_, dyn FnOnce()>::to_fn_once())
/// }
/// fn arg1<F: 'static + FnOnce(u32)>(fptr: &F) -> &dyn FnOnce(u32) {
///     fptr.unsize(Coercion::<_, dyn FnOnce(u32)>::to_fn_once())
/// }
/// fn arg6<F: 'static + FnOnce(u32,u32,u32,u32,u32,u32)>(fptr: &F)
///     -> &dyn FnOnce(u32,u32,u32,u32,u32,u32)
/// {
///     fptr.unsize(Coercion::<_, dyn FnOnce(u32,u32,u32,u32,u32,u32)>::to_fn_once())
/// }
/// arg0(&|| {});
/// arg1(&|_| {});
/// arg6(&|_,_,_,_,_,_| {});
/// ```
extern {}

/// Add unsizing methods to pointer-like types.
///
/// # Safety
/// A correct implementation must uphold, when calling `replace_ptr` with valid arguments, that the
/// pointer target and the provenance of the pointer stay unchanged. This allows calling the
/// coercion of inner fields of wrappers even when an invariant depends on the pointer target.
pub unsafe trait CoerciblePtr<U: ?Sized>: Sized {
    /// The type we point to.
    /// This influences which kinds of unsizing are possible.
    type Pointee;
    /// The output type when unsizing the pointee to `U`.
    type Output;
    /// Get the raw inner pointer.
    fn as_sized_ptr(&mut self) -> *mut Self::Pointee;
    /// Replace the container inner pointer with an unsized version.
    /// # Safety
    /// The caller guarantees that the replacement is the same pointer, just a fat pointer variant
    /// with a correct tag.
    unsafe fn replace_ptr(self, _: *mut U) -> Self::Output;
}

/// An extension trait using `CoerciblePtr` for a safe interface.
pub trait CoerceUnsized<U: ?Sized>: CoerciblePtr<U> {
    /// Convert a pointer, as if with unsize coercion.
    ///
    /// See [`CoerciblePtr::unsize_with`][unsize_with] for details.
    ///
    /// [unsize_with]: struct.CoerciblePtr.html#method.unsize_with
    fn unsize<F>(mut self, with: Coercion<Self::Pointee, U, F>) -> Self::Output
    where
        F : FnOnce(*const Self::Pointee) -> *const U,
    {
        unsafe {
            let ptr = self.as_sized_ptr();
            let new_ptr = unsize_with(ptr, with.coerce);
            self.replace_ptr(new_ptr)
        }
    }
}

/// A deprecated alias for the `CoerceUnsized` trait.
/// It will be hidden in a few releases from now.
#[doc(no_inline)]
#[deprecated = "This was initially misnamed"]
pub use self::CoerceUnsized as CoerceUnsize;

impl<T, U: ?Sized> CoerceUnsized<U> for T
where
    T: CoerciblePtr<U>
{}

/// Convert a pointer, as if with unsize coercion.
///
/// The result pointer will have the same provenance information as the argument `ptr`.
///
/// # Safety
///
/// The caller must guarantee that it is sound to dereference the argument and convert it into a
/// reference. This also, very slightly, relies on some of Rust's internal layout but it will
/// assert that they actually hold. If this sounds too risky, do not use this method. The caller
/// must also guarantee that `with` will only execute a coercion and _not_ change the pointer
/// itself.
unsafe fn unsize_with<T, U: ?Sized>(
    ptr: *mut T,
    with: impl FnOnce(*const T) -> *const U,
) -> *mut U {
    let mut raw_unsized = with(ptr) as *mut U;

    // Not a debug assert since it hopefully monomorphizes to a no-op (or an
    // unconditional panic should multi-trait objects end up happening).
    assert_eq!(Layout::for_value(&raw_unsized), Layout::new::<[usize; 2]>(),
        "Unexpected layout of unsized pointer.");
    debug_assert_eq!(raw_unsized as *const u8 as usize, ptr as usize,
        "Unsize coercion seemingly changed the pointer base");

    let ptr_slot = &mut raw_unsized as *mut *mut U as *mut *const u8;
    // Safety: Not totally clear as it relies on the standard library implementation of pointers.
    // The layout is usually valid for such a write (we've asserted that above) and all pointers
    // are larger and at least as aligned as a single pointer to bytes.
    // It could be that this invalidates the representation of `raw_unsized` but all currently
    // pointers store their tag _behind_ the base pointer.
    //
    // There is an `unsafe`, unstable method (#75091) with the same effect and implementation.
    //
    // According to the `ptr::set_ptr_value` method we change provenance back to the `raw` pointer.
    // This can be used since we haven't used any pointer from which it was derived. This
    // invalidates the access tags of `temp_reference` and the original `raw_unsized` value but
    // both will no longer be used.
    unsafe { ptr_slot.write(ptr as *const u8) };

    // Safety: the base pointer that we've just written was not null.
    raw_unsized
}

/// Ensure that using `CoerceUnsized` does not import as_sized_ptr.
///
/// ```compile_fail
/// use unsize::CoerceUnsized;
/// use core::ptr::NonNull;
///
/// let ptr = NonNull::from(&2u32);
/// let _ = ptr.as_sized_ptr();
/// ```
extern {}

#[cfg(test)]
mod tests;

/// Non-`unsafe` [`struct@Coercion`] constructor for arbitrary trait bounds.
///
/// # Example
// (and test!)
///
/// ```rust
/// use unsize::{Coercion, CoerceUnsized};
///
/// trait MyFancyTrait { /* … */ }
///
/// fn generic<T: MyFancyTrait>(ptr: &T) -> &dyn MyFancyTrait {
///     ptr.unsize(Coercion!(to dyn MyFancyTrait))
/// }
/// ```
#[macro_export]
macro_rules! Coercion {
    (to dyn $($bounds:tt)*) => (
        #[allow(unused_unsafe)] unsafe {
            $crate::Coercion::new({
                #[allow(unused_parens)]
                fn coerce<'lt> (p: *const (impl $($bounds)* + 'lt)) -> *const (dyn $($bounds)* + 'lt) {
                    p
                }
                coerce
            })
        }
    );
}
