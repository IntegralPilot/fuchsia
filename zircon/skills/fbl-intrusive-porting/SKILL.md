---
name: fbl-intrusive-porting
description: Guide for porting FBL intrusive containers from C++ to Rust in Fuchsia.
---

# Porting FBL Intrusive Containers to Rust

This skill provides guidelines and patterns for porting C++ intrusive containers
from the `fbl` library to Rust in the Fuchsia project. It captures the lessons
learned during the porting of `SinglyLinkedList`.

## Core Principles

1.  **Memory Layout Parity**: The Rust implementation MUST match the memory
    layout of the C++ version to allow cross-language sharing of containers and
    objects.
2.  **Intrusive Nature**: Objects store their own node state. The container only
    stores pointers to the objects.
3.  **Ownership Flexibility**: Support raw pointers (`*mut T`), unique pointers
    (`UniquePtr<T>`), and ref-counted pointers (`RefPtr<T>`).

## Key Patterns and Solutions

### 1. Solving Aliasing Violations (Interior Mutability)
Intrusive lists manipulate pointers inside shared objects. Creating a mutable
reference (`&mut Node`) to a shared object violates Rust's aliasing rules and is
undefined behavior.
* **Solution**: Use `core::cell::UnsafeCell` for the `next` (and `prev`)
  pointers in the node state. This allows mutating them via shared references
  (`&self`).
* **Result**: Trait methods like `get_node` only need to return `&Node`, not
  `&mut Node`.

### 2. Container-Specific Attribute Names
To avoid conflicts when an object participates in multiple types of containers
(e.g., both a singly and doubly linked list), derive macros should use
container-specific helper attributes.
* **Pattern**: Use `#[sll_node]` for `SinglyLinkedList`, and plan for
  `#[dll_node]` for `DoublyLinkedList`, rather than a generic `#[node]`.

### 3. Safe vs. Unsafe APIs
Prioritize safe APIs for managed pointers and clearly mark unsafe operations for
raw pointers.
* **Pattern**: Use short, clean names for safe operations (e.g., `push_front`
  taking `UniquePtr` or `RefPtr`). Use a `_raw` suffix for unsafe operations
  taking raw pointers (e.g., `push_front_raw`).

### 4. Safe Positional Operations (Cursor API)
Operations at a specific position (like `insert_after` or `erase_next`) are
inherently unsafe if they take raw pointers, as the compiler cannot verify
membership in the container.
* **Solution**: Implement a `CursorMut` API. A cursor borrows the list mutably
  and points to a specific element. Operations on the cursor can be safe because
  the borrow checker ensures list integrity.

### 5. Systematic Testing Across Pointer Types
To ensure all operations work correctly for all supported pointer types, use
declarative macros (`macro_rules!`) to generate a suite of tests for each type.
* **Pattern**: Generate a separate module for each pointer type inside the macro
  to avoid name collisions without external crates.

### 6. Destructor Checks (Address Sensitivity)
Since intrusive lists store pointers to objects, destroying an object while it
is still in a list causes dangling pointers.
* **Solution**: Implement `Drop` for the Node struct with a
  `debug_assert!(!self.in_container())`. This catches premature destruction of
  stack-allocated objects in debug builds.

### 7. Size Tracker Underflow in Iterators
If your container supports constant-time size tracking, and you pass the size to
the iterator to support `ExactSizeIterator`, beware of operations like
`split_after` where a new container is created with an initial size of 0.
* **Pitfall**: Calling `iter().count()` or driving the iterator on such a
  container will cause the iterator's size counter to underflow (panicking in
  debug mode) or yield wrong results if `count()` is optimized to use `len()`.
* **Solution**: For physical counting (like `size_slow`), use a non-tracking
  iterator (e.g., created via `from_element` if available) or a manual
  pointer-walking loop to avoid relying on the iterator's size counter.

### 8. Trait Inheritance for Bounds Simplification
If you have a marker trait (like `SizeTracker`) and all its implementations are
trivial to clone, make the trait inherit from `Clone` (`trait SizeTracker:
Clone`).
* **Benefit**: This eliminates the need for duplicate `impl` blocks or complex
  `where S: SizeTracker + Clone` bounds in the container implementation.

### 9. Encapsulating Unsafe Operations (Node Helpers)
When manipulating raw pointers in intrusive structures, `unsafe` raw pointer
dereferences and cell mutations can quickly clutter mutator methods, making the
code error-prone and difficult to review.
* **Pattern**: Encapsulate all raw pointer dereferencing and `UnsafeCell`
  mutations inside extremely localized **private node helper methods**
  (`get_next`, `set_next`, `get_prev`, `set_prev`) and container-internal
  helpers (`self.get_node_ref`).
* **Benefit**: Container methods (`pop_front`, `pop_back`, Cursors, etc.) can
  then be written cleanly using safe-looking code blocks, vastly improving
  readability and ensuring that raw pointer invariants are isolated and easy to
  audit.

### 10. Bidirectional Language Interop & Lifecycle Management
When sharing intrusive containers across the C++/Rust FFI boundary, both
languages must be able to hold ownership of objects, and memory must always be
reclaimed on the side that allocated it.
* **Pattern**:
  1.  Define shared, multi-list containable C++ objects
      (`SharedUniqueObject`/`SharedRefObject`) in a common C++ header/source
      (`intrusive_container_test_support.h`/`.cc`) containing both singly-linked
      and doubly-linked node states, with C++ `static_assert` size/offset
      matching.
  2.  Match them in a shared Rust test support module
      (`intrusive_container_test_support.rs`) with Rust `static_assert`
      size/offset matching (adjusting sizes from 32 bytes to 48 bytes as list
      node fields are added).
  3.  Track allocation origins using a boolean flag (`allocated_in_rust`). When
      C++ drops/releases a Rust-allocated item, its destructor redirects
      deallocation back to Rust via FFI callbacks
      (`rust_free_shared_unique_object`/`rust_recycle_shared_ref_object`) to
      cleanly free memory using Rust's `Box` allocator.
* **Benefit**: This prevents cross-language memory leaks regardless of which
  language currently owns the list or triggers the deallocation.

### 11. Safe Unit Test Design (Managed vs Raw Pointers)
Unit tests should be as safe and leak-free as possible. However, intrusive
containers present unique borrow checker challenges when using managed pointer
types (`UniquePtr`).
* **Pattern**:
  * **Managed Pointers for General Testing**: Prioritize `UniquePtr` and
    `RefPtr` for standard container mutators and cursor tests. They eliminate
    inline `unsafe` blocks in tests and automatically reclaim memory on drop,
    making memory leaks during test failures impossible.
  * **Raw Pointers for Low-Level Mutations**: For reference-based container
    mutations (such as `list.erase(&obj)` or `list.replace_raw(&obj,
    replacement)`), retrieving an element reference from the list to pass back
    to a list mutating method creates simultaneous mutable and immutable
    borrows, violating the borrow checker. For these specific tests, use
    **stack-allocated** objects with raw unmanaged pointers (`*mut T`). Stack
    allocations naturally protect against memory leaks while bypassing borrow
    checker limitations cleanly.

## Step-by-Step Porting Guide

1.  **Analyze C++ Implementation**: Understand layout, sentinel values, and
    ownership rules.
2.  **Define Node State**: Use `UnsafeCell` for pointers. Apply `#[repr(C)]`.
3.  **Define Traits**: Create `Containable` and `PtrTraits` analogs.
4.  **Implement Container**:
    * Use C++ sentinel value representation.
    * Provide safe wrappers for managed pointers.
    * Provide `_raw` methods for unsafe operations.
5.  **Implement Cursors and Iterators**: Ensure `get_next` and safe removal
    methods are available on cursors.
6.  **Add Macros**: Implement `#[derive(Containable)]` with container-specific
    attributes.
7.  **Test Extensively**: Use the macro-based testing pattern to test across all
    pointer types. Add static asserts for size and alignment.
