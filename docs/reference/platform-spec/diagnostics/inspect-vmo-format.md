# Inspect VMO file format

[TOC]

This document describes the **Component Inspection File Format** (Inspect Format).

Files formatted using the Inspect Format are known as **Inspect Files**,
which commonly have a `.inspect` file extension.

For information on how to change the format. Please see
[Extending the Inspect file format][updating-format]

# Overview

[Component Inspection][inspect] provides components with the ability to
expose structured, hierarchical information about their state at runtime.

Components host a mapped Virtual Memory Object ([VMO]) using the Inspect
Format to expose an **Inspect Hierarchy** containing this internal state.

An Inspect Hierarchy consists of nested **Nodes** containing typed **Properties**.

## Goals

The Inspect Format described in this document has the following goals:

- **Low-overhead mutations to data**

    The Inspect File Format allows data to be changed in-place. For instance,
    the overhead of incrementing an integer is ~2 atomic increments.

- **Support a non-static hierarchy**

    The hierarchy stored in an Inspect File can be modified at
    runtime. Children can be added or removed from the hierarchy at any
    time. In this way, the hierarchy can closely represent the hierarchy of
    objects in the component's working set.

- **Single writer, multiple reader concurrency without explicit synchronization**

    Readers operating concurrently with the writer map the VMO and attempt to
    take a snapshot of the data. Writers indicate being in a critical section
    though a *generation counter* that requires no explicit synchronization
    with readers. Readers use the generation counter to determine when a
    snapshot of the VMO is consistent and may be safely read.

- **Data may remain available after component termination**

    A reader may maintain a handle to the VMO containing Inspect data even
    after the writing component terminates.

[inspect]: /docs/development/diagnostics/inspect/README.md
[updating-format]: /docs/reference/diagnostics/inspect/updating-vmo-format.md

## Terminology

This section defines common terminology used in this document.

* Inspect File - A bounded sequence of bytes using the format described in this document.
* Inspect VMO - An Inspect File stored in a Virtual Memory Object (VMO).
* Block - A sized section of an Inspect File. Blocks have an Index and an Order.
* Index - A unique identifier for a particular Block. `byte_offset = index * 16`
* Order - The size of a block given as a bit shift from the minimum
          size. `size_in_bytes = 16 << order`. Separates blocks into
          classes by their (power of two) size.
* Node  - A named value in the hierarchy under which other values may
          be nested. Only Nodes may be parents in the Hierarchy.
* Property - A named value that contains typed data (e.g. String,
             Integer, etc).
* Hierarchy - A tree of Nodes, descending from a single "root" node, that
              may each contain Properties. An Inspect File contains a
              single Hierarchy.

This document uses MUST, SHOULD/RECOMMENDED, and MAY keywords as defined in [RFC 2119][rfc2119]

All bit field diagrams are stored in little-endian ordering.

## Version

_Current version: 2_

* **Version 2** allows the name of a value to be either a [NAME](#name)
    or a [STRING_REFERENCE](#stringreference).

# Blocks

Inspect files are split into a number of `Blocks` whose size must be a
power of 2.

The minimum block size must be 16 bytes (`MIN_BLOCK_SIZE`) and the
maximum block size must be a multiple of 16 bytes. Implementers are
recommended specify a maximum block size less than the size of a page
(typically 4096 bytes). In our reference implementation, the maximum
block size is 2048 bytes (`MAX_BLOCK_SIZE`).

All blocks must be aligned on 16-byte boundaries, and addressing within
the VMO is in terms of an Index, specifying a 16-byte offsets (`offset =
index * 16`).

We use 24 bits for indexes, but Inspect Files may be at most 128MiB for legacy reasons.

Note: The content index of Link Value nodes is only 20 bits. That can't account for a 256MB VMO.
Instead of fixing the node, which would require a version bump and extensive testing, we just
restrict the size of the VMO.

A `block_header` consists of 8 bytes as follows:

```mermaid
---
title: "Inspect block header"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type"
16-63: "depends on type"
```

Each block has an `order`, specifying its size.

If the maximum block size is 2048 bytes, then there are 8 possible block
orders (`NUM_ORDERS`), numbered 0...7, corresponding to blocks of sizes
16, 32, 64, 128, 256, 512, 1024, and 2048 bytes respectively.

Each block also has a type, which is used to determine how the rest of
the bytes in the block are to be interpreted.

## Buddy Allocation

This block layout permits efficient allocation of blocks using [buddy
allocation][buddy]. Buddy allocation is the recommended allocation
strategy, but it is not a requirement for using the Inspect Format.

# Types

All the supported types are defined in
[//zircon/system/ulib/inspect/include/lib/inspect/cpp/vmo/block.h][block.h]
which fall into categories as follows:

enum               | value | type name                | category
-------------------|-------|--------------------------|-------
`kFree`            | 0     | `FREE`                   | Internal
`kReserved`        | 1     | `RESERVED`               | Internal
`kHeader`          | 2     | `HEADER`                 | Header
`kNodeValue`       | 3     | `NODE_VALUE`             | Value
`kIntValue`        | 4     | `INT_VALUE`              | Value
`kUintValue`       | 5     | `UINT_VALUE`             | Value
`kDoubleValue`     | 6     | `DOUBLE_VALUE`           | Value
`kBufferValue`     | 7     | `BUFFER_VALUE`           | Value
`kExtent`          | 8     | `EXTENT`                 | Extent
`kName`            | 9     | `NAME`                   | Name
`kTombstone`       | 10    | `TOMBSTONE`              | Value
`kArrayValue`      | 11    | `ARRAY_VALUE`            | Value
`kLinkValue`       | 12    | `LINK_VALUE`             | Value
`kBoolValue`       | 13    | `BOOL_VALUE`             | Value
`kStringReference` | 14    | `STRING_REFERENCE`       | Reference

* *Internal* - These types are provided for implementing block allocation, and
they must be ignored by readers.

* *Header* - This type allows readers to detect Inspect Files and reason
about snapshot consistency. This block must exist at index 0.

* *Value* - These types appear directly in the hierarchy. Values must have a *Name*
and a parent (which must be a `NODE_VALUE`).

* *Extent* - This type stores long binary data that may not fit in a single block.

* *Name* - This type stores binary data that fits in a single block,
and it is typically used to store the name of values.

* *Reference* - This type holds a single canonical value to which other blocks can refer.

Each type interprets the payload differently, as follows:

* [FREE](#free)
* [RESERVED](#reserved)
* [HEADER](#header)
* [Common VALUE fields](#value)
* [NODE\_VALUE](#node)
* [INT\_VALUE](#numeric)
* [UINT\_VALUE](#numeric)
* [DOUBLE\_VALUE](#numeric)
* [BUFFER\_VALUE](#buffer_value)
* [EXTENT](#extent)
* [NAME](#name)
* [TOMBSTONE](#node)
* [ARRAY\_VALUE](#array)
* [LINK](#link)
* [BOOL\_VALUE](#numeric)
* [STRING\_REFERENCE](#stringreference)

## FREE {#free}

```mermaid
---
title: "Free block"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type (0)"
16-39: "next free block"
40-63: "unused"
64-127: "unused (cont.)"
```

A `FREE` block is available for allocation. Importantly, the zero-valued
block (16 bytes of `\0`) is interpreted as a `FREE` block of order 0,
so buffers may simply be zeroed to free all blocks.

Writer implementations may use the unused bits from 8..63 of `FREE`
blocks for any purpose. Writer implementation must set all other unused
bits to 0.

It is recommended that writers use the location specified above to store
the index of the next free block of the same order. Using this field,
free blocks may create singly linked lists of free blocks of each size
for fast allocation. The end of the list is reached when NextFreeBlock
points to a location that is either not `FREE` or not of the same order
(commonly the Header block at index 0).

## RESERVED {#reserved}

```mermaid
---
title: "Reserved block"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type (1)"
16-31: "unused"
32-127: "unused (cont.)"
```

`RESERVED` blocks are simply available to be changed to a different
type.  It is an optional transitional state between the allocation of a
block and setting its type that is useful for correctness checking of
implementations (to ensure that blocks that are about to be used are
not treated as free).

## HEADER {#header}

```mermaid
---
title: "Header block"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type (2)"
16-31: "version (2)"
32-63: "magic number (INSP)"
64-95: "generation count"
96-127: "generation count (cont.)"
128-159: "size in bytes"
160-191: "unused"
192-255: "unused (cont.)"
```

There must be one `HEADER` block at the beginning of the file. It consists
of a **Magic Number** ("INSP"), a **Version** (currently 2), the
**Generation Count** for concurrency control and the size of the part of the VMO
that is allocated in bytes. The first byte of the header must not be a valid
ASCII character.

See the [next section](#concurrency) for how concurrency control must be
implemented using the generation count.

## NODE\_VALUE and TOMBSTONE {#node}


```mermaid
---
title: "Node value and tombstone"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type (3|10)"
16-39: "parent index"
40-63: "name index"
64-95: "reference count (optional)"
96-127: "reference count (cont.)"
```

Nodes are anchor points for further nesting, and the `ParentID` field
of values must only refer to blocks of type `NODE_VALUE`.

`NODE_VALUE` blocks support optional *reference counting* and *tombstoning*
to permit efficient implementations as follows:

The `Refcount` field may contain the number of values referencing a given
`NODE_VALUE` as their parent. On deletion, the `NODE_VALUE` becomes a new
special type called `TOMBSTONE`. `TOMBSTONE`s are deleted only when their
`Refcount` is 0.

This allows for writer implementations that do not need to explicitly
keep track of children for Nodes, and it prevents the following scenario:

```
// "b" has a parent "a"
Index | Value
0     | HEADER
1     | NODE "a", parent 0
2     | NODE "b", parent 1

/* delete "a", allocate "c" as a child of "b" which reuses index 1 */

// "b"'s parent is now suddenly "c", introducing a cycle!
Index | Value
0     | HEADER
1     | NODE "c", parent 2
2     | NODE "b", parent 1
```

## \{INT,UINT,DOUBLE,BOOL\}\_VALUE {#numeric}

```mermaid
---
title: "Numeric/bool value block"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type (4|5|6|13)"
16-39: "parent index"
40-63: "name index"
64-95: "inlined value"
96-127: "inlined value (cont.)"
```

Numeric `VALUE` blocks all contain the 64-bit numeric type inlined into
the second 8 bytes of the block. Numeric values are little endian.

## BUFFER\_VALUE {#buffer_value}

```mermaid
---
title: "Buffer value block"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type (7)"
16-39: "parent index"
40-63: "name index"
64-95: "total length"
96-123: "extent OR string reference index"
124-127: "format (0|1|2)"
```

`BUFFER_VALUE` blocks may point to either the first `EXTENT` block in a chain, or to a
`STRING_REFERENCE`.

For `format` values of `kUtf8` or `kBinary`, the referee is an `EXTENT` chain. For `format` values
of `kStringReference`, the referee is a `STRING_REFERENCE`.

If the `format` is `kStringReference`, then the `total length` field is zeroed.

The format flags specify how the byte data should be interpreted,
as follows:

Enum    | Value | Meaning
----    | ----  | ----
kUtf8   | 0     | The byte data may be interpreted as a UTF-8 string.
kBinary | 1     | The byte data is arbitrary binary data and may not be printable.
kStringReference | 2     | The data is a `STRING_REFERENCE` block.

## EXTENT {#extent}

```mermaid
---
title: "Extent block"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type (8)"
16-39: "next extent index"
40-63: "reserved = 0"
64-95: "payload"
96-127: "payload (cont.)"
```

`EXTENT` blocks contain an arbitrary byte data payload and the index of
the next `EXTENT` in the chain. The byte data for a buffer_value is retrieved
by reading each `EXTENT` in order until **Total Length** bytes are read.

The payload is byte data up to at most the end of the block. The size depends on
the order.

## NAME {#name}

```mermaid
---
title: "Name block"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type (9)"
16-27: "length"
28-63: "reserved = 0"
64-95: "payload"
96-127: "payload (cont.)"
```

`NAME` blocks give objects and values a human-readable identifier. They
consist of a UTF-8 payload that fits entirely within the given block.
The payload is the contents of the name. The size depends on the order.

## STRING\_REFERENCE {#stringreference}

```mermaid
---
title: "Name block"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type (14)"
16-39: "next extent index"
40-63: "reference count"
64-95: "total length"
96-127: "payload"
```

`STRING_REFERENCE` blocks are used to implement strings with reference semantics in the VMO.
They are the start of a linked list of `EXTENT`s, meaning that their values are not size-restricted.
`STRING_REFERENCE` blocks may be used where a `NAME` is expected.

Notes:

- The total length is the size of the payload in bytes. Payload overflows into
  "next extent" if "total length > (16 << order) - 12".
- The payload is the canonical instance of a string. The size of the payload
  depends on the order. If the size of the payload + 12 is greater than "16 << order",
  then the payload is too large to fit in one block and will overflow into the next
  extent.
- The next extent index is the index of the first overflow `EXTENT`, or 0 if the
  payload does not overflow.

## ARRAY\_VALUE {#array}

```mermaid
---
title: "Array value block"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type (11)"
16-39: "parent index"
40-63: "name index"
64-67: "value type (4|5|6|14)"
68-71: "display format (0|1|2)"
72-79: "count of stored values"
80-127: "reserved = 0"
128-159: "payload"
160-191: "payload (cont.)"
```

The format of an `ARRAY_VALUE` block `Payload` depends on the **Stored Value Type** `T`,
interpreted exactly like the **Type** field. Where `T ∊ {4, 5, 6}`, the `Payload` shall be 64-bit
numeric values packed on byte boundaries. Where `T ∊ {14}`, the `Payload` shall be composed of
32-bit values, representing the 24-bit index of a block of type `T`, packed together along byte
boundaries. In this case, only `F = 0`, a flat array, is allowed.

When `F = 0`, `ARRAY_VALUE`s shall be default instantiated. In the numeric case, this shall be the
associated zero value. In the string case, this shall be the empty string, indicated by the special
value `0`.

Exactly **Count** entries of the given **Stored Value Type** (or indexes thereof) appear in
the bytes at offset 16 into the block.

The **Display Format** field is used to affect how the array should be
displayed, and it is interpreted as follows:

Enum                  | Value | Description
---------             | ----  | ----
kFlat                 | 0     | Display as an ordered flat array with no additional formatting.
kLinearHistogram      | 1     | Interpret the first two entries as `floor` and `step_size` parameters for a linear histogram, as defined below.
kExponentialHistogram | 2     | Interpret the first three entries as `floor`, `initial_step`, and `step_multiplier` for an exponential histogram, as defined below.

### Linear Histogram

The array is a linear histogram that stores its parameters inline and
contains both an overflow and underflow bucket.

The first two elements are parameters `floor` and `step_size`, respectively
(as defined below).

The number of buckets (N) is implicitly `Count - 4`.

The remaining elements are buckets:

```
2:     (-inf, floor),
3:     [floor, floor+step_size),
i+3:   [floor + step_size*i, floor + step_size*(i+1)),
...
N+3:   [floor+step_size*N, +inf)
```

### Exponential Histogram

The array is an exponential histogram that stores its parameters inline
and contains both an overflow and underflow bucket.

The first three elements are parameters `floor`, `initial_step`, and
`step_multiplier` respectively (as defined below).

The number of buckets (N) is implicitly Count - 5.

The remaining are buckets:

```
3:     (-inf, floor),
4:     [floor, floor+initial_step),
i+4:   [floor + initial_step * step_multiplier^i, floor + initial_step * step_multiplier^(i+1))
N+4:   [floor + initial_step * step_multiplier^N, +inf)
```

## LINK\_VALUE {#link}

```mermaid
---
title: "Link value block"
---
packet-beta
0-3: "order"
4-7: "reserved = 0"
8-15: "type (12)"
16-39: "parent index"
40-63: "name index"
64-83: "content index"
84-123: "unused"
124-127: "disposition (0|1)"
```

`LINK_VALUE` blocks allow nodes to support children that are present
in a separate Inspect File.

The **Content Index** specifies another `NAME` block whose contents
are a unique identifier referring to another Inspect File. Readers are
expected to obtain a bundle of `(Identifier, File)` pairs (through either
a directory read or another interface) and they may attempt to follow
links by splicing the trees together using the stored identifiers.

The **Disposition Flags** instruct readers on how to splice the trees, as follows:

Enum               | Value | Description
----               | ----  | ----
kChildDisposition  | 0     | The hierarchy stored in the linked file should be a child of the `LINK_VALUE`'s parent.
kInlineDisposition | 1     | The children and properties of the root stored in the linked file should be added to the `LINK_VALUE`'s parent.

For example:

```
// root.inspect
root:
  int_value = 10
  child = LINK("other.inspect")

// other.inspect
root:
  test = "Hello World"
  next:
    value = 0


// kChildDisposition produces:
root:
  int_value = 10
  child:
    test = "Hello World"
    next:
      value = 0

// kInlineDisposition produces:
root:
  int_value = 10
  test = "Hello World"
  next:
    value = 0
```

Note: In all cases the name of the root node in the linked file is ignored.

In the event of a collision in child names between a Node and values being
added by its inline linked child, precedence is reader defined. Most
readers, however, would find it useful to have linked values take
precedence so they may override the original values.

# Concurrency Control {#concurrency}

Writers must use a global version counter so that readers can
detect in-flight modifications and modifications between reads without
communicating with the writer. This supports single-writer multi-reader
concurrency.

The strategy is for writers to increment a global *generation counter*
both when they begin and when they end a write operation.

This is a simple strategy with a significant benefit: between incrementing
the version number for beginning and ending a write the writer can perform
any number of operations on the buffer without regard for atomicity of
data updates.

The main drawback is that reads could be delayed indefinitely due to a
frequently updating writer, but readers can have mitigations in place
in practice.

## Reader Algorithm

Readers use the following algorithm to obtain a consistent snapshot of
an Inspect VMO:

1. Spinlock until the version number is even (no concurrent write),
2. Copy the entire VMO buffer, and
3. Check that the version number from the buffer is equal to the version
number from step 1.

As long as the version numbers match, the client may read their local
copy to construct the shared state.
If the version numbers do not match, the client may retry the whole
process.


## Writer Lock Algorithm

Writers lock an Inspect VMO for modification by doing the following:

```c
atomically_increment(generation_counter, acquire_ordering);
```

This locks the file against concurrent reads by setting the generation to an
odd number. Acquire ordering ensures that loads are not reordered before
this change.

## Writer Unlock Algorithm

Writers unlock an Inspect VMO following modification by doing the
following:

```c
atomically_increment(generation_counter, release_ordering);
```

Unlock the file allowing concurrent reads by setting the generation to
a new even number. Release ordering ensures that writes to the file are
visible before the generation count update is visible.

## Frequently asked questions

### How many bytes does my string need?

Strings are stored in `STRING_REFERENCE` blocks. Therefore, if a string length
is `N` bytes, it may end up using more than `N` bytes in the Inspect VMO. The
following table illustrates how many bytes a string of a specific length
actually uses:


| String length | Block order| Block size (bytes) |
| ------------- | ---------- | ------------------ |
|    0 - 4      | 0          | 16                 |
|    5 - 20     | 1          | 32                 |
|   21 - 52     | 2          | 64                 |
|   53 - 116    | 3          | 128                |
|  117 - 244    | 4          | 256                |
|  245 - 500    | 5          | 512                |
|  501 - 1012   | 6          | 1024               |
| 1013 - 2036   | 7          | 2048               |

If a string is longer than 2036 bytes, then the format begins to use `EXTENT`
blocks for the remaining data.

<!-- xrefs -->
[block.h]: /zircon/system/ulib/inspect/include/lib/inspect/cpp/vmo/block.h
[buddy]: https://en.wikipedia.org/wiki/Buddy_memory_allocation
[rfc2119]: https://www.ietf.org/rfc/rfc2119.txt
[VMO]: /docs/reference/kernel_objects/vm_object.md
