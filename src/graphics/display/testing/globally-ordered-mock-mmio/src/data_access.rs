// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Types used to model the data transferred by a single MMIO read or write.

use std::fmt;
use std::mem::size_of;

/// The input/output size for an MMIO access (read/write operation).
///
/// Each type implementing [`mmio::MmioOperand`] maps to one member.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AccessSize {
    /// [`u8`] operand -- 8 bits / 1 byte.
    U8 = 1,

    /// [`u16`] operand -- 16 bits / 2 bytes.
    U16 = 2,

    /// [`u32`] operand -- 32 bits / 4 bytes.
    U32 = 4,

    /// [`u64`] operand -- 64 bits / 8 bytes.
    U64 = 8,
}

impl AccessSize {
    /// Returns the [`AccessSize`] representing the given [`mmio::MmioOperand`] type.
    ///
    /// # Panics
    ///
    /// Panics if `T` is not one of the operand types covered by [`AccessSize`].
    pub const fn of<T: mmio::MmioOperand>() -> Self {
        match size_of::<T>() {
            1 => AccessSize::U8,
            2 => AccessSize::U16,
            4 => AccessSize::U32,
            8 => AccessSize::U64,
            _ => panic!("Uncovered MmioOperand trait implementation"),
        }
    }

    /// Number of bytes transferred in an access of this size.
    pub const fn size_bytes(self) -> usize {
        self as usize
    }

    /// Number of bits transferred in an access of this size.
    pub const fn bits(self) -> usize {
        self.size_bytes() * 8
    }

    /// True iff the given value can be read/written by a memory access of this size.
    pub const fn is_valid_value(self, value: u64) -> bool {
        match self {
            // Shifting by `bits()` would overflow for the widest access.
            AccessSize::U64 => true,
            _ => value >> self.bits() == 0,
        }
    }
}

impl fmt::Display for AccessSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-bit", self.bits())
    }
}

/// The value read / written by an MMIO read / write operation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AccessData {
    /// Only the lower [`AccessSize::bits`] bits are used. Unused bits are set to zero.
    value: u64,

    /// The access width of the operation.
    size: AccessSize,
}

impl AccessData {
    /// The value transferred by an access of width `access_size`.
    ///
    /// # Panics
    ///
    /// Panics if `value` exceeds the maximum value representable in `access_size`.
    pub const fn new(access_size: AccessSize, value: u64) -> AccessData {
        assert!(access_size.is_valid_value(value), "value does not fit in the MMIO access width");
        AccessData { size: access_size, value }
    }

    pub const fn size(self) -> AccessSize {
        self.size
    }

    pub const fn value(self) -> u64 {
        self.value
    }
}

impl fmt::Display for AccessData {
    /// Renders the value in hexadecimal, zero-padded to the access width.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Two hexadecimal digits per byte.
        write!(f, "{:#0width$x}", self.value, width = self.size.size_bytes() * 2 + 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_access_size_size_bytes() {
        assert_eq!(AccessSize::U8.size_bytes(), 1);
        assert_eq!(AccessSize::U16.size_bytes(), 2);
        assert_eq!(AccessSize::U32.size_bytes(), 4);
        assert_eq!(AccessSize::U64.size_bytes(), 8);
    }

    #[fuchsia::test]
    fn test_access_size_bits() {
        assert_eq!(AccessSize::U8.bits(), 8);
        assert_eq!(AccessSize::U16.bits(), 16);
        assert_eq!(AccessSize::U32.bits(), 32);
        assert_eq!(AccessSize::U64.bits(), 64);
    }

    #[fuchsia::test]
    fn test_access_size_of() {
        assert_eq!(AccessSize::of::<u8>(), AccessSize::U8);
        assert_eq!(AccessSize::of::<u16>(), AccessSize::U16);
        assert_eq!(AccessSize::of::<u32>(), AccessSize::U32);
        assert_eq!(AccessSize::of::<u64>(), AccessSize::U64);
    }

    #[fuchsia::test]
    fn test_access_size_display() {
        assert_eq!(format!("{}", AccessSize::U8), "8-bit");
        assert_eq!(format!("{}", AccessSize::U16), "16-bit");
        assert_eq!(format!("{}", AccessSize::U32), "32-bit");
        assert_eq!(format!("{}", AccessSize::U64), "64-bit");
    }

    #[fuchsia::test]
    fn test_is_valid_value_u8() {
        assert!(AccessSize::U8.is_valid_value(0));
        assert!(AccessSize::U8.is_valid_value(0xff));
        assert!(!AccessSize::U8.is_valid_value(0x100));
    }

    #[fuchsia::test]
    fn test_is_valid_value_u16() {
        assert!(AccessSize::U16.is_valid_value(0));
        assert!(AccessSize::U16.is_valid_value(0xffff));
        assert!(!AccessSize::U16.is_valid_value(0x1_0000));
    }

    #[fuchsia::test]
    fn test_is_valid_value_u32() {
        assert!(AccessSize::U32.is_valid_value(0));
        assert!(AccessSize::U32.is_valid_value(0xffff_ffff));
        assert!(!AccessSize::U32.is_valid_value(0x1_0000_0000));
    }

    #[fuchsia::test]
    fn test_is_valid_value_u64() {
        assert!(AccessSize::U64.is_valid_value(0));
        assert!(AccessSize::U64.is_valid_value(u64::MAX));
    }

    #[fuchsia::test]
    fn test_access_data_accessors() {
        let data = AccessData::new(AccessSize::U32, 0x1234_5678);
        assert_eq!(data.size(), AccessSize::U32);
        assert_eq!(data.value(), 0x1234_5678);
    }

    #[fuchsia::test]
    fn test_access_data_display_pads_to_the_access_width() {
        assert_eq!(format!("{}", AccessData::new(AccessSize::U8, 0x5)), "0x05");
        assert_eq!(format!("{}", AccessData::new(AccessSize::U16, 0x5)), "0x0005");
        assert_eq!(format!("{}", AccessData::new(AccessSize::U32, 0x1234)), "0x00001234");
        assert_eq!(
            format!("{}", AccessData::new(AccessSize::U64, 0x0123_4567_89ab_cdef)),
            "0x0123456789abcdef"
        );
    }

    #[fuchsia::test]
    #[should_panic(expected = "value does not fit in the MMIO access width")]
    fn test_access_data_u8_overflow() {
        let _ = AccessData::new(AccessSize::U8, 0x100);
    }

    #[fuchsia::test]
    #[should_panic(expected = "value does not fit in the MMIO access width")]
    fn test_access_data_u16_overflow() {
        let _ = AccessData::new(AccessSize::U16, 0x1_0000);
    }

    #[fuchsia::test]
    #[should_panic(expected = "value does not fit in the MMIO access width")]
    fn test_access_data_u32_overflow() {
        let _ = AccessData::new(AccessSize::U32, 0x1_0000_0000);
    }
}
