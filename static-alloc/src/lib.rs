//! General purpose global allocator(s) with static, inline storage.
//!
//! Provides an allocator for extremely resource constrained environments where the only memory
//! guaranteed is your program's image in memory as provided by the loader. Possible use cases are
//! OS-less development, embedded, bootloaders (even stage0/1).
//!
//! The allocator does not support meaningful deallocation but the whole allocator itself can be
//! reset by mutable reference. This is useful for a local, single thread allocator. It's
//! recommended to use the global instance to split resources _once_ at startup and then utilize
//! multiple local allocators for actual working memory. See [`doc`] for some use case studies with
//! examples.
//!
//! ## Usage – Global allocator
//!
//! ```rust
//! use static_alloc::Bump;
//!
//! #[global_allocator]
//! static A: Bump<[u8; 1 << 16]> = Bump::uninit();
//!
//! fn main() {
//!     let v = vec![0xdeadbeef_u32; 128];
//!     println!("{:x?}", v);
//!
//!     let buffer: &'static mut [u32; 128] = A.leak([0; 128])
//!         .unwrap_or_else(|_| panic!("Runtime allocated before main"));
//! }
//! ```
//!
//! You can find more detailed examples in the `doc` module:
//!
//! * [The readme](doc/readme/index.html)
//! * [Usage as a global allocator](doc/global_allocator/index.html)
//! * [Usage as a static allocator](doc/static_allocator/index.html)
//! * [Safe pinning of static tasks](doc/pinned/index.html)
//!
//! ## Why the name?
//!
//! This crates makes it safe to define a *static* object and to then use its memory to *allocate*
//! dynamic values without accidentally exposing or using uninitialized memory. This allows
//! obtaining `&'static mut T` instances which is handy if a struct requires a mutable reference
//! but it is also required that this struct has `'static` lifetime bounds itself.
// Copyright 2019,2022 Aurelia Molzer
#![no_std]
#![deny(missing_docs)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod bump;
pub use bump::Bump;
pub mod leaked;

/// An unsynchronized allocator.
pub mod unsync;

// Can't use the macro-call itself within the `doc` attribute. So force it to eval it as part of
// the macro invocation.
// 
// The inspiration for the macro and implementation is from
// <https://github.com/GuillaumeGomez/doc-comment>
//
// MIT License
//
// Copyright (c) 2018 Guillaume Gomez
#[cfg(doc)]
macro_rules! insert_as_doc {
    { $content:expr, $name:ident } => {
        #[doc = $content] pub mod $name { }
    }
}

/// A module containing extended documentation and examples.
#[cfg(doc)]
pub mod doc {
    // Provides the README.md as doc, to ensure the example works!
    insert_as_doc!(include_str!("../Readme.md"), e0_readme);
    insert_as_doc!(include_str!("doc/global_allocator.md"), e1_global_allocator);
    insert_as_doc!(include_str!("doc/static_allocator.md"), e2_static_allocator);
    insert_as_doc!(include_str!("doc/pinned.md"), e3_pinned);
}
