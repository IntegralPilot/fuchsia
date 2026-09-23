// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Converts an [`MmioOperand`] to an integer value.

use mmio::{Mmio, MmioError, MmioOperand};

/// Numeric type guaranteed to be large enough to store any MMIO operand value.
pub type MmioOperandValue = u64;

/// Returns the numerical value for any [`MmioOperand`].
pub fn mmio_operand_to_u64<T: MmioOperand>(operand_value: T) -> MmioOperandValue {
    let mut value_extractor = MmioWriteOperandValueExtractor::new();
    T::try_store(&mut value_extractor, 0, operand_value)
        .expect("MmioWriteOperandValueExtractor failed to intercept the write operation");
    value_extractor.operand_value()
}

/// [`Mmio`] implementation that captures the integer value
/// behind an [`MmioOperand`].
///
/// The implementation is limited to the methods used by
/// [`MmioOperand::try_store`]. All other methods panic.
struct MmioWriteOperandValueExtractor {
    operand_value: MmioOperandValue,
}

impl MmioWriteOperandValueExtractor {
    /// Creates an extractor that has not captured any value yet.
    fn new() -> Self {
        Self { operand_value: 0 }
    }

    /// Returns the value captured by the most recent store operation.
    ///
    /// Returns zero if no store operation was performed.
    fn operand_value(&self) -> MmioOperandValue {
        self.operand_value
    }
}

impl Mmio for MmioWriteOperandValueExtractor {
    fn len(&self) -> usize {
        core::mem::size_of::<MmioOperandValue>()
    }

    fn align_offset(&self, _align: usize) -> usize {
        0
    }

    fn try_load8(&self, _offset: usize) -> Result<u8, MmioError> {
        unimplemented!("MmioOperand::try_store never performs load operations")
    }

    fn try_load16(&self, _offset: usize) -> Result<u16, MmioError> {
        unimplemented!("MmioOperand::try_store never performs load operations")
    }

    fn try_load32(&self, _offset: usize) -> Result<u32, MmioError> {
        unimplemented!("MmioOperand::try_store never performs load operations")
    }

    fn try_load64(&self, _offset: usize) -> Result<u64, MmioError> {
        unimplemented!("MmioOperand::try_store never performs load operations")
    }

    fn try_store8(&mut self, _offset: usize, value: u8) -> Result<(), MmioError> {
        self.operand_value = u64::from(value);
        Ok(())
    }

    fn try_store16(&mut self, _offset: usize, value: u16) -> Result<(), MmioError> {
        self.operand_value = u64::from(value);
        Ok(())
    }

    fn try_store32(&mut self, _offset: usize, value: u32) -> Result<(), MmioError> {
        self.operand_value = u64::from(value);
        Ok(())
    }

    fn try_store64(&mut self, _offset: usize, value: u64) -> Result<(), MmioError> {
        self.operand_value = value;
        Ok(())
    }

    fn write_barrier(&self) {
        unimplemented!("MmioOperand::try_store never issues write barriers")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_mmio_operand_to_u64_u8() {
        assert_eq!(mmio_operand_to_u64(0x5au8), 0x5a);
    }

    #[fuchsia::test]
    fn test_mmio_operand_to_u64_u16() {
        assert_eq!(mmio_operand_to_u64(0x1234u16), 0x1234);
    }

    #[fuchsia::test]
    fn test_mmio_operand_to_u64_u32() {
        assert_eq!(mmio_operand_to_u64(0x1234_5678u32), 0x1234_5678);
    }

    #[fuchsia::test]
    fn test_mmio_operand_to_u64_u64() {
        assert_eq!(mmio_operand_to_u64(0x1234_5678_9abc_def0u64), 0x1234_5678_9abc_def0);
    }

    #[fuchsia::test]
    fn test_extractor_captures_stores_of_every_size() {
        let mut extractor = MmioWriteOperandValueExtractor::new();
        assert_eq!(extractor.operand_value(), 0);

        extractor.try_store8(0x10, 0x42).unwrap();
        assert_eq!(extractor.operand_value(), 0x42);

        extractor.try_store16(0x20, 0x1234).unwrap();
        assert_eq!(extractor.operand_value(), 0x1234);

        extractor.try_store32(0x30, 0x1234_5678).unwrap();
        assert_eq!(extractor.operand_value(), 0x1234_5678);

        extractor.try_store64(0x40, 0x0123_4567_89ab_cdef).unwrap();
        assert_eq!(extractor.operand_value(), 0x0123_4567_89ab_cdef);
    }

    #[fuchsia::test]
    fn test_extractor_covers_the_largest_operand() {
        // `MmioOperand::try_store()` rejects offsets past the reported length.
        // The extractor must accept the offset used by `mmio_operand_to_u64()`
        // for the largest supported operand.
        let extractor = MmioWriteOperandValueExtractor::new();
        assert_eq!(extractor.len(), core::mem::size_of::<u64>());
        assert_eq!(extractor.align_offset(4), 0);
    }
}
