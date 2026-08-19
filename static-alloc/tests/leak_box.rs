use core::{cell::RefCell, mem::MaybeUninit};
use static_alloc::{Bump, leaked::LeakBox};

#[test]
fn leak_box_drops() {
    let bump: Bump<[*const (); 4]> = Bump::uninit();
    let cell = RefCell::new(());
    assert!(cell.try_borrow().is_ok());

    let _ = bump.leak_box(cell.borrow()).unwrap();
    assert!(cell.try_borrow().is_ok(), "Immediately dropped it");
    assert!(cell.try_borrow_mut().is_ok(), "Immediately dropped it");

    let leak_box = bump.leak_box(cell.borrow()).unwrap();
    assert!(cell.try_borrow().is_ok(), "Borrow works, of course");
    assert!(cell.try_borrow_mut().is_err(), "Still borrowed");
    drop(leak_box);

    assert!(cell.try_borrow_mut().is_ok(), "No longer borrowed");
}

#[test]
fn leaking() {
    struct PanicOnDrop(#[expect(unused)] usize);
    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("Do not drop me.");
        }
    }

    let bump: Bump<[usize; 1]> = Bump::uninit();
    let leak_box = bump.leak_box(PanicOnDrop(0)).unwrap();
    core::mem::forget(leak_box);
    // Panic averted.
}

#[test]
fn init() {
    let bump: Bump<[usize; 1]> = Bump::uninit();
    let leak_box = bump.leak_box(MaybeUninit::uninit()).unwrap();
    let init = LeakBox::write(leak_box, 0usize);
    assert_eq!(*init, 0usize);
}

#[test]
fn trait_impls() {
    let mut memory = MaybeUninit::uninit();
    let mut leak_box = LeakBox::write(LeakBox::from(&mut memory), 0usize);

    // AsMut and AsRef
    *leak_box.as_mut() = 1;
    assert_eq!(1usize, *leak_box.as_ref());
    // Deref and DerefMut
    *leak_box = 2;
    assert_eq!(2usize, *leak_box);
    // Debug, Display, Pointer
    println!("{:?}", leak_box);
    println!("{}", leak_box);
    println!("{:p}", leak_box);
}

#[test]
fn questionable_copy() {
    let mut value = 0;
    let mut quote_leak_box_unquote = LeakBox::from_mut(&mut value);
    *quote_leak_box_unquote = 1;
    drop(quote_leak_box_unquote);
    assert_eq!(value, 1)
}
