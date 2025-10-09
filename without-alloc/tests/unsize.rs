use unsize::{Coercion, CoerceUnsize};
use without_alloc::{boxed::Box, Uninit};
use core::mem::MaybeUninit;

#[test]
fn unsizing() {
    let mut memory: MaybeUninit<usize> = MaybeUninit::uninit();
    let boxed = Box::new(0usize, Uninit::from(&mut memory));

    let debug: Box<dyn core::fmt::Debug> = boxed.unsize(Coercion::to_debug());

    assert_eq!(format!("{:?}", &*debug), "0");
}
