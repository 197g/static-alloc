The `SameVec` makes it possible to re-use allocations across multiple
invocations of zero-copy parsers.

This crate provides an allocated buffer that can be used by vectors of
different element types, as long as they have the same layout. Most prominently
this allows use of one buffer where the element type depends on a function
local lifetime. The required vector type would be impossible to name outside
the function.

```rust
fn select_median_name(unparsed: &str) -> &str {
    // Problem: This type depends on the lifetime parameter. Ergo, we can not normally store _one_
    // vector in the surrounding function, and instead need to allocate here a new one.
    let mut names: Vec<_> = unparsed.split(' ').collect();
    let idx = names.len() / 2;
    *names.select_nth_unstable(idx).1
}

fn select_median_name_with_buffer<'names>(
    unparsed: &'names str,
    buf: &mut SameVec<*const str>,
) -> &'names str {
    let mut names = buf.use_for(same::for_ref());
    names.extend(unparsed.split(' '));
    let idx = names.len() / 2;
    *names.select_nth_unstable(idx).1
}
```

# License

This project is licensed under Zlib OR Apache-2.0 OR MIT. You may alternatively
choose [the Unlicense](http://unlicense.org/) instead in which case the
copyright headers signify the parts dedicated to the public domain to the
fullest possible extent instead.
