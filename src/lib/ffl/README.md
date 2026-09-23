# Fuchsia Fixed-Point Library (FFL) for Rust

A Rust port of the Fuchsia Fixed-Point Library (originally
`//zircon/system/ulib/ffl`).

## Introduction

FFL is a fixed-point arithmetic library. In Rust, it is designed as a type-safe
library to support kernel scheduler development and other performance-sensitive
code where floating-point math is unavailable or undesirable, and well-defined
saturating arithmetic and convergent rounding (round-half-to-even) are required.

## Key Types

1. **`Fixed<I, const FRAC: usize>`**:
   The primary fixed-point type. It uses the integer type `I` as the underlying
   representation, and reserves `FRAC` bits for the fractional component.
   Supports standard arithmetic operators (addition, subtraction,
   multiplication, division, negation), relational operators, and
   formatting/display.

2. **`FastFixed<I, const FRAC: usize>`**:
   A wrapper around `Fixed` that forces intermediate calculations to be
   performed in 64-bit saturating space for maximum execution efficiency on
   64-bit targets.  For performance-sensitive code paths (like scheduler tick
   math), `FastFixed` can be used to avoid 128-bit intermediates where 64-bit
   intermediates are known not to overflow or where saturation to 64-bit limits
   is acceptable.
## General Usage

```rust
use ffl::Fixed;

// Construct Fixed values
let a = Fixed::<i32, 16>::from_integer(5);
let b = Fixed::<i32, 16>::from_raw(12345);

// Arithmetic
let sum = a + b;
let diff = a - b;

// Division produces a DivExpression, which is evaluated on conversion back to
// Fixed
let ratio = Fixed::<i32, 16>::from(a / b);
```

## Expression Trees and Precision

To preserve precision during intermediate calculation (especially division),
arithmetic operators return intermediate types:

- Division returns a `DivExpression`, which evaluates only when converted back
  to a `Fixed` type or coerced to a target resolution via `to_resolution`.
- Mixed-precision arithmetic uses `PromoteAddSub` and `PromoteMul` traits to
  automatically select wider intermediate integer types, preventing premature
  overflow/underflow.

### Resolution Coercion

You can coerce intermediate expression results to a specific target resolution
using `to_resolution`:
```rust
use ffl::{from_ratio, to_resolution, Fixed};

let val = to_resolution::<2, i32, _, _>(from_ratio(123, 456));
```
