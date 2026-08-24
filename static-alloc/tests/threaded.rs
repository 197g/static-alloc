use static_alloc::Bump;
use std::thread;

#[test]
fn each_thread_one() {
    const COUNT: usize = 10;
    // Static but not the global allocator.
    static BUMP: Bump<[u64; COUNT]> = Bump::uninit();

    let threads = (0..COUNT)
        .map(|i| {
            thread::spawn(move || {
                BUMP.leak(i).unwrap();
            })
        })
        .collect::<Vec<_>>();

    threads
        .into_iter()
        .try_for_each(thread::JoinHandle::join)
        .expect("No thread failed to allocate");

    // But now no more left.
    assert!(BUMP.leak(0).is_err());
}
