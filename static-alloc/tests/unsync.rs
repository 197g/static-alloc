use core::mem::MaybeUninit;
use static_alloc::leaked::LeakBox;
use static_alloc::unsync::MemBump;

#[test]
fn raw_from_mem() {
    let mut memory = [MaybeUninit::new(0); 128];
    let bump = MemBump::from_mem(&mut memory)
        .expect("Enough memory for its metadata");

    let n1 = bump.bump_box::<u64>().unwrap();
    let n2 = bump.bump_box::<u64>().unwrap();
    let n3 = bump.bump_box::<u64>().unwrap();

    let mut n1 = LeakBox::write(n1, 10);
    let mut n2 = LeakBox::write(n2, 20);
    let mut n3 = LeakBox::write(n3, 30);

    *n1 += 1;
    *n2 += 2;
    *n3 += 3;

    assert_eq!(*n1, 11);
    assert_eq!(*n2, 22);
    assert_eq!(*n3, 33);
}

#[test]
#[cfg(feature = "alloc")]
fn allocate_with_fixed_capacity() {
    const CAPACITY: usize = 16;
    let bump = MemBump::new(CAPACITY);
    for i in 0..CAPACITY {
        bump.get::<u8>().unwrap_or_else(|| {
            panic!("works {}", i)
        });
    }
    assert!(bump.get::<u8>().is_none());
}
