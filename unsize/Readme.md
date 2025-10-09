# unsize

[![Crates.io Status](https://img.shields.io/crates/v/unsize.svg)](https://crates.io/crates/static-alloc)
[![Docs.rs Status](https://docs.rs/unsize/badge.svg)](https://docs.rs/unsize/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://raw.githubusercontent.com/HeroicKatora/static-alloc/LICENSE)
[![CI Status](https://api.cirrus-ci.com/github/HeroicKatora/static-alloc.svg)](https://cirrus-ci.com/github/HeroicKatora/static-alloc)

## Goals and Targets

In embedded environments there are quite many use cases for custom smart
pointers, for example to encapsulate ownership of objects on the stack or
within a custom arena etc. One also wants to avoid unnecessary monomorphization
that is for code size it is often desirable to use a dynamic trait object as
indirection instead of instantiating a generic for a great number of type
parameters. However, these can not be allocated (or often even constructed)
directly. The standard library `Box` usually provides a sort of coercion: You
can convert `Box<[T; N]>` to `Box<[T]>` for all array sizes or `Box<u32>` to
`Box<dyn Any>`.

However, this does not work for custom smart pointers. The conversion is based
on a nightly-only trait that one needs to explicitly opt-in to. This crate
provides an alternative that works with `no_std` environments, for example to
enable custom `Box` designs that avoid any dependency on a global allocator, or
an arena based reference counted pointer, etc. For these use cases it is
intended to replace the nightly unsizing mechanism with a stable and safe
solution.

## Usage

As a library developer you can implement the `CoerciblePtr` trait for your
smart pointer type. This enables the use of all coercions with this pointer
type. To provide custom unsize coercions to your own `?Sized` wrapper type you
can provide a safe constructor for `Coercion`.

As a user of a `unsize`-enabled pointer type, you should import the
`CoerceUnsized` extension trait and the `Coercion` marker type. Then create an
instance of `Coercion` to 'coerce' the pointee type of a smart pointer. The
type defines a number of safe constructors and an `unsafe` escape hatch. The
crate further defines a macro that provides safe coercion to other unsized
types, if it compiles.

## Additional

This project is licensed under Zlib OR Apache-2.0 OR MIT. You may alternatively
choose the Unlicense instead in which case the copyright headers signify the
parts dedicated to the public domain to the fullest possible extent instead.
