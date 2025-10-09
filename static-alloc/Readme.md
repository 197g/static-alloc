# static-alloc

[![Crates.io Status](https://img.shields.io/crates/v/static-alloc.svg)](https://crates.io/crates/static-alloc)
[![Docs.rs Status](https://docs.rs/static-alloc/badge.svg)](https://docs.rs/static-alloc/)
[![License](https://img.shields.io/badge/license-Zlib-blue.svg)](https://raw.githubusercontent.com/HeroicKatora/static-alloc/master/LICENSE.ZLIB)
[![CI Status](https://api.cirrus-ci.com/github/HeroicKatora/static-alloc.svg)](https://cirrus-ci.com/github/HeroicKatora/static-alloc)

General purpose global allocator(s) with static, inline storage.

## Goal and Target Platform

Provides an allocator for extremely resource constrained environments where the
only memory guaranteed is your program's image in memory as provided by the
loader. Possible use cases are OS-less development, embedded, bootloaders (even
stage0/1).

The primary goals are similar to the standard library simplicity, and
correctness, and minimal assumptions.

## Usage

As a global allocator for `alloc` with some safe allocation extensions:

```rust
use static_alloc::Bump;

#[global_allocator]
static A: Bump<[u8; 1 << 16]> = Bump::uninit();

fn main() {
    // Vec occupying `1 << 7` bytes
    let v = vec![0xdeadbeef_u32; 32];

    // … or allocate values directly.
    let buffer: &mut [u32; 32] = A.leak([0; 32])
        .unwrap();
    buffer.copy_from_slice(&v);
}
```

You can also use it as a local allocator creating dynamic values on the stack.
In this case you might want to be more conservative with resource usage so as
not to blow the stack. The benefit is even larger using it together with
[`without-alloc`] which provides high-level data structures that you are used
to from `alloc`.

```rust
use static_alloc::Bump;

fn main() {
    for _ in 0..100 {
        let local: Bump<[u8; 32]> = Bump::uninit();
        let temp_buffer = local.leak([0; 32]);
	// Resources are cleaned up.
    }
}
```

[`without-alloc`]: https://crates.io/crates/without-alloc/

## Contributing

PRs introducing more tests or documentation are very welcome! Whatever else
submitted should have simplicity and composability in mind, ideas that can not
be put into a draft form are likely too complex or not focussed enough. PRs
should be *extremely* reluctant with introducing new dependencies and *should*
contain no non-optional dependency.

Please open issues with drafts only, feature requests and 'help' issues will be
closed (if you are lucky with a final comment). Stability trumps growth. I
simply can not make any longterm commitment outside my intrinsic motiviation
towards this project. Hence, I favour a highly usable core over a large
interface that is only somewhat usable.

## Additional

This project is licensed under Zlib OR Apache-2.0 OR MIT. You may alternatively
choose [the Unlicense](http://unlicense.org/) instead in which case the
copyright headers signify the parts dedicated to the public domain to the
fullest possible extent instead.
