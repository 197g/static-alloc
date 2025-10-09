use super::*;

use ::core::{
    fmt,
    hash::{Hash, Hasher},
};

impl<T, U : ?Sized, F : FnOnce(*const T) -> *const U> Clone
    for Coercion<T, U, F>
where
    F : Clone,
{
    fn clone(&self) -> Self {
        Self {
            coerce: self.coerce.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<T, U : ?Sized, F : FnOnce(*const T) -> *const U> Copy
    for Coercion<T, U, F>
where
    F : Copy,
{}

impl<T, U : ?Sized, F : FnOnce(*const T) -> *const U> PartialEq
    for Coercion<T, U, F>
where
    F : PartialEq,
{
    fn eq(self: &Self, other: &Self) -> bool {
        match (self, other) {
            (
                Self {
                    coerce: coerce_left,
                    _phantom: _,
                },
                Self {
                    coerce: coerce_right,
                    _phantom: _,
                },
            ) => coerce_left == coerce_right,
        }
    }
}

impl<T, U : ?Sized, F : FnOnce(*const T) -> *const U> Eq
    for Coercion<T, U, F>
where
    F : Eq,
{}

impl<T, U : ?Sized, F : FnOnce(*const T) -> *const U> fmt::Debug
    for Coercion<T, U, F>
where
    F : fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f   .debug_struct("Coercion")
            .field("coerce", &self.coerce)
            .finish()
    }
}

impl<T, U : ?Sized, F : FnOnce(*const T) -> *const U> Hash
    for Coercion<T, U, F>
where
    F : Hash,
{
    fn hash<H : Hasher>(&self, state: &mut H) {
        (&self.coerce).hash(state)
    }
}
