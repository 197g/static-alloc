use super::*;

macro_rules! assert_impls {(
    for [$($generics:tt)*]
    $T:ty : $($bounds:tt)*
) => (
    const _: () = {
        fn _for<$($generics)*> ()
        {
            fn bounds_check<__>() where
                __ : $($bounds)*
            {}
            let _ = bounds_check::<$T>;
        }
    };
)}

assert_impls! {
    for[
        // The pointees' lack of bounds should not make `Coercion` lack the bounds too.
        T, U : ?Sized,
    ]
    Coercion<T, U> :
        Copy +
        Clone +
        PartialEq +
        Eq +
        ::core::hash::Hash +
}

// More generally,
assert_impls! {
    for[
        // The pointees' lack of bounds should not make `Coercion` lack the bounds too.
        T, U : ?Sized,
        F :
            FnOnce(*const T) -> *const U +
            Copy +
            Clone +
            PartialEq +
            Eq +
            ::core::hash::Hash +
        ,
    ]
    Coercion<T, U, F> :
        Copy +
        Clone +
        PartialEq +
        Eq +
        ::core::hash::Hash +
}
