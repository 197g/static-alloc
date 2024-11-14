//! Contains a proof type, demonstrating the layout equality of two types.
//!
//! This is relevant to allocated containers as well as other reuse of raw memory since layout
//! equality guarantees certain types of soundness. For example, the memory allocated for a
//! `Box<A>` can be reused for storing a type `B` exactly if those two types have the same layout.
//!
//! The module defines a number of helpers (`for_*`) that _guarantee_ construction. Also note that
//! most of the methods are usable in `const` contexts. Albeit, in practice you might need to use a
//! generic lifetime parameter in one of your proofs but this is not possible in constants.
//! Instead, wait for `const {}` blocks to stabilize.
use core::alloc::Layout;
use core::mem::MaybeUninit;
use core::marker::PhantomData;
use core::ptr::NonNull;

use alloc::boxed::Box;
use alloc::vec::Vec;

/// A proof type, showing two types `A` and `B` have the **same** layout.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct SameLayout<A, B>(PhantomData<(A, B)>);

impl<A, B> SameLayout<A, B> {
    pub const fn new() -> Option<Self> {
        let layout_a = Layout::new::<A>();
        let layout_b = Layout::new::<B>();
        // Direct comparison requires `ops::Eq` which obviously is NOT yet const fn.
        // Also this is exactly what we required for allocators, as documented there.
        if layout_a.size() == layout_b.size() && layout_a.align() == layout_b.align() {
            Some(SameLayout(PhantomData))
        } else {
            None
        }
    }

    /// 'Transmute' a vector by reusing its buffer.
    /// NOTE: This will _forget_ all elements. You must clear the vector first if they are
    /// important, or `set_len` on the result if you can guarantee that old elements are valid
    /// initializers for the new type.
    /// This affords more flexibility for the caller as they might want to use As as an initializer
    /// for Bs which would be invalid if we dropped them. Manually drain the vector if this is not
    /// desirable.
    pub fn forget_vec(self, vec: Vec<A>) -> Vec<B> {
        let mut vec = core::mem::ManuallyDrop::new(vec);
        let cap = vec.capacity();
        let ptr = vec.as_mut_ptr();
        // SAFETY:
        // - ptr was previously allocated with Vec.
        // - B has the same alignment and size as per our invariants.
        // - 0 is less than or equal to capacity.
        // - capacity is the capacity the Vec was allocated with.
        // - All elements (there are none) as initialized.
        unsafe { Vec::from_raw_parts(ptr as *mut B, 0, cap) }
    }

    /// 'Transmute' a box by reusing its buffer.
    /// NOTE: for the same flexibility as Vec, forget about the returned `A`.
    pub fn deinit_box(self, boxed: Box<A>) -> (A, Box<MaybeUninit<B>>) {
        let ptr = Box::into_raw(boxed);
        // SAFETY: just was a valid box..
        let a = unsafe { core::ptr::read(ptr) };
        // SAFETY:
        // - ptr was previously allocated with Box.
        // - The ptr is valid for reads and writes as it comes from a Box.
        // - B has the same alignment and size as per our invariants.
        // - Any instance of MaybeUninit is always valid.
        (a, unsafe { Box::from_raw(ptr as *mut MaybeUninit<B>) })
    }
}

impl<A, B> Clone for SameLayout<A, B> {
    fn clone(&self) -> Self {
        SameLayout(self.0)
    }
}

impl<A, B> Copy for SameLayout<A, B> {}

impl<A, B> SameLayout<A, B> {
    pub const fn array<const N: usize>(self) -> SameLayout<[A; N], [B; N]> {
        SameLayout(PhantomData)
    }

    /// Apply a transitive argument to construct a new relation proof.
    pub const fn chain<C>(self, _: SameLayout<B, C>) -> SameLayout<A, C> {
        SameLayout(PhantomData)
    }

    /// Use commutativity of equality.
    pub const fn transpose(self) -> SameLayout<B, A> {
        SameLayout(PhantomData)
    }
}

/// A proof that any type has the same layout as itself.
pub const fn id<A>() -> SameLayout<A, A> {
    SameLayout(PhantomData)
}

/// A proof that any reference has same layout as a raw pointer.
pub const fn for_ref<'a, A: ?Sized>() -> SameLayout<*const A, &'a A> {
    SameLayout(PhantomData)
}

/// A proof that any mutable reference has same layout as a raw pointer.
/// FIXME: this is not const because of the very narrow formulation of https://github.com/rust-lang/rust/issues/57349
pub fn for_mut<'a, A: ?Sized>() -> SameLayout<*const A, &'a mut A> {
    SameLayout(PhantomData)
}

/// A proof that any option wrapped reference has same layout as an pure reference.
pub const fn for_ref_opt<'a, A: ?Sized>() -> SameLayout<&'a A, Option<&'a A>> {
    SameLayout(PhantomData)
}

/// A proof that any option wrapped mutable reference has same layout as an pure reference.
/// FIXME: this is not const because of the very narrow formulation of https://github.com/rust-lang/rust/issues/57349
pub fn for_mut_opt<'a, A: ?Sized>() -> SameLayout<&'a mut A, Option<&'a mut A>> {
    SameLayout(PhantomData)
}

/// A proof that sized pointers have same layout as any other sized pointer.
pub const fn for_sized_ptr<A, B>() -> SameLayout<*const A, *const B> {
    SameLayout(PhantomData)
}

/// A proof that mutable pointer has the same layout as a const pointer.
pub const fn for_ptr_mut<A: ?Sized>() -> SameLayout<*const A, *mut A> {
    SameLayout(PhantomData)
}

/// A proof that a non-null pointer has the same layout as a raw pointer.
pub const fn for_non_null<A: ?Sized>() -> SameLayout<*const A, NonNull<A>> {
    SameLayout(PhantomData)
}

/// A proof that an option of a non-null pointer has the same layout as a raw pointer.
pub const fn for_non_null_opt<A: ?Sized>() -> SameLayout<*const A, Option<NonNull<A>>> {
    SameLayout(PhantomData)
}

/// A proof that any box has same layout as a raw pointer.
pub const fn for_box<A: ?Sized>() -> SameLayout<*const A, Box<A>> {
    SameLayout(PhantomData)
}

/// A proof that any optional box has same layout as a raw pointer.
pub const fn for_box_opt<A: ?Sized>() -> SameLayout<*const A, Option<Box<A>>> {
    SameLayout(PhantomData)
}
