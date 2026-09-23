# Storage Units (`storage_units`)

`storage_units` (`//src/storage/lib/units`) provides zero-cost typed
abstractions for power-of-two block sizes and system memory page sizes.

## Overview

Filesystem and storage code frequently checks alignments, rounds byte offsets
or ranges up/down to block boundaries, and converts between byte counts and
block counts. Using plain integer types (`u64` / `usize`) for block sizes has
two main drawbacks:

1. **Performance**: Standard integer division (`/`) and modulo (`%`) on 64-bit
   integers require expensive multi-cycle CPU division instructions, even when
   the divisor is known at runtime to be a power of two.
2. **Safety & Ergonomics**: Manual alignment math (e.g.
   `(offset + block_size - 1) / block_size` or `offset & !(block_size - 1)`) is
   repetitive and prone to integer overflow bugs near `u64::MAX`.

`storage_units` guarantees that block sizes are powers of two and stores the
alignment bitmask (`size - 1`), replacing division and modulo with single-cycle
bit shifts and masks while providing overflow-safe alignment helpers.

## Core Types

* **[`GenericBlockSize<T: BlockSizeSpec>`](src/lib.rs)**: A power-of-two block
  size parameterized by a [`BlockSizeSpec`](src/lib.rs) that supplies the
  size, bitmask, and shift.
* **[`BlockSize`](src/lib.rs)** (`GenericBlockSize<MaskShiftSpec>`): A dynamic
  or constant block size backed by a packed mask-shift (`MaskShiftSpec`),
  supporting power-of-two sizes up to 4 GiB (`1 << 32`).
  * Predefined constants: `SIZE_512B`, `SIZE_1KIB`, `SIZE_2KIB`, `SIZE_4KIB`,
    `SIZE_8KIB`, `SIZE_16KIB`, `SIZE_32KIB`, `SIZE_64KIB`, `SIZE_128KIB`,
    `SIZE_256KIB`, `SIZE_512KIB`, `SIZE_1MIB`.
* **[`PAGE_SIZE`](src/page.rs)** *(Fuchsia targets)*: A
  `GenericBlockSize<PageSizeSpec>` representing the Fuchsia system memory page
  size. It lazily caches the `MaskShiftSpec` representation of
  `zx::system_get_page_size()` in a relaxed atomic on first use to avoid
  repeated syscalls/vDSO lookups.

### Why is `GenericBlockSize<T>` Generic Over `BlockSizeSpec`?

A filesystem typically deals with multiple distinct domains of block alignment
simultaneously:

* **Device Block Size** (e.g. physical 512B or 4KiB sectors)
* **Filesystem Block Size** (e.g. logical 4KiB blocks in Fxfs)
* **Journal Block Size** (e.g. fixed 4KiB journal chunks)
* **System Page Size** (`PAGE_SIZE`)

If all block sizes shared a single concrete type, the type system could not
distinguish *which* domain a byte offset or range was aligned to. By
parameterizing [`GenericBlockSize<T: BlockSizeSpec>`](src/lib.rs) over a
specification type `T`, subsystems can define distinct types such as
`FxfsBlockSize`, `DeviceBlockSize`, `JournalBlockSize`, or `LayerBlockSize`.

In this hierarchy, **[`BlockSize`](src/lib.rs)**
(`GenericBlockSize<MaskShiftSpec>`) acts as a **universal block size**: it
represents a runtime power-of-two block size when you need to align values or
perform block arithmetic, but do not need (or cannot statically know) a
domain-specific type tag.

## Operations & Helpers

### Alignment Helpers

* `bs.is_aligned(val)`: Returns `true` if a `u64` (or `Range<u64>`) is aligned
  to `bs`.
* `bs.align_down(bytes) -> u64`: Rounds `bytes` down to the nearest block
  boundary (`bytes & !bs.mask()`).
* `bs.align_up(bytes) -> Option<u64>`: Rounds `bytes` up to the nearest block
  boundary, returning `None` on `u64` overflow.
* `bs.align_up_to_blocks(bytes) -> u64`: Rounds `bytes` up to the nearest block
  boundary and returns the total block count without overflowing `u64`.
* `bs.align_range_outwards(range) -> Option<Range<u64>>`: Aligns `range.start`
  down and `range.end` up to block boundaries.

### Arithmetic & Comparisons

`GenericBlockSize` implements standard arithmetic (`+`, `-`, `*`, `/`, `%` and
their assignment counterparts) and comparison traits (`PartialEq`, `Eq`,
`PartialOrd`, `Ord`, `Hash`) with `u64` values and across different
`BlockSizeSpec` types:

* **Division (`bytes / bs`)**: Compiles to `bytes >> bs.shift()`.
* **Remainder (`bytes % bs`)**: Compiles to `bytes & bs.mask()`.
* **Multiplication (`blocks * bs`)**: Compiles to `blocks << bs.shift()` (with
  debug assertions checking for overflow).

## Example

```rust
use storage_units::BlockSize;

let bs = BlockSize::SIZE_4KIB;

assert!(bs.is_aligned(8192u64));
assert!(bs.is_aligned(0..4096));

assert_eq!(bs.align_down(5000), 4096);
assert_eq!(bs.align_up(5000), Some(8192));
assert_eq!(bs.align_up_to_blocks(5000), 2);
assert_eq!(bs.align_range_outwards(100..5000), Some(0..8192));

// Arithmetic with u64 uses shifts and masks
let blocks = 12288u64 / bs; // 3
let rem = 5000u64 % bs;     // 904
let bytes = blocks * bs;    // 12288
```

## Future Work

Planned additions to `storage_units` include newtypes parameterized by `T:
BlockSizeSpec` that encode both block alignment and alignment domain invariants
directly in the type system:

* **`BlockAligned<T>`**: Represents a byte offset or byte count guaranteed to
  be a multiple of the block size `T`.
* **`BlockAlignedRange<T>`**: Represents a `Range` whose start and end offsets
  are both guaranteed to be aligned to `T`.
* **`BlockCount<T>`**: Represents an explicit count of blocks of size `T`
  rather than raw bytes.

Because these types are generic over `T: BlockSizeSpec`, a byte count aligned
to the device block size (`BlockAligned<DeviceBlockSize>`) will be a distinct
type from a byte count aligned to the filesystem block size
(`BlockAligned<FxfsBlockSize>`). Accepting these types in function signatures
in place of raw `u64` values eliminates redundant runtime `is_aligned` checks
and prevents accidentally passing values aligned to the wrong block size at
compile time.
