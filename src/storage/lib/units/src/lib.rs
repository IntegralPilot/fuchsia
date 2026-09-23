// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Types for representing and performing efficient power-of-two block size arithmetic and
//! alignments.

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt::{self, Debug};
use std::ops::{
    Add, AddAssign, Div, DivAssign, Mul, MulAssign, Range, Rem, RemAssign, Sub, SubAssign,
};

#[cfg(target_os = "fuchsia")]
mod page;
#[cfg(target_os = "fuchsia")]
pub use crate::page::*;

/// Defines the block size specification for use with [`GenericBlockSize`]. Only block sizes which
/// are a power of 2 are supported.
// TODO(https://github.com/rust-lang/rust/issues/143874): Make this a const trait.
pub trait BlockSizeSpec: Copy + Clone + Debug + Eq + PartialEq {
    /// Returns the block size in bytes (e.g. `4096` for 4096-byte blocks).
    fn size(self) -> u64;

    /// Returns the bitmask corresponding to `size - 1` (e.g. `4095` for 4096-byte blocks).
    fn mask(self) -> u64;

    /// Returns the power-of-two bit shift (e.g. `12` for 4096-byte blocks).
    fn shift(self) -> u32;
}

/// Represents a block size that is a power of 2, parameterized by a [`BlockSizeSpec`].
///
/// Provides zero-cost bitwise operations for alignments, conversions, and arithmetic with
/// integers.
#[derive(Copy, Clone, Debug, Eq)]
pub struct GenericBlockSize<T: BlockSizeSpec>(T);

impl<T: BlockSizeSpec> GenericBlockSize<T> {
    #[inline(always)]
    pub fn from_spec(spec: T) -> Self {
        Self(spec)
    }

    /// Returns the block size in bytes.
    #[inline(always)]
    pub fn get(self) -> u64 {
        self.0.size()
    }

    /// Returns the alignment mask (`size - 1`).
    #[inline(always)]
    pub fn mask(self) -> u64 {
        self.0.mask()
    }

    /// Returns the power-of-two bit shift (e.g. `12` for 4096-byte blocks).
    #[inline(always)]
    pub fn shift(self) -> u32 {
        self.0.shift()
    }

    /// Returns `true` if `value` is aligned to this block size.
    #[inline(always)]
    pub fn is_aligned(self, value: impl IsAligned<T>) -> bool {
        value.is_aligned(self)
    }

    /// Aligns `bytes` up to the nearest block boundary, returning `None` on overflow.
    #[inline(always)]
    pub fn align_up(self, bytes: u64) -> Option<u64> {
        match bytes.checked_add(self.mask()) {
            Some(bytes) => Some(self.align_down(bytes)),
            None => None,
        }
    }

    /// Aligns `bytes` down to the nearest block boundary.
    #[inline(always)]
    pub fn align_down(self, bytes: u64) -> u64 {
        bytes & !self.mask()
    }

    /// Aligns `bytes` up to the nearest block boundary and returns the total number of blocks.
    #[inline(always)]
    pub fn align_up_to_blocks(self, bytes: u64) -> u64 {
        bytes / self + ((bytes % self != 0) as u64)
    }

    /// Aligns the range outwards to block boundaries (`start` aligned down, `end` aligned up).
    ///
    /// Returns `None` if aligning `end` overflows `u64`.
    #[inline(always)]
    pub fn align_range_outwards(self, range: impl Borrow<Range<u64>>) -> Option<Range<u64>> {
        let range = range.borrow();
        let start = self.align_down(range.start);
        let end = self.align_up(range.end)?;
        Some(start..end)
    }
}

/// A [`BlockSizeSpec`] that packs the alignment mask in the upper 32 bits and the power-of-two bit
/// shift in the lower 32 bits: `(mask << 32) | shift`.
///
/// This representation is an extremely efficient way to store a runtime power-of-two block size on
/// modern 64-bit architectures (AArch64 and x86_64).
///
/// ### Why this representation was chosen:
///
/// 1. **Zero Dynamic Bit-Scanning Latency:** Representations that only store the size or the mask
///    must dynamically compute `shift` via trailing-zero counting (`trailing_zeros` or
///    `trailing_ones`).
///    - On **ARM64**, computing trailing ones requires a dependent instruction chain: `mvn` +
///      `rbit` + `clz`. On in-order efficiency cores, this 3-instruction dependency stalls the
///      pipeline, making operations like `shift`, `div`, and `mul` more than 2x slower.
///
/// 2. **The 6-Bit Shift Rule (Zero-Instruction Unpacking for Shifts):** Variable shift instructions
///    on both AArch64 (`lsr xd, xn, xm` / `lsl xd, xn, xm`) and x86_64 (`shrx r64, r64, r64`) only
///    inspect the lowest 6 bits of the register operand (`shift_amount mod 64`). Because `shift`
///    (which is between 0 and 32) is placed in the low 32 bits (`[31..0]`), the lowest 6 bits of
///    the raw `u64` are *literally* the shift amount. Consequently, compilers do not need to mask,
///    clear, or move `shift` into a temporary register before shifting. The raw 64-bit
///    `MaskShiftSpec` register can be passed directly as the shift operand, executing in a **single
///    instruction and single cycle** (`lsr x_val, x_val, x_bs`).
///
/// 3. **ARM64 Fused Barrel Shifter (`adds ..., lsr #32`):** ARM64 ALU instructions feature a
///    hardware barrel shifter that can shift an input operand at zero extra cycle cost. To add the
///    mask to an offset (such as during `align_up` or `align_up_to_blocks`), the compiler emits:
///    ```text
///    adds x_res, x_val, x_bs, lsr #32
///    ```
///    This shifts out the low 32 bits and adds the high 32-bit mask in a **single cycle**. In
///    `align_up_to_blocks`, the compiler follows this immediately with `lsr x_res, x_res, x_bs`,
///    performing the combined align-and-divide in just **two back-to-back single-cycle
///    instructions**.
///
/// 4. **Packed `u64` vs. Two Separate `u32` Fields:** Storing a single packed `u64` is superior to
///    a struct with two `u32`s (`{ shift: u32, mask: u32 }`):
///    - **Register Pressure & ABI:** Under Rust's internal calling convention, small multi-field
///      structs are scalarized across multiple registers when passed by value. A packed `u64`
///      always consumes a single argument register, avoiding register pressure and spills in
///      functions with many arguments.
///    - **Register Reuse on ARM64:** With a single `u64`, the exact same register can be fed
///      directly into both the barrel-shifted mask operation (`adds ..., x0, lsr #32`) and the
///      shift (`lsr ..., x0`). Separate fields force values into different registers and require
///      moving or zero-extending them.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct MaskShiftSpec(u64);

impl BlockSizeSpec for MaskShiftSpec {
    #[inline(always)]
    fn size(self) -> u64 {
        self.mask() + 1
    }

    #[inline(always)]
    fn mask(self) -> u64 {
        self.0 >> 32
    }

    #[inline(always)]
    fn shift(self) -> u32 {
        self.0 as u32
    }
}

impl MaskShiftSpec {
    /// Constructs a new MaskShiftSpec.
    ///
    /// #Panics
    ///
    /// Panics in debug mode if `block_size` is not a power of 2 or is greater than 4GiB.
    #[inline(always)]
    const fn new(block_size: u64) -> Self {
        debug_assert!(block_size.is_power_of_two() && block_size <= (1 << 32));
        let mask = (block_size - 1) as u32 as u64;
        let shift = (block_size - 1).trailing_ones();
        Self((mask << 32) | (shift as u64))
    }
}

/// A [`GenericBlockSize`] configured with a block size stored as a [`MaskShiftSpec`].
pub type BlockSize = GenericBlockSize<MaskShiftSpec>;

impl BlockSize {
    pub const SIZE_512B: Self = Self::new(1 << 9).unwrap();
    pub const SIZE_1KIB: Self = Self::new(1 << 10).unwrap();
    pub const SIZE_2KIB: Self = Self::new(1 << 11).unwrap();
    pub const SIZE_4KIB: Self = Self::new(1 << 12).unwrap();
    pub const SIZE_8KIB: Self = Self::new(1 << 13).unwrap();
    pub const SIZE_16KIB: Self = Self::new(1 << 14).unwrap();
    pub const SIZE_32KIB: Self = Self::new(1 << 15).unwrap();
    pub const SIZE_64KIB: Self = Self::new(1 << 16).unwrap();
    pub const SIZE_128KIB: Self = Self::new(1 << 17).unwrap();
    pub const SIZE_256KIB: Self = Self::new(1 << 18).unwrap();
    pub const SIZE_512KIB: Self = Self::new(1 << 19).unwrap();
    pub const SIZE_1MIB: Self = Self::new(1 << 20).unwrap();

    /// Constructs a `BlockSize` from a `u32` byte count if it is a power of 2 and not equal to 0.
    #[inline(always)]
    pub const fn new(block_size: u32) -> Option<Self> {
        Self::from_u64(block_size as u64)
    }

    /// Constructs a `BlockSize` from a `u64` if it is a power of 2, not equal to 0, and the mask
    /// fits in a u32. This is equivalent to `Self::new` but allows for constructing a 4GiB block
    /// size.
    #[inline(always)]
    pub const fn from_u64(block_size: u64) -> Option<Self> {
        if block_size.is_power_of_two() && block_size <= (1 << 32) {
            Some(GenericBlockSize(MaskShiftSpec::new(block_size)))
        } else {
            None
        }
    }

    /// Returns the block size in bytes.
    ///
    /// Equivalent to `Self::get` but can be called from a const context.
    // TODO(https://github.com/rust-lang/rust/issues/143874) Remove once `BlockSizeSpec::mask` is
    // const.
    #[inline(always)]
    pub const fn size(self) -> u64 {
        (self.0.0 >> 32) + 1
    }
}

macro_rules! impl_binary_op {
    ($trait:ident, $method:ident, $lhs:ty, $rhs:ty, |$a:ident, $b:ident| $expr:expr) => {
        impl<T: BlockSizeSpec> $trait<$rhs> for $lhs {
            type Output = u64;
            #[inline(always)]
            fn $method(self, other: $rhs) -> u64 {
                let $a = self;
                let $b = other;
                $expr
            }
        }
    };
}

// Add: GenericBlockSize + u64 and u64 + GenericBlockSize (and reference variants)
impl_binary_op!(Add, add, GenericBlockSize<T>, u64, |bs, val| bs.get() + val);
impl_binary_op!(Add, add, GenericBlockSize<T>, &u64, |bs, val| bs.get() + *val);
impl_binary_op!(Add, add, &GenericBlockSize<T>, u64, |bs, val| bs.get() + val);
impl_binary_op!(Add, add, &GenericBlockSize<T>, &u64, |bs, val| bs.get() + *val);

impl_binary_op!(Add, add, u64, GenericBlockSize<T>, |val, bs| val + bs.get());
impl_binary_op!(Add, add, u64, &GenericBlockSize<T>, |val, bs| val + bs.get());
impl_binary_op!(Add, add, &u64, GenericBlockSize<T>, |val, bs| *val + bs.get());
impl_binary_op!(Add, add, &u64, &GenericBlockSize<T>, |val, bs| *val + bs.get());

// Sub: GenericBlockSize - u64 and u64 - GenericBlockSize (and reference variants)
impl_binary_op!(Sub, sub, GenericBlockSize<T>, u64, |bs, val| bs.get() - val);
impl_binary_op!(Sub, sub, GenericBlockSize<T>, &u64, |bs, val| bs.get() - *val);
impl_binary_op!(Sub, sub, &GenericBlockSize<T>, u64, |bs, val| bs.get() - val);
impl_binary_op!(Sub, sub, &GenericBlockSize<T>, &u64, |bs, val| bs.get() - *val);

impl_binary_op!(Sub, sub, u64, GenericBlockSize<T>, |val, bs| val - bs.get());
impl_binary_op!(Sub, sub, u64, &GenericBlockSize<T>, |val, bs| val - bs.get());
impl_binary_op!(Sub, sub, &u64, GenericBlockSize<T>, |val, bs| *val - bs.get());
impl_binary_op!(Sub, sub, &u64, &GenericBlockSize<T>, |val, bs| *val - bs.get());

#[inline(always)]
fn mul_block_size<T: BlockSizeSpec>(val: u64, bs: GenericBlockSize<T>) -> u64 {
    let shift = bs.shift();
    let res = val << shift;
    // Preserve the panic on overflow during multiplication in debug builds.
    debug_assert_eq!(res >> shift, val, "attempt to multiply with overflow");
    res
}

// Mul: GenericBlockSize * u64 and u64 * GenericBlockSize (and reference variants)
impl_binary_op!(Mul, mul, GenericBlockSize<T>, u64, |bs, val| mul_block_size(val, bs));
impl_binary_op!(Mul, mul, GenericBlockSize<T>, &u64, |bs, val| mul_block_size(*val, bs));
impl_binary_op!(Mul, mul, &GenericBlockSize<T>, u64, |bs, val| mul_block_size(val, *bs));
impl_binary_op!(Mul, mul, &GenericBlockSize<T>, &u64, |bs, val| mul_block_size(*val, *bs));

impl_binary_op!(Mul, mul, u64, GenericBlockSize<T>, |val, bs| mul_block_size(val, bs));
impl_binary_op!(Mul, mul, u64, &GenericBlockSize<T>, |val, bs| mul_block_size(val, *bs));
impl_binary_op!(Mul, mul, &u64, GenericBlockSize<T>, |val, bs| mul_block_size(*val, bs));
impl_binary_op!(Mul, mul, &u64, &GenericBlockSize<T>, |val, bs| mul_block_size(*val, *bs));

// Div: u64 / GenericBlockSize (and reference variants)
impl_binary_op!(Div, div, u64, GenericBlockSize<T>, |val, bs| val >> bs.shift());
impl_binary_op!(Div, div, u64, &GenericBlockSize<T>, |val, bs| val >> bs.shift());
impl_binary_op!(Div, div, &u64, GenericBlockSize<T>, |val, bs| *val >> bs.shift());
impl_binary_op!(Div, div, &u64, &GenericBlockSize<T>, |val, bs| *val >> bs.shift());

// Rem: u64 % GenericBlockSize (and reference variants)
impl_binary_op!(Rem, rem, u64, GenericBlockSize<T>, |val, bs| val & bs.mask());
impl_binary_op!(Rem, rem, u64, &GenericBlockSize<T>, |val, bs| val & bs.mask());
impl_binary_op!(Rem, rem, &u64, GenericBlockSize<T>, |val, bs| *val & bs.mask());
impl_binary_op!(Rem, rem, &u64, &GenericBlockSize<T>, |val, bs| *val & bs.mask());

macro_rules! impl_assign_op {
    ($trait:ident, $method:ident, $rhs:ty, |$a:ident, $b:ident| $expr:expr) => {
        impl<T: BlockSizeSpec> $trait<$rhs> for u64 {
            #[inline(always)]
            fn $method(&mut self, other: $rhs) {
                let $a = self;
                let $b = other;
                $expr
            }
        }
    };
}

// Assign operations: u64 <op>= GenericBlockSize (and reference variants)
impl_assign_op!(AddAssign, add_assign, GenericBlockSize<T>, |val, bs| *val += bs.get());
impl_assign_op!(AddAssign, add_assign, &GenericBlockSize<T>, |val, bs| *val += bs.get());

impl_assign_op!(SubAssign, sub_assign, GenericBlockSize<T>, |val, bs| *val -= bs.get());
impl_assign_op!(SubAssign, sub_assign, &GenericBlockSize<T>, |val, bs| *val -= bs.get());

impl_assign_op!(MulAssign, mul_assign, GenericBlockSize<T>, |val, bs| *val =
    mul_block_size(*val, bs));
impl_assign_op!(MulAssign, mul_assign, &GenericBlockSize<T>, |val, bs| *val =
    mul_block_size(*val, *bs));

impl_assign_op!(DivAssign, div_assign, GenericBlockSize<T>, |val, bs| *val >>= bs.shift());
impl_assign_op!(DivAssign, div_assign, &GenericBlockSize<T>, |val, bs| *val >>= bs.shift());

impl_assign_op!(RemAssign, rem_assign, GenericBlockSize<T>, |val, bs| *val &= bs.mask());
impl_assign_op!(RemAssign, rem_assign, &GenericBlockSize<T>, |val, bs| *val &= bs.mask());

macro_rules! impl_partial_eq_ord {
    ($lhs:ty, $rhs:ty, |$a:ident, $b:ident| ($expr_a:expr, $expr_b:expr)) => {
        impl<T: BlockSizeSpec> PartialEq<$rhs> for $lhs {
            #[inline(always)]
            fn eq(&self, other: &$rhs) -> bool {
                let $a = self;
                let $b = other;
                $expr_a == $expr_b
            }
        }

        impl<T: BlockSizeSpec> PartialOrd<$rhs> for $lhs {
            #[inline(always)]
            fn partial_cmp(&self, other: &$rhs) -> Option<Ordering> {
                let $a = self;
                let $b = other;
                $expr_a.partial_cmp(&$expr_b)
            }
        }
    };
}

// Cross-type comparisons: PartialEq and PartialOrd between GenericBlockSize and u64 (and reference variants)
impl_partial_eq_ord!(GenericBlockSize<T>, u64, |bs, val| (bs.get(), *val));
impl_partial_eq_ord!(GenericBlockSize<T>, &u64, |bs, val| (bs.get(), **val));
impl_partial_eq_ord!(&GenericBlockSize<T>, u64, |bs, val| (bs.get(), *val));
impl_partial_eq_ord!(u64, GenericBlockSize<T>, |val, bs| (*val, bs.get()));
impl_partial_eq_ord!(u64, &GenericBlockSize<T>, |val, bs| (*val, bs.get()));
impl_partial_eq_ord!(&u64, GenericBlockSize<T>, |val, bs| (**val, bs.get()));

impl<T: BlockSizeSpec, U: BlockSizeSpec> PartialEq<GenericBlockSize<T>> for GenericBlockSize<U> {
    #[inline(always)]
    fn eq(&self, other: &GenericBlockSize<T>) -> bool {
        self.get() == other.get()
    }
}

impl<T: BlockSizeSpec, U: BlockSizeSpec> PartialOrd<GenericBlockSize<T>> for GenericBlockSize<U> {
    #[inline(always)]
    fn partial_cmp(&self, other: &GenericBlockSize<T>) -> Option<Ordering> {
        self.get().partial_cmp(&other.get())
    }
}

impl<T: BlockSizeSpec> Ord for GenericBlockSize<T> {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> Ordering {
        self.get().cmp(&other.get())
    }
}

impl<T: BlockSizeSpec> std::hash::Hash for GenericBlockSize<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.get().hash(state);
    }
}

macro_rules! impl_fmt {
    ($($trait:ident),*) => {
        $(
            impl<T: BlockSizeSpec> fmt::$trait for GenericBlockSize<T> {
                #[inline(always)]
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    fmt::$trait::fmt(&self.get(), f)
                }
            }
        )*
    };
}

// Formatting traits: Display, Binary, LowerHex, UpperHex, Octal
impl_fmt!(Display, Binary, LowerHex, UpperHex, Octal);

/// A trait for checking if something is aligned to a block size.
pub trait IsAligned<T: BlockSizeSpec> {
    fn is_aligned(self, block_size: GenericBlockSize<T>) -> bool;
}

impl<T: BlockSizeSpec> IsAligned<T> for u64 {
    #[inline(always)]
    fn is_aligned(self, block_size: GenericBlockSize<T>) -> bool {
        self % block_size == 0
    }
}

impl<T: BlockSizeSpec> IsAligned<T> for &u64 {
    #[inline(always)]
    fn is_aligned(self, block_size: GenericBlockSize<T>) -> bool {
        block_size.is_aligned(*self)
    }
}

impl<T: BlockSizeSpec> IsAligned<T> for Range<u64> {
    #[inline(always)]
    fn is_aligned(self, block_size: GenericBlockSize<T>) -> bool {
        block_size.is_aligned(&self)
    }
}

impl<T: BlockSizeSpec> IsAligned<T> for &Range<u64> {
    #[inline(always)]
    fn is_aligned(self, block_size: GenericBlockSize<T>) -> bool {
        (self.start | self.end) % block_size == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_new() {
        assert!(BlockSize::new(0).is_none());
        assert_eq!(BlockSize::new(1).unwrap().get(), 1);
        assert!(BlockSize::new(3).is_none());
        assert_eq!(BlockSize::new(512).unwrap().get(), 512);
        assert_eq!(BlockSize::new(4096).unwrap().get(), 4096);
        assert!(BlockSize::new(u32::MAX).is_none());

        assert!(BlockSize::from_u64(0).is_none());
        assert_eq!(BlockSize::from_u64(1).unwrap().get(), 1);
        assert!(BlockSize::from_u64(3).is_none());
        assert_eq!(BlockSize::from_u64(4096).unwrap().get(), 4096);
        assert_eq!(BlockSize::from_u64(u32::MAX as u64 + 1).unwrap().get(), 1 << 32);
        assert!(BlockSize::from_u64((1 << 32) + 1).is_none());
        assert!(BlockSize::from_u64(1 << 33).is_none());
        assert!(BlockSize::from_u64(u64::MAX).is_none());
    }

    #[fuchsia::test]
    fn test_getters_and_constants() {
        let bs = BlockSize::SIZE_4KIB;
        assert_eq!(bs.get(), 4096);
        assert_eq!(bs.size(), 4096);
        assert_eq!(bs.mask(), 4095);
        assert_eq!(bs.shift(), 12);

        assert_eq!(BlockSize::SIZE_512B.get(), 512);
        assert_eq!(BlockSize::SIZE_1KIB.get(), 1024);
        assert_eq!(BlockSize::SIZE_2KIB.get(), 2048);
        assert_eq!(BlockSize::SIZE_4KIB.get(), 4096);
        assert_eq!(BlockSize::SIZE_8KIB.get(), 8192);
        assert_eq!(BlockSize::SIZE_16KIB.get(), 16384);
        assert_eq!(BlockSize::SIZE_32KIB.get(), 32768);
        assert_eq!(BlockSize::SIZE_64KIB.get(), 65536);
        assert_eq!(BlockSize::SIZE_128KIB.get(), 131072);
        assert_eq!(BlockSize::SIZE_256KIB.get(), 262144);
        assert_eq!(BlockSize::SIZE_512KIB.get(), 524288);
        assert_eq!(BlockSize::SIZE_1MIB.get(), 1048576);
    }

    #[fuchsia::test]
    fn test_alignment_helpers() {
        let bs = BlockSize::SIZE_4KIB;

        assert!(bs.is_aligned(0u64));
        assert!(bs.is_aligned(4096u64));
        assert!(bs.is_aligned(&4096u64));
        assert!(!bs.is_aligned(4095u64));
        assert!(!bs.is_aligned(4097u64));

        assert!(bs.is_aligned(0..4096));
        assert!(bs.is_aligned(&(4096..8192)));
        assert!(!bs.is_aligned(0..4095));
        assert!(!bs.is_aligned(1..4096));
        assert!(!bs.is_aligned(1..4095));

        assert_eq!(bs.align_down(0), 0);
        assert_eq!(bs.align_down(4095), 0);
        assert_eq!(bs.align_down(4096), 4096);
        assert_eq!(bs.align_down(4097), 4096);

        assert_eq!(bs.align_up(0), Some(0));
        assert_eq!(bs.align_up(1), Some(4096));
        assert_eq!(bs.align_up(4095), Some(4096));
        assert_eq!(bs.align_up(4096), Some(4096));
        // Exact overflow boundary for 4KiB blocks:
        assert_eq!(bs.align_up(u64::MAX - 4095), Some(u64::MAX - 4095));
        assert_eq!(bs.align_up(u64::MAX - 4094), None);
        assert_eq!(bs.align_up(u64::MAX), None);

        assert_eq!(bs.align_up_to_blocks(0), 0);
        assert_eq!(bs.align_up_to_blocks(1), 1);
        assert_eq!(bs.align_up_to_blocks(4095), 1);
        assert_eq!(bs.align_up_to_blocks(4096), 1);
        assert_eq!(bs.align_up_to_blocks(4097), 2);
        assert_eq!(bs.align_up_to_blocks(u64::MAX), 1 << 52);

        assert_eq!(bs.align_range_outwards(1..4095), Some(0..4096));
        assert_eq!(bs.align_range_outwards(&(0..4096)), Some(0..4096));
        assert_eq!(bs.align_range_outwards(4096..4096), Some(4096..4096));
        assert_eq!(bs.align_range_outwards(10..10), Some(0..4096));
        assert_eq!(bs.align_range_outwards(0..u64::MAX), None);
    }

    #[fuchsia::test]
    fn test_arithmetic_ops() {
        let bs = BlockSize::SIZE_4KIB;
        let bs_ref = &bs;
        let val = 8192u64;
        let val_ref = &val;

        // Add
        assert_eq!(bs + val, 12288);
        assert_eq!(bs + val_ref, 12288);
        assert_eq!(bs_ref + val, 12288);
        assert_eq!(bs_ref + val_ref, 12288);
        assert_eq!(val + bs, 12288);
        assert_eq!(val + bs_ref, 12288);
        assert_eq!(val_ref + bs, 12288);
        assert_eq!(val_ref + bs_ref, 12288);

        // Sub
        assert_eq!(val - bs, 4096);
        assert_eq!(val - bs_ref, 4096);
        assert_eq!(val_ref - bs, 4096);
        assert_eq!(val_ref - bs_ref, 4096);
        assert_eq!(bs - 100u64, 3996);
        assert_eq!(bs - &100u64, 3996);
        assert_eq!(bs_ref - 100u64, 3996);
        assert_eq!(bs_ref - &100u64, 3996);

        // Mul
        assert_eq!(bs * 3u64, 12288);
        assert_eq!(bs * &3u64, 12288);
        assert_eq!(bs_ref * 3u64, 12288);
        assert_eq!(bs_ref * &3u64, 12288);
        assert_eq!(3u64 * bs, 12288);
        assert_eq!(3u64 * bs_ref, 12288);
        assert_eq!(&3u64 * bs, 12288);
        assert_eq!(&3u64 * bs_ref, 12288);

        // Div
        assert_eq!(val / bs, 2);
        assert_eq!(val / bs_ref, 2);
        assert_eq!(val_ref / bs, 2);
        assert_eq!(val_ref / bs_ref, 2);

        // Rem
        assert_eq!(5000u64 % bs, 904);
        assert_eq!(5000u64 % bs_ref, 904);
        assert_eq!(&5000u64 % bs, 904);
        assert_eq!(&5000u64 % bs_ref, 904);
    }

    #[fuchsia::test]
    fn test_assign_ops() {
        let bs = BlockSize::SIZE_4KIB;

        let mut v = 100u64;
        v += bs;
        assert_eq!(v, 4196);
        v += &bs;
        assert_eq!(v, 8292);

        v -= bs;
        assert_eq!(v, 4196);
        v -= &bs;
        assert_eq!(v, 100);

        let mut blocks = 3u64;
        blocks *= bs;
        assert_eq!(blocks, 12288);
        let mut blocks2 = 3u64;
        blocks2 *= &bs;
        assert_eq!(blocks2, 12288);

        let mut bytes = 12288u64;
        bytes /= bs;
        assert_eq!(bytes, 3);
        let mut bytes2 = 12288u64;
        bytes2 /= &bs;
        assert_eq!(bytes2, 3);

        let mut rem = 5000u64;
        rem %= bs;
        assert_eq!(rem, 904);
        let mut rem2 = 5000u64;
        rem2 %= &bs;
        assert_eq!(rem2, 904);
    }

    #[fuchsia::test]
    fn test_comparisons_and_hash() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        #[derive(Copy, Clone, Debug, Eq, PartialEq)]
        struct Custom4KiBSpec;
        impl BlockSizeSpec for Custom4KiBSpec {
            fn size(self) -> u64 {
                4096
            }
            fn mask(self) -> u64 {
                4095
            }
            fn shift(self) -> u32 {
                12
            }
        }
        let custom_bs = GenericBlockSize(Custom4KiBSpec);

        let bs1 = BlockSize::SIZE_512B;
        let bs2 = BlockSize::SIZE_4KIB;

        assert!(bs1 < bs2);
        assert!(bs2 > bs1);
        assert_eq!(bs1.min(bs2), bs1);
        assert_eq!(bs1.max(bs2), bs2);

        // Cross-spec comparisons and Hash consistency
        assert_eq!(bs2, custom_bs);
        assert_eq!(custom_bs, bs2);
        assert!(bs1 < custom_bs);
        assert!(custom_bs > bs1);

        fn hash_val<H: Hash>(v: &H) -> u64 {
            let mut hasher = DefaultHasher::new();
            v.hash(&mut hasher);
            hasher.finish()
        }
        assert_eq!(hash_val(&bs2), hash_val(&custom_bs));
        assert_ne!(hash_val(&bs1), hash_val(&bs2));

        assert_eq!(bs2, 4096u64);
        assert_eq!(bs2, &4096u64);
        assert_eq!(&bs2, 4096u64);
        assert_eq!(&bs2, &4096u64);
        assert_eq!(4096u64, bs2);
        assert_eq!(4096u64, &bs2);
        assert_eq!(&4096u64, bs2);
        assert_eq!(&4096u64, &bs2);

        assert!(bs2 > 512u64);
        assert!(bs2 > &512u64);
        assert!(&bs2 > 512u64);
        assert!(&bs2 > &512u64);
        assert!(512u64 < bs2);
        assert!(512u64 < &bs2);
        assert!(&512u64 < bs2);
        assert!(&512u64 < &bs2);
    }

    #[cfg(debug_assertions)]
    #[fuchsia::test]
    #[should_panic(expected = "attempt to multiply with overflow")]
    fn test_mul_overflow() {
        let _ = BlockSize::SIZE_4KIB * (1u64 << 60);
    }

    #[cfg(debug_assertions)]
    #[fuchsia::test]
    #[should_panic(expected = "attempt to multiply with overflow")]
    fn test_mul_assign_overflow() {
        let mut v = 1u64 << 60;
        v *= BlockSize::SIZE_4KIB;
    }

    #[fuchsia::test]
    fn test_formatting() {
        let bs = BlockSize::SIZE_4KIB;
        assert_eq!(format!("{}", bs), "4096");
        assert_eq!(format!("{:x}", bs), "1000");
        assert_eq!(format!("{:X}", bs), "1000");
        assert_eq!(format!("{:b}", bs), "1000000000000");
        assert_eq!(format!("{:o}", bs), "10000");
    }

    #[fuchsia::test]
    fn test_block_size_one() {
        let block_size = BlockSize::new(1).unwrap();

        assert_eq!(block_size.align_up(0), Some(0));
        assert_eq!(block_size.align_up(20), Some(20));
        assert_eq!(block_size.align_up(u64::MAX), Some(u64::MAX));

        assert_eq!(block_size.align_down(0), 0);
        assert_eq!(block_size.align_down(20), 20);
        assert_eq!(block_size.align_down(u64::MAX), u64::MAX);

        assert_eq!(block_size * 0, 0);
        assert_eq!(block_size * 20, 20);
        assert_eq!(block_size * u64::MAX, u64::MAX);

        assert_eq!(0 / block_size, 0);
        assert_eq!(20 / block_size, 20);
        assert_eq!(u64::MAX / block_size, u64::MAX);

        assert_eq!(0 % block_size, 0);
        assert_eq!(20 % block_size, 0);
        assert_eq!(u64::MAX % block_size, 0);
    }

    #[fuchsia::test]
    fn test_block_size_max_4gib() {
        let bs = BlockSize::from_u64(1 << 32).unwrap();
        assert_eq!(bs.get(), 1 << 32);
        assert_eq!(bs.mask(), u32::MAX as u64);
        assert_eq!(bs.shift(), 32);

        assert!(bs.is_aligned(0u64));
        assert!(bs.is_aligned(1u64 << 32));
        assert!(!bs.is_aligned((1u64 << 32) - 1));

        assert_eq!(bs.align_up(1), Some(1 << 32));
        assert_eq!(bs.align_down((1 << 32) + 123), 1 << 32);
        assert_eq!(bs * 3u64, 3 << 32);
        assert_eq!((3u64 << 32) / bs, 3);
        assert_eq!(((3u64 << 32) + 55) % bs, 55);
    }
}
