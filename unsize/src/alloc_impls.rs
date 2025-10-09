use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::sync::Arc;
#[cfg(rustc_1_45)]
use alloc::rc::Weak as WeakRc;
#[cfg(rustc_1_45)]
use alloc::sync::Weak as WeakArc;

use super::CoerciblePtr;

macro_rules! smart_ptr_to_from_raw {
    (unsafe impl $tgt:ident) => (smart_ptr_to_from_raw! {
        unsafe impl $tgt {
            fn as_sized_ptr(&mut self) -> *mut T {
                // Use deref to acquire pointer to self
                // NOTE: Turning this into an &mut T is UB if there is shared ownership
                (&**self) as *const T as *mut T
            }
        }
    });
    (unsafe impl $tgt:ident { $($items:tt)* }) => {
        unsafe impl<T, U: ?Sized> CoerciblePtr<U> for $tgt<T> {
            type Pointee = T;
            type Output = $tgt<U>;
            $($items)*
            unsafe fn replace_ptr(self, new: *mut U) -> $tgt<U> {
                unsafe {
                    // SAFETY: Caller has guarenteed that `new` is
                    // just an unsized version of the original
                    //
                    // Ownership is correctly transfered from `self` to result.

                    // Provenance transfered into `raw` as per each individual `into_raw`.
                    let raw = $tgt::into_raw(self) as *mut T;
                    // Provenance merged into `new` as per `replace_ptr`.
                    let new: *mut U = <*mut T as CoerciblePtr<U>>::replace_ptr(raw, new);
                    // Provenance transferred as per `from_raw`.
                    $tgt::from_raw(new)
                }
            }
        }
    }
}
smart_ptr_to_from_raw!(unsafe impl Box);
smart_ptr_to_from_raw!(unsafe impl Rc);
smart_ptr_to_from_raw!(unsafe impl Arc);
#[cfg(rustc_1_45)] // Weak::from_raw, Weak::as_ptr only on 1.45
smart_ptr_to_from_raw!(unsafe impl WeakRc {
    fn as_sized_ptr(&mut self) -> *mut T {
        // The safety of this implementation is subtle.
        // If there are still strong references, everything works like Rc.
        // If there are no strong references,
        // the result of Weak::as_ptr is well-defined but dangling
        //
        // Furthermore, it is still safe to call
        // Weak::from_raw even with a strong count of zero
        // as long as we still own a weak reference
        WeakRc::as_ptr(self) as *mut T
    }
});
#[cfg(rustc_1_45)] // std::sync::Arc works same as std::rc::Arc
smart_ptr_to_from_raw!(unsafe impl WeakArc {
    fn as_sized_ptr(&mut self) -> *mut T {
        // Safety of this implementation is safe as for WeakRc
        WeakArc::as_ptr(self) as *mut T
    }
});