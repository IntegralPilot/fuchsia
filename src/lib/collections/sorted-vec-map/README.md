# sorted-vec-map

A memory-efficient collection library providing ordered maps and sets built on contiguous vectors.

## Overview

`SortedVecMap` and `SortedVecSet` are alternatives to standard `BTreeMap` and `BTreeSet` designed
for scenarios where memory footprint and cache locality are critical, and data is rarely modified
after creation. You should always benchmark since the type of the keys and values may change the
performance characteristics.

Instead of allocating separate nodes with pointers (as a tree does), these collections store all
entries in a single contiguous `Vec`. Lookups use binary search, taking `O(log N)` time, while
benefiting significantly from CPU cache hits due to contiguous memory layout.

## When to Use

* **Read-Heavy Workloads**: Lookups are `O(log N)` but highly cache-friendly, often outperforming
  tree-based collections for smaller to medium-sized datasets.
* **Memory-Constrained Environments**: Eliminates the per-node pointer overhead of tree
  structures.
* **Static or Rarely Mutated Data**: Ideal for lookup tables, configuration data, or indices that
  are built once and queried frequently.
* **Batch Construction**: Highly efficient to build using the `Builder` types when data can be
  inserted in sorted order or built all at once.

## When NOT to Use

*   **Frequent Mutations**: Single insertions and removals require shifting elements, taking linear
    `O(N)` time. If your workload involves frequent incremental updates, use `BTreeMap` or
    `HashMap`.
*   **Extremely Large Mutable Collections**: The `O(N)` cost of single insertions becomes
    prohibitive as the collection grows large.

## Complexity Guarantees

| Operation | `SortedVecMap` / `SortedVecSet` |
|---|---|
| Lookup (`get`/`contains`) | `O(log N)` |
| Insertion (`insert`) | `O(N)` |
| Removal (`remove`) | `O(N)` |
| Iteration (`iter`) | `O(1)` per step |
| Batch Build (Sorted) | `O(N)` |
| Batch Build (Unsorted) | `O(N log N)` |

## Examples

### SortedVecMap

```rust
use sorted_vec_map::SortedVecMap;

// Efficient batch construction
let map = SortedVecMap::from([
    ("apple", 1),
    ("banana", 2),
    ("cherry", 3),
]);

assert_eq!(map.get("banana"), Some(&2));
```

### SortedVecSet

```rust
use sorted_vec_map::SortedVecSet;

let mut set = SortedVecSet::new();
set.insert(1);
set.insert(2);

assert!(set.contains(&1));
```

## See Also

* **`PackedMap`**: If your key type is a dynamically sized type (DST) such as `str` or `[u8]`,
  consider using `PackedMap` from the `packed` collection library. It packs all keys into a single
  contiguous byte buffer, eliminating the pointer/length overhead of individual keys and offering even
  better memory compression.
