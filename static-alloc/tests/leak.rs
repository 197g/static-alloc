use core::num::NonZeroU16;

use static_alloc::Bump;

#[test]
fn homogeneous() {
    let slab = Bump::<[u64; 3]>::uninit();

    let new_zero = slab.leak(0_u64).unwrap();
    assert_eq!(*new_zero, 0);

    let next = slab.leak(255_u64).unwrap();
    assert_eq!(*next, 255);
    assert_eq!(*new_zero, 0);

    let last = slab.leak(u64::MAX).unwrap();
    assert_eq!(*next, 255);
    assert_eq!(*new_zero, 0);
    assert_eq!(*last, u64::MAX);

    assert!(slab.leak(0_u8).is_err());
}

#[test]
fn heterogeneous() {
    // Avoids additional space usage from alignments: all 3 values fit for an aligned `u16`.
    let slab = Bump::<[u16; 3]>::uninit();

    let intu16: &mut u16 = slab.leak(0).unwrap();
    assert_eq!(*intu16, 0);

    let option: &mut Option<u8> = slab.leak(Some(0_u8)).unwrap();
    assert_eq!(*option, Some(0));

    let nonzero: &mut Option<NonZeroU16> = slab.leak(None).unwrap();
    assert_eq!(*nonzero, None);

    assert!(slab.leak(0_u8).is_err());
}

#[test]
fn zst() {
    let slab = Bump::<()>::uninit();

    slab.leak::<()>(())
        .expect("Could 'allocate' zst in no space");
}

#[test]
fn level() {
    let slab = Bump::<[u16; 2]>::uninit();
    let init = slab.level();

    let (intu16, level) = slab.leak_at(0u16, init).unwrap();
    assert_eq!(*intu16, 0);
    assert!(init < level);
    assert_eq!(slab.level(), level);

    // Can not get the same level again.
    assert_eq!(
        slab.leak_at(0u16, init).unwrap_err().kind(),
        static_alloc::bump::Failure::Mismatch { observed: level }
    );

    let (othu16, next) = slab.leak_at(10u16, level).unwrap();
    assert_eq!(*othu16, 10);
    assert!(init < next);
    assert!(level < next);
    assert_eq!(slab.level(), next);
}
