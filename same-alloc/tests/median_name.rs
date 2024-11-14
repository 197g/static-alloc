use buffer_vec::{same, BufferVec};

/// Given a list of space separated names, give the 'middle' one if they were sorted.
fn select_median_name(unparsed: &str) -> &str {
    // Problem: This type depends on the lifetime parameter. Ergo, we can not normally store _one_
    // vector in the surrounding function, and instead need to allocate here a new one.
    let mut names: Vec<_> = unparsed.split(' ').collect();
    let idx = names.len() / 2;
    *names.select_nth_unstable(idx).1
}

fn select_median_name_with_buffer<'names>(
    unparsed: &'names str,
    buf: &mut BufferVec<*const str>,
) -> &'names str {
    let mut names = buf.use_for(same::for_ref());
    names.extend(unparsed.split(' '));
    let idx = names.len() / 2;
    *names.select_nth_unstable(idx).1
}

#[test]
fn works() {
    let names = "Adrian Carla Beren Eliza Dala";

    let mut buffer = BufferVec::default();
    assert_eq!(buffer.use_for(same::id()).capacity(), 0);

    assert_eq!(select_median_name(names), select_median_name_with_buffer(names, &mut buffer));
    assert!(buffer.use_for(same::id()).capacity() >= 5);

    // Now this second call does allocate a new buffer with the use of this library.
    assert_eq!(select_median_name(names), select_median_name_with_buffer(names, &mut buffer));
}
