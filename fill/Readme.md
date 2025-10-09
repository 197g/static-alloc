# fill

[![Crates.io Status](https://img.shields.io/crates/v/fill.svg)](https://crates.io/crates/fill)
[![Docs.rs Status](https://docs.rs/fill/badge.svg)](https://docs.rs/fill/)
[![License](https://img.shields.io/badge/license-Zlib-blue.svg)](https://raw.githubusercontent.com/HeroicKatora/static-alloc/master/LICENSE.ZLIB)
[![CI Status](https://api.cirrus-ci.com/github/HeroicKatora/static-alloc.svg)](https://cirrus-ci.com/github/HeroicKatora/static-alloc)

Provides the Fill trait, an alternative to Extend for finite containers.

## Usage

The official recommendation for the `Extend` trait is to simulate pushing all
items from the iterator, panicking if a resource limit is exceeded.  Instead of
looping over all items the implementors of `Fill` should only pull items from
the iterator while space is available. For example, an option can be viewed as
a collection with a capacity of one. One can fill it with the first item of an
iterator if it is empty.

```rust
use fill::Fill;
let mut memory = None;

memory.fill(42..);
assert_eq!(memory, Some(42));
```
