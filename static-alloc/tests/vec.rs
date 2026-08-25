use static_alloc::Bump;

#[global_allocator]
static A: Bump<[u8; 1 << 20]> = Bump::uninit();

#[test]
fn ok_vec() {
    let v = vec![0xdeadbeef_u32; 128];
    println!("{:x?}", v);
    v.into_iter().for_each(|x| assert_eq!(x, 0xdeadbeef_u32));
}

#[test]
fn shrink() {
    let mut v = vec![0xdeadbeef_u32; 2];
    v.pop();
    v.shrink_to_fit();
    assert_eq!(v.capacity(), 1);
    assert_eq!(&v, &[0xdeadbeef_u32; 1]);
}

#[test]
fn grow() {
    let mut v = vec![0xdeadbeef_u32; 2];
    assert_eq!(v.capacity(), 2);
    v.push(0xdeadbeef_u32);
    assert!(v.capacity() > 3);
    assert_eq!(&v, &[0xdeadbeef_u32; 3]);
}
