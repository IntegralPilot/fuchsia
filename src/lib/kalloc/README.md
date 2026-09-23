# kalloc - Fallible Allocations for Zircon

`kalloc` is a minimal Rust crate that provides safe, fallible heap allocation
for code running in the Zircon kernel or shared between userspace and the
kernel.

## Why `kalloc`?

In standard Rust, when you create a heap-allocated object like a `Box`, the
runtime assumes that memory allocation will always succeed. If the system runs
out of memory, the program will immediately crash (panic).

While this behavior is acceptable for most userspace applications, it is
**unacceptable in an operating system kernel**. A kernel must be resilient and
handle out-of-memory conditions gracefully without crashing the entire system.

`kalloc` provides an analog to the standard Rust `alloc` crate interface but
guarantees that all allocations are explicit and fallible.

## Key Features

### Fallible `alloc` and `dealloc`

The primary functions provided by this crate are `kalloc::alloc` and
`kalloc::dealloc`. These functions match the signatures of the standard `alloc`
crate's functions, but they return an `Option<NonNull<u8>>` to indicate success
or failure.

```rust
use core::alloc::Layout;
use kalloc::{alloc, dealloc};

unsafe {
    let layout = Layout::from_size_align(1024, 8).unwrap();
    if let Some(ptr) = alloc(layout) {
        // Use the allocated memory...
        // ...

        // Deallocate when done
        dealloc(ptr.as_ptr(), layout);
    } else {
        // Handle allocation failure!
    }
}
```

### Fallible `Box`

`kalloc` provides a custom `Box` type that supports fallible allocation for
both sized types and slices.

```rust
use kalloc::boxed::Box;

// Sized type
if let Ok(b) = Box::<u32>::try_new(42) {
    assert_eq!(*b, 42);
}

// Slice
if let Ok(b) = Box::<[u32]>::try_new_uninit_slice(10) {
    assert_eq!(b.len(), 10);
}
```

### `Allocator` Trait

The crate defines an `Allocator` trait that allows customization of the
allocation strategy for collections like `Box`. A `DefaultAllocator` is
provided that uses `kalloc::alloc` and `kalloc::dealloc`.

### Dual Environment Support

`kalloc` is designed to work in two different environments:

1.  **In the Kernel**: When compiled for the Zircon kernel (`is_kernel` is
    true), it directly invokes the kernel's C `malloc` and `free` functions via
    FFI.
2.  **In Userspace/Tests**: When compiled for userspace or unit tests, it falls
    back to using the standard Rust allocator, allowing tests to run on the
    host machine or in emulators easily.
