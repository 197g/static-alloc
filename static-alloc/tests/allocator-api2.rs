extern crate alloc;
use alloc as astd;
use static_alloc::{Bump, leaked::LeakBox, unsync};

#[test]
fn local_vector() {
    let storage = Bump::<[u8; 128]>::uninit();

    let mut v = astd::vec::Vec::<u8, _>::new_in(&storage);
    assert_eq!(v.len(), 0);
    v.extend(0..64);
    assert_eq!(v.len(), 64);

    assert!(
        v.try_reserve(64).is_err(),
        "Reserved more space than available"
    );

    let _ = v.as_mut_slice();
    assert!(v.capacity() <= 128);
}

#[test]
fn unsync_vector() {
    let storage = unsync::Bump::<[u8; 128]>::uninit();

    let mut v = astd::vec::Vec::<u8, _>::new_in(&storage);
    assert_eq!(v.len(), 0);
    v.extend(0..64);
    assert_eq!(v.len(), 64);

    assert!(
        v.try_reserve(64).is_err(),
        "Reserved more space than available"
    );

    let _ = v.as_mut_slice();
    assert!(v.capacity() <= 128);
}

#[test]
fn unsized_leak_box() {
    let storage = Bump::<[u8; 128]>::uninit();
    let boxed = astd::boxed::Box::new_in("look, a &str", &storage);
    // `std`'s box is `Unsize` so we can coerce it to a `Box<dyn Display>` while retaining the
    // allocator. Then that box can be readily converted into our own `LeakBox` to erase the
    // allocator handle and type. This leaves us with a pure dyn-trait object pointer and its
    // lifetime.
    let _decay: LeakBox<'_, dyn core::fmt::Display> =
        LeakBox::from(boxed as astd::boxed::Box<dyn core::fmt::Display, _>);
}
