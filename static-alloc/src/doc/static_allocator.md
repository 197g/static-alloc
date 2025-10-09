Using a Bump as a dedicated static pool.

# Usage – Static allocator

Some tasks will be created once during program startup but are not constant expression. The
bump allocator allows you to provision static storage for them and then initialize them
dynamically. In this way, the caller can also retain precise control over memory locality.

Consider a program that runs a global system, this might be an event loop for tasks, but it has
two flavors of implementing this system. One of them might perform energy hungry but fast
polling while a more efficient system uses interrupts. You want to let the program's runtime
environment decide which of these systems should be used so they implement a common trait and
your program passes around a static `dyn`-reference.

```rust
# #[derive(Default, Debug)]
struct TaskSystemA {
    // …
# _field: u8,
}

# #[derive(Default, Debug)]
struct TaskSystemB {
    // …
# _field: u8,
}

trait CommonTrait {
    // …
}
```

Using a Bump allocator as a pool solves this problem very conveniently without the need for a
global allocator. Compared to using the stack, your reference retain the `'static` lifetime
while compared to a once-cell you retain the uniqueness of the original reference.

```rust
# #[derive(Default, Debug)]
# struct TaskSystemA(u8);
# #[derive(Default, Debug)]
# struct TaskSystemB(u16);
# trait CommonTrait {}
# impl CommonTrait for TaskSystemA {}
# impl CommonTrait for TaskSystemB {}
# use core::mem::ManuallyDrop;
use static_alloc::Bump;

union StorageForAOrB {
    variant_a: ManuallyDrop<TaskSystemA>,
    variant_b: ManuallyDrop<TaskSystemB>,
}

static POOL: Bump<StorageForAOrB> = Bump::uninit();

fn main() {
    // Split the pool based on an environment variable.
    let use_b = std::env::var_os("USE_VARIANT_B")
        .map_or(false, |_| true);

    // If we had stack-allocate them we wouldn't have `'static`.
    let used: &'static mut dyn CommonTrait = if use_b {
        POOL.leak(TaskSystemA::default()).unwrap()
    } else {
        POOL.leak(TaskSystemB::default()).unwrap()
    };
}
```
