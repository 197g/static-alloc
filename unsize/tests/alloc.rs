//! Tests that require the `alloc` feature.
#![cfg(feature = "alloc")]
use std::fmt::Debug;
use std::rc::Rc;
use std::sync::Arc;

use unsize::{Coercion, CoerceUnsized, CoerciblePtr};


/// Does an arbitrary coercion for `dyn Debug`
fn arbitrary_debug<T: CoerciblePtr<dyn Debug, Output=U>, U>(ptr: T) -> U
    where T::Pointee: Debug + 'static {
    ptr.unsize(Coercion::to_debug())
}

const TEST_VAL: &str = "foo";
fn check_debug<U: AsRef<dyn Debug>>(val: U) {
    assert_eq!(format!("{:?}", val.as_ref()), format!("{:?}", TEST_VAL))
}

#[test]
fn smart_ptrs() {
    check_debug(arbitrary_debug::<Box<String>, Box<dyn Debug>>(Box::new(TEST_VAL.into())));
    check_debug(arbitrary_debug::<Rc<String>, Rc<dyn Debug>>(Rc::new(TEST_VAL.into())));
    check_debug(arbitrary_debug::<Arc<String>, Arc<dyn Debug>>(Arc::new(TEST_VAL.into())));
}

#[test]
#[cfg(rustc_1_45)]
fn smart_ptrs_weak() {
    use std::rc::{Rc, Weak as WeakRc};
    use std::sync::{Arc, Weak as WeakArc};
    macro_rules! test_weak_ptr {
        ($strong:ident, $weak:ident) => ({
            {
                let strong: $strong<String> = $strong::new(TEST_VAL.into());
                {
                    // coerce weak
                    let weak = $strong::downgrade(&strong);
                    let weak = arbitrary_debug::<$weak<String>, $weak<dyn Debug>>(weak);
                    check_debug($weak::upgrade(&weak).unwrap());
                }
                {
                    // coerce strong, then downgrade to dynamic weak
                    let strong = arbitrary_debug::<$strong<String>, $strong<dyn Debug>>(strong);
                    let weak: $weak<dyn Debug> = $strong::downgrade(&strong);
                    check_debug($weak::upgrade(&weak).unwrap());
                }
            };
            // Check dangling pointers
            arbitrary_debug::<$weak<String>, $weak<dyn Debug>>($weak::new());
        });
    }
    test_weak_ptr!(Rc, WeakRc);
    test_weak_ptr!(Arc, WeakArc);
}