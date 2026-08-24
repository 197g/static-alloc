use static_alloc::leaked::Alloca;

#[test]
fn alloca_small() {
    let alloc = Alloca::<usize>::new(16).unwrap();
    alloc.run(|slice| {
        assert_eq!(slice.len(), 16);
    });
}
