Safely pinning a statically allocated task.

# Usage — Pinned tasks

Another use case similar to a pool is the ability to _pin_ objects that have
been allocated without the need for macros or `unsafe`. This is sound because
the memory within a static bump allocator is _never_ reused for any other
purpose.

Let's set the scene by defining a long running task that requires pinning.

```
use static_alloc::Bump;

async fn example(x: usize) -> usize {
    // Holding reference across yield point.
    // This requires pinning to run this future.
    let y = &x;
    core::future::pending::<()>().await;
    *y
}
```

Note that the type of this asynchronous future, the state machine synthesized
as the result type of the function `example`, is self-referential because it
holds a reference to `x`, assigned to `y`, at an await point. This means that
this type is also `!Unpin`. Once it is polled at a particular location in
memory it mustn't move. This is enforced by requiring you to _pin_ it before
polling the future, a kind of state that ensures the memory used for
representing the type can not be reused until _after_ its destructor (`Drop`)
was called.

Ordinarily there are several `unsafe` constructors for `Pin<&mut _>` and a few
helper macros that, in one way or another, ensure you can't access it by value
after pinning it, and that you can't `forget` it either. There is also an
entirely safe method available only when a global allocator exists: A value
within a `Box<_>` can be pinned at will by calling `Box::into_pin`. This is
safe because the memory can not be reused before it is deallocated, which can
only happen by dropping the box itself which necessarily drops the value as
well.

Since a static `Bump` is _also_ an allocator, an argument similar to a globally
allocated `Box` holds for values put into its memory! If ensure that the bump
allocator was borrowed forever, that is for the `'static` lifetime, then it can
not be reset (because this requires `&mut`/unique access) and no memory is ever
reused. This allows using a pool _instead_ of the global allocator. This is
particularly interesting for tasks that will run _exactly_ once or a bounded
number of times. In this case there is no risk that this will eventually
exhaust the allocator's memory because memory is never returned.

## Pinning an allocated task

It is time to demonstrate this in code:

```
use core::pin::Pin;
use static_alloc::{Bump, leaked::LeakBox};
# async fn example(x: usize) -> usize { todo!() }

// A generous size estimation.
// See below for a genius exact idea.
// On nightly you could calculate the size as a constant.
static SLOT: Bump<[usize; 4]> = Bump::uninit();

let future: LeakBox<'static, _> = SLOT.leak_box(example(0))
  .expect("Our size estimation was generous enough");
let mut future: Pin<_> = LeakBox::into_pin(future);

let can_use_this_in_async = async move {
  let _ = future.as_mut().await;
};
```

Let me repeat how that this will of course only work the first time, after that
the bump allocator might be exhausted. You can also _only_ pin values that were
leak-boxed on a `static` pool of memory, since this is the required assurance
that the memory is never reused. This means that we will mostly want to use
this for creating an instance of a single global task.

Nevertheless, the code above guarantee that the task is properly `Drop`'d in
benign circumstances. This is in contrast to _actually_ leaking the value where
it would never be dropped. It's _sound_ not to forget a pinned future
constructed in this way but it it shouldn't happen by accident in decently
well-written code.

## Calculate the required allocation for a future's state machine

The following idea by [@Yandros](https://github.com/danielhenrymantilla)
calculates the layout of a future. We can then create a memory reservation
large enough to guarantee that one of that future can be allocated.

```
use core::{alloc::Layout, marker::PhantomData};
use static_alloc::Bump;

struct Helper<F>(PhantomData<F>);

impl<F> Helper<F> {
  const fn layout(self: Helper<F>) -> Layout {
    Layout::new::<F>()
  }

  fn use_the_type_inference_luke(self: &'_ Self, _: &'_ F)
    {}
}

async fn test() {}

const LAYOUT_OF_TEST: Layout = {
    let h = Helper(PhantomData);
    let _ = || {
        h.use_the_type_inference_luke(&test())
    };
    h.layout()
};

// Number of bytes to guarantee one address is aligned.
const REQUIRED_TO_GUARANTEE: usize = LAYOUT_OF_TEST.size() + LAYOUT_OF_TEST.align();
static SLOT: Bump<[u8; REQUIRED_TO_GUARANTEE]> = Bump::uninit();

SLOT.leak_box(test())
  .expect("This _must_ work the first time it is called");
```
