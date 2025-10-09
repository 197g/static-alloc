Using a Bump as a global allocator.

# Usage – Global allocator

This was the example from the create introduction.

```rust
use static_alloc::Bump;

#[global_allocator]
static A: Bump<[u8; 1 << 16]> = Bump::uninit();

fn main() {
    let v = vec![0xdeadbeef_u32; 128];
    println!("{:x?}", v);

    let buffer: &'static mut [u32; 128] = A.leak([0; 128])
        .unwrap_or_else(|_| panic!("Runtime allocated before main"));
}
```
