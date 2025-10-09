use unsize::{Coercion, CoerceUnsized};

#[test]
fn any() {
    use core::any::Any;
    fn generic<T: Any>(ptr: &T) -> &dyn Any {
        ptr.unsize(Coercion::to_any())
    }
    generic(&0u32);
    fn generic_mut<T: Any>(ptr: &mut T) -> &mut dyn Any {
        ptr.unsize(Coercion::to_any())
    }
    generic_mut(&mut 0u32);
    use core::pin::Pin;
    fn generic_mut_pinned<T: Any>(ptr: Pin<&mut T>) -> Pin<&mut dyn Any> {
        ptr.unsize(Coercion::to_any())
    }
    let mut p = (0_u32, ::core::marker::PhantomPinned);
    let p = unsafe {
        Pin::new_unchecked(&mut p)
    };
    generic_mut_pinned(p);
}

#[test]
fn debug() {
    use core::fmt::Debug;
    fn generic<T: Debug>(ptr: &T) -> &dyn Debug {
        ptr.unsize(Coercion::to_debug())
    }
    generic(&0u32);
}

#[test]
fn display() {
    use core::fmt::Display;
    fn generic<T: Display>(ptr: &T) -> &dyn Display {
        ptr.unsize(Coercion::to_display())
    }
    generic(&0u32);
}

#[cfg(rustc_1_51)]
#[test]
fn to_slice() {
    fn generic<T>(ptr: &[T; 4]) -> &[T] {
        ptr.unsize(Coercion::to_slice())
    }
    generic(&[0u32; 4]);
}

#[test]
fn functions() {
    fn arg0<F: 'static + FnOnce()>(fptr: &F) -> &dyn FnOnce() {
        fptr.unsize(Coercion::<_, dyn FnOnce()>::to_fn_once())
    }

    fn arg1<F: 'static + FnOnce(u32)>(fptr: &F) -> &dyn FnOnce(u32) {
        fptr.unsize(Coercion::<_, dyn FnOnce(u32)>::to_fn_once())
    }

    fn arg6<F: 'static + FnOnce(u32,u32,u32,u32,u32,u32)>(fptr: &F)
        -> &dyn FnOnce(u32,u32,u32,u32,u32,u32)
    {
        fptr.unsize(Coercion::<_, dyn FnOnce(u32,u32,u32,u32,u32,u32)>::to_fn_once())
    }

    arg0(&|| {});
    arg1(&|_| {});
    arg6(&|_,_,_,_,_,_| {});
}
