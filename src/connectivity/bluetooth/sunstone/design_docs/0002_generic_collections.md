---
title: Sunstone generic collections
description: >
    Defines the underlying mechanism for managing heap and heap-less collections
    in a platform-agnostic manner
status: approved # [approved | declined | implemented | superseded]
authors:
  - asnaider@google.com
tags:
  - alloc
  - collections
bugs:
  - 520452026
rfc: 0002
---

# Objective

The Sapphire team is developing a platform-agnostic Bluetooth stack in Rust that
must run on both resource-rich systems and resource-constrained microcontrollers
(MCUs). This requirement introduces significant challenges for dynamic memory
management.

# Summary

This document proposes a dynamic memory management solution designed to support
a platform-agnostic Bluetooth stack.

# Background

Dynamic memory management is common in software applications, including the
original Sapphire C++ stack. However, it presents significant challenges for
embedded environments where satisfying allocation requests cannot be guaranteed.
Furthermore, unbounded memory allocations (like those in `std::vector`) can lead
to attacker-controlled Out-of-Memory (OOM) vulnerabilities, which affect both
microcontrollers and larger operating systems like Fuchsia.

Key issues associated with dynamic memory allocations include:

-   **OOM Vulnerabilities:** Susceptibility to denial-of-service via memory
    exhaustion.
-   **Memory Leaks:** Unintentional exhaustion of available memory over time.
-   **Embedded Constraints:** Difficulty in guaranteeing allocations on
    resource-constrained devices.
-   **Latency:** Performance overhead from heap allocation, which is problematic
    for latency-sensitive tasks.

Eliminating all dynamic memory is not a goal of this design. Instead, this
document outlines how to constrain heap usage, ensuring it is only used when
necessary.

# Requirements

-   **Platform-Agnostic:** The solution must function correctly on both Fuchsia
    and Pigweed.
-   **Data Structures:** The solution must support standard Bluetooth stack data
    structures (e.g., `Vec`, `Deque`, `HashMap`, `OrderedMap`).
-   **Fallible Allocations:** APIs must support fallible operations, allowing
    the stack to handle allocation failures gracefully without triggering OOM
    panics.

# Non-requirements

-   **Pointer Types:** Smart pointers (e.g., `Box`, `Rc`) are conceptually
    distinct from collection backing-stores and are excluded from this design.

# Design

The primary differentiator from the Rust standard library is the requirement for
**fallible allocation**. Standard library collections assume allocations are
infallible. This assumption is unacceptable in embedded environments or in
systems where untrusted peers can indirectly trigger allocations. Introducing
fallibility into the collections API has significant downstream effects, and
retrofitting it onto code that assumes infallibility is notoriously difficult.

## Storage

A proposed update to the Rust nightly allocator introduces a
[`Storage`-based API](https://docs.rs/storage_api/latest/storage_api/). The
`Storage` API generalizes the nightly `Allocator` API to support inline
allocations by returning an opaque handle instead of a pointer. This handle must
be resolved to a pointer before use.

Backing collections with `Storage` instead of `Allocator` allows them to support
heap, stack, or static allocation.

The storage API is encapsulated in the `Storage` trait:

```rust

/// The trait for allocating memory in a storage
///
/// # Safety
///
/// - [`Storage::resolve`] must return a valid pointer to the allocation when passed a valid
///   [`Storage::Handle`]
pub unsafe trait Storage {
    type Handle: Copy;

    /// Returns a pointer to the allocation represented by `handle`
    ///
    /// # Safety
    ///
    /// - `handle` must be valid
    unsafe fn resolve(&self, handle: Self::Handle) -> NonNull<()>;

    /// Allocates memory with a layout specified by `layout`
    ///
    /// Also returns the total amount of bytes actually allocated, which may be more than requested by `layout`
    fn allocate(&mut self, layout: Layout) -> Result<(Self::Handle, usize), AllocError>;

    /// Deallocates (and invalidates) a [`StorageHandle`] that was allocated with this [`Storage`]
    ///
    /// # Safety
    ///
    /// - `layout` must be the same layout that was used to allocate it,
    ///   though the size may by greater as long as its less than the available capacity returned by any of the allocation methods ([`Storage::allocate`]/[`Storage::grow`]/[`Storage::shrink`])
    /// - `handle` must be valid
    unsafe fn deallocate(&mut self, layout: Layout, handle: Self::Handle);

    /// Grows (increases the size of) an allocation
    ///
    /// Similar to [`Storage::allocate`] this method also returns the number of bytes actually allocated, which may be more than requested with `new_layout`
    ///
    /// # Safety
    ///
    /// - `new_layout.size() >= old_layout.size()`
    /// - `handle` must be valid
    /// - if this method succeeds, `handle` is now invalid and cannot be used
    unsafe fn grow(
        &mut self,
        old_layout: Layout,
        new_layout: Layout,
        handle: Self::Handle,
    ) -> Result<(Self::Handle, usize), AllocError>;

    /// Shrinks (decreases the size of) an allocation
    ///
    /// Similar to [`Storage::allocate`] this method also returns the number of bytes actually allocated, which may be more than requested with `new_layout`
    ///
    /// # Safety
    ///
    /// - `new_layout.size() <= old_layout.size()`
    /// - `handle` must be valid
    /// - if this method succeeds, `handle` is now invalid and cannot be used
    unsafe fn shrink(
        &mut self,
        old_layout: Layout,
        new_layout: Layout,
        handle: Self::Handle,
    ) -> Result<(Self::Handle, usize), AllocError>;
}
```

We use a "family trait" pattern to decouple the element type from the storage
mechanics. This allows defining generic storage backings (such as
`ArrayStorage<N>`) independently of the element type.

```rust
/// Family trait to consolidate storage families around the type `T` that it's used for.
pub trait StorageFamily {
    /// The storage type parameterized over a generic `T`
    type Storage<T>: Storage;
}

// Blanket `impl` for anything that is already a storage itself
impl<S: Storage> StorageFamily for S {
    type Storage<T> = Self;
}

```

> ***Note:*** This "Family" trait pattern emulates Higher-Kinded Types (HKTs) in
> Rust. While verbose to define, it simplifies usage.

## Contiguous Buffers

Designing for fallibility enables the use of bounded buffers. Many common
collections (`Vec`, `Deque`, `IndexMap`, `Heap`, `LruCache`) can be backed by a
contiguous, potentially growable buffer. Fallible APIs also allow using
non-growable buffers as backing storage.

Contiguous buffers often outperform fragmented memory alternatives due to better
data locality, which improves CPU cache utilization.

> For example, `hashbrown` (the Rust port of Google's SwissTable) shows
> significant performance gains by using a data-oriented design.

### `struct RawVec<T, A: StorageFamily>`

We implement a [`RawVec`](https://doc.rust-lang.org/nomicon/vec/vec-raw.html)
similar to the standard library's internal `RawVec` to manage buffer allocation,
growing, and shrinking. It represents a `[MaybeUninit<T>]` with a capacity but
no length.

All contiguous collections can be built on top of `RawVec`.

```rust
impl<A: StorageFamily, T> RawVec<T, A> {
    /// Default-constructs an empty `RawVec`.
    pub fn new() -> Self
    where
        Self: Default;

    /// Creates the `RawVec` in the given allocator.
    pub fn new_in(allocator: A::Storage<T>) -> Self;
    /// Returns the total capacity of the buffer.
    pub fn capacity(&self) -> usize;

    /// Attempts to grow the buffer
    ///
    /// If successful, all of the data in the original buffer will be copied over
    /// to the beginning of the new buffer.
    ///
    /// Returns `Err(AllocError)` if the allocation fails.
    pub fn grow(&mut self) -> Result<(), AllocError>;

    pub fn as_ptr(&self) -> NonNull<[MaybeUninit<T>]>;

    pub fn as_ptr_mut(&mut self) -> NonNull<[MaybeUninit<T>]>;

    /// Returns a shared reference to the underlying buffer.
    pub fn buffer(&self) -> &[MaybeUninit<T>];

    /// Returns a mutable reference to the underlying buffer.
    pub fn buffer_mut(&mut self) -> &mut [MaybeUninit<T>];
}

impl<T, A: StorageFamily> Default for RawVec<T, A>
where
    A::Storage<T>: Default,
{
    ...
}

// Frees the underlying buffer
impl<T, A: StorageFamily> Drop for RawVec<T, A> {
    ...
}
```

## Vec

```rust

impl<T, A: StorageFamily> Vec<T, A> {
    /// Creates a new, empty vector.
    pub fn new() -> Self
    where
        Self: Default;

    /// Creates a new, empty vector with the given allocator.
    pub fn new_in(allocator: A::Storage<T>) -> Self;

    /// Returns the total capacity of the underlying buffer.
    pub fn capacity(&self) -> usize;

    /// Attempts to push a value to the back of the vector.
    ///
    /// Returns `Err(value)` if the buffer is full and cannot be grown.
    pub fn push(&mut self, value: T) -> Result<(), T>;
    /// Removes and returns the last element of the vector, if any.
    pub fn pop(&mut self) -> Option<T>;

    /// Removes and returns the element at position `index` within the vector,
    /// shifting all elements after it to the left.
    ///
    /// # Panics
    /// Panics if `index` is out of bounds.
    pub fn remove(&mut self, index: usize) -> T;
    /// Returns the number of elements currently in the vector.
    pub fn len(&self) -> usize;
}

impl Deref for Vec<T, A> {
    type Target = [T];
}
impl DerefMut for Vec<T, A> {}
```

## Deque

A Double-Ended Queue (`Deque`) can function as both a stack and a queue,
offering significant versatility.

```rust
impl<T, A: StorageFamily> Deque<T, A> {
    /// Creates a new, empty `Deque` using the default buffer initialization.
    pub fn new() -> Self
    where
        Self: Default;

    /// Returns the number of elements currently stored in the queue.
    pub fn len(&self) -> usize;

    /// Returns the number of elements currently stored in the queue.
    pub fn capacity(&self) -> usize;

    /// Returns `true` if the queue contains no elements.
    pub fn is_empty(&self) -> bool;

    /// Pushes an element to the front of the queue.
    ///
    /// Returns `Err(value)` if the buffer is at full capacity.
    pub fn push_front(&mut self, value: T) -> Result<(), T>;

    /// Pushes an element to the back of the queue.
    ///
    /// Returns `Err(value)` if the buffer is at full capacity.
    pub fn push_back(&mut self, value: T) -> Result<(), T>;

    /// Pushes an element to the front of the queue. If the queue is full,
    /// the rearmost element is overwritten and dropped.
    pub fn force_push_front(&mut self, value: T);

    /// Pushes an element to the back of the queue. If the queue is full,
    /// the frontmost element is overwritten and dropped.
    pub fn force_push_back(&mut self, value: T);

    /// Removes and returns the element at the front of the queue, if any.
    pub fn pop_front(&mut self) -> Option<T>;

    /// Removes and returns the element at the back of the queue, if any.
    pub fn pop_back(&mut self) -> Option<T>;

    /// Attempts to grow the underlying storage, doubling its capacity.
    ///
    /// If wrapped around, shifts the front segment to the new space to maintain contiguity.
    pub fn grow(&mut self) -> Result<(), crate::AllocError>;

    /// Returns a shared reference to the element at the front of the queue, if any.
    pub fn peek_front(&self) -> Option<&T>;

    /// Returns a shared reference to the element at the back of the queue, if any.
    pub fn peek_back(&self) -> Option<&T>;

    /// Returns a mutable reference to the element at the front of the queue, if any.
    pub fn peek_front_mut(&mut self) -> Option<&mut T>;

    /// Returns a mutable reference to the element at the back of the queue, if any.
    pub fn peek_back_mut(&mut self) -> Option<&mut T>;

    /// Returns a reference to the element at the logical `index` (0 is oldest).
    pub fn get(&self, index: usize) -> Option<&T>;

    /// Returns a reference to the element at the logical `index` (0 is oldest).
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T>;

    /// Clears the queue, removing and dropping all elements.
    pub fn clear(&mut self);

    /// Removes and returns the element at the front of the queue only if it satisfies `predicate`.
    pub fn pop_front_if<F>(&mut self, predicate: F) -> Option<T>
    where
        F: FnOnce(&T) -> bool;

    /// Removes and returns the element at the back of the queue only if it satisfies `predicate`.
    pub fn pop_back_if<F>(&mut self, predicate: F) -> Option<T>
    where
        F: FnOnce(&T) -> bool;

    /// Returns an iterator yielding shared references to the elements of the queue in FIFO order.
    pub fn iter(&self) -> impl Iterator<Item = &T>;

    /// Returns an iterator yielding mutable references to the elements of the queue in FIFO order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T>;
}
```

## Maps

Unordered maps can be implemented as index maps utilizing two underlying buffers
(one for indices, one for items). The API follows the same pattern as other
collections, parameterized over `A: StorageFamily`, `K: Hash`, and `V`. Notably,
map insertion operations will be fallible.

Due to the potential complexity of handling fallible map insertions, we should
evaluate the use of bounded-buffer maps on a case-by-case basis.

## LRU Cache

A Least Recently Used (LRU) cache is a strong candidate for a bounded map. This
can be implemented by combining an `IndexMap` with an index-based linked list to
track and reclaim the least recently used elements.

## Heap

Min/Max heaps are typically implemented as binary trees laid out in a contiguous
buffer. Rust's `BinaryHeap` and C++'s `std::priority_queue` follow this
approach.

# Testing

While custom collections introduce maintenance overhead, they can be rigorously
verified.

In addition to standard unit tests, we can implement property-based testing
using Rust standard library collections as reference oracles. We will also run
all tests under `miri` to validate the safety of any `unsafe` code.

# Resource Constraints & Requirements

The core design principle is to delegate resource management policy to the API
consumer. This section details how the design addresses:

-   Runtime heap allocations
-   Stack usage
-   Code size
-   Multi-threading

## Heap Usage: Runtime Allocations

This design does not mandate heap allocations; collections can be backed by
stack or static memory. However, it does not prohibit them. The goal is to allow
consumers to choose the allocation strategy that best fits their target
environment.

## Stack Usage

Unlike the `heapless` crate, this design remains agnostic to the underlying
storage allocation. While stack-allocated backing is supported, it is not
enforced.

Developers can select from different allocation schemes based on their specific
trade-offs (e.g., convenience, predictability, footprint).

## Code Size

Code size is evaluated in terms of data footprint (static allocations) and
instruction size.

### Data Footprint

The choice to allocate collections in static memory is delegated to the
consumer. Static storage is supported but not required.

### Instruction Size (Monomorphization)

A key concern is code size expansion due to Rust's monomorphization of generic
types. Because these collections are generic over their backing storage, each
unique combination of element type and storage type generates separate machine
code.

To mitigate this:

-   We can avoid `heapless`-style designs that parameterize collection size in
    the type definition. Instead, we can use runtime-sized slices (backed by
    static or heap memory) to share implementations across different capacities.
-   We can structure the implementation to delegate to non-generic internal
    helper functions (type erasure) where possible.

# Alternatives

## The `heapless` Crate

We could use the existing `heapless` crate. However, `heapless` collections
parameterize capacity in their type signature, which limits flexibility and can
significantly increase code size due to monomorphization.

## Standard Library `Allocator` API

We could leverage the experimental unstable Rust `Allocator` API (which aims to
support custom allocators for standard types like `Box` and `Vec`).

However, this approach still requires pointer indirection and does not easily
support inline, stack-allocated storage without heap-like management.
Additionally, our proposed `Storage` abstraction can be implemented on top of
the `Allocator` API once it stabilizes.

# Implementation Plan

## Files to Add

We will introduce a new crate, `sapphire-collections`, with the following
structure:

-   `sapphire-collections/src/lib.rs`
-   `sapphire-collections/src/storage/storages.rs`
-   `sapphire-collections/src/storage/storages/inline.rs`
-   `sapphire-collections/src/storage/storages/allocated.rs`
-   `sapphire-collections/src/vec.rs`
-   `sapphire-collections/src/vec/raw_vec.rs`
-   `sapphire-collections/src/deque.rs`
-   `sapphire-collections/src/storage.rs`
-   Other collection implementations as needed.

# Documentation & Examples

All APIs will include documentation comments and `doctests` to provide verified
usage examples.

# Future Work

*   **Ordered Maps:** Investigating efficient layouts for ordered maps, which do
    not map naturally to simple contiguous buffers.
*   **Smart Pointer Alternatives:** Designing platform-agnostic smart pointers
    (like `Box` or `Rc`). These are excluded from this document as they
    primarily address lifetime erasure rather than collection storage layout.
