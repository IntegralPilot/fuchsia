# Packed Collections

A Rust library providing memory-efficient collections optimized for dynamically
sized types (specifically `[u8]` and `str`) stored in a single contiguous
buffer.

## Overview

In Rust, storing dynamically sized types (DSTs) like strings or slices in
standard collections typically requires indirection, such as `Vec<String>` or
`Vec<Box<[u8]>>`. This introduces:

1. **Memory Overhead**: Each item requires a pointer and allocator metadata.
2. **Memory Fragmentation**: Items are scattered in the heap, hurting cache
   locality.

The `packed` library solves this by storing all elements sequentially in a
single contiguous `Vec<u8>` buffer and maintaining a separate `Vec<usize>` of
offsets. This eliminates the per-element allocator overhead and ensures all data
is local in memory.

## Collections

### `PackedVec<T>`

A vector-like structure that stores elements of type `T` (where `T` is `[u8]` or
`str`) contiguously.

- **O(1)** random access by index.
- **O(log N)** binary search support when items are sorted.
- **Append-only**: Supports `push` but not insertion or deletion in the middle.

### `PackedMap<K, V>`

A map with keys stored in a sorted `PackedVec<K>` and values in a standard
`Vec<V>`.
- **O(log N)** lookups via binary search.
- **Immutable Keys**: The map structure is rigid after creation. You can update
  values of existing keys, but you cannot insert new keys that are out of order.

### `PackedMapBuilder<K, V>`

A builder for `PackedMap` that allows incrementally appending potentially
out-of-order key-value pairs. It optimizes for mostly-sorted inputs while
supporting full out-of-order insertion.

- **O(log N)** inserts and lookups via binary search.

## When to Use

- **Memory-Constrained Environments**: When you need to store millions of small
  strings or byte slices and cannot afford the overhead of `Box` or `String` for
  each.

- **Read-Heavy Workloads**: When the collection is built once or rarely and read
  frequently. Contiguous storage provides excellent cache locality compared to
  pointer-based map structures like `BTreeMap`.

- **Hot Loops / Cache Locality**: When performing frequent lookups in a
  performance-critical loop. Since all keys are stored contiguously, it provides
  much better cache locality than pointer-based map structures like `BTreeMap`.

- **Streaming Data**: When you are streaming data (e.g., from a file or network)
  and want to avoid allocation overhead during construction. You can read data
  into a single reusable buffer and push references to it into `PackedVec` or
  `PackedMapBuilder`. The collection copies the data into its internal
  contiguous buffer, avoiding the need to allocate a new `String` or `Box` for
  every item processed.

- **Long-Lived Structures**: When the data structure is kept around for a while,
  allowing you to amortize the initial construction cost and benefit from
  reduced memory usage over time.

- **Ordered Data**: When you need a map that maintains key order or supports
  range queries (via `range` and `range_mut`).

- **Mostly Sorted Input**: If you are building a map from data that is already
  sorted or mostly sorted, `PackedMapBuilder` will be very fast.

## When NOT to Use

- **Arbitrary Types**: The `PackedItem` trait is sealed and limited to `[u8]`
  and `str`. You cannot use this for custom types without modifying the library.

- **Frequent Mutations**: If you need a map that supports frequent insertions
  and deletions across the entire key range, use `std::collections::HashMap` or
  `BTreeMap`. `PackedMap` does not support arbitrary insertions.

- **Already Allocated Data**: If your data is already stored in a collection of
  heap-allocated objects (e.g., `BTreeMap<String, V>`), the memory savings of
  converting to a packed structure may not justify the conversion cost unless
  the structure is long-lived or cache locality in hot loops is critical. The
  conversion will also temporarily require double memory.

- **Unsorted Bulk Inserts without Builder**: If you try to use
  `PackedMap::insert` directly with unsorted data, it will fail if the key is
  out of order. You must use `PackedMapBuilder`.
