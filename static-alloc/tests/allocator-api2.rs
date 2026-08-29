use allocator_api2 as astd;
use static_alloc::{Bump, unsync};

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

    let _ = v.push_within_capacity(0);
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

    let _ = v.push_within_capacity(0);
    assert!(v.capacity() <= 128);
}
