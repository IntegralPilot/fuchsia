// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use mmio::region::{MmioRegion, UnsafeMmio};
use std::sync::Arc;

use crate::data_access::AccessSize;
use crate::scoreboard::Scoreboard;
use crate::trace_builder::ExpectedTraceBuilder;

/// Mock [`UnsafeMmio`] implementation that routes all calls to [`Scoreboard`].
///
/// Guaranteed to implement [`Send`] and [`Sync`].
#[derive(Clone, Debug)]
pub struct MockUnsafeMmio {
    scoreboard: Arc<Scoreboard>,
}

impl UnsafeMmio for MockUnsafeMmio {
    fn len(&self) -> usize {
        self.scoreboard.region_size_bytes()
    }

    fn align_offset(&self, _align: usize) -> usize {
        // The mock models an abstract contiguous MMIO region starting at byte offset 0.
        // Unlike physical VMO mappings, no page-boundary alignment adjustment is needed.
        0
    }

    unsafe fn load8_unchecked(&self, offset: usize) -> u8 {
        let value = self.scoreboard.load(offset, AccessSize::U8);
        u8::try_from(value).expect("Scoreboard guarantees the value fits in the access size")
    }

    unsafe fn load16_unchecked(&self, offset: usize) -> u16 {
        let value = self.scoreboard.load(offset, AccessSize::U16);
        u16::try_from(value).expect("Scoreboard guarantees the value fits in the access size")
    }

    unsafe fn load32_unchecked(&self, offset: usize) -> u32 {
        let value = self.scoreboard.load(offset, AccessSize::U32);
        u32::try_from(value).expect("Scoreboard guarantees the value fits in the access size")
    }

    unsafe fn load64_unchecked(&self, offset: usize) -> u64 {
        self.scoreboard.load(offset, AccessSize::U64)
    }

    unsafe fn store8_unchecked(&self, offset: usize, value: u8) {
        self.scoreboard.store(offset, AccessSize::U8, u64::from(value));
    }

    unsafe fn store16_unchecked(&self, offset: usize, value: u16) {
        self.scoreboard.store(offset, AccessSize::U16, u64::from(value));
    }

    unsafe fn store32_unchecked(&self, offset: usize, value: u32) {
        self.scoreboard.store(offset, AccessSize::U32, u64::from(value));
    }

    unsafe fn store64_unchecked(&self, offset: usize, value: u64) {
        self.scoreboard.store(offset, AccessSize::U64, value);
    }

    fn write_barrier(&self) {
        self.scoreboard.write_barrier();
    }
}

/// [`MmioRegion`] produced by [`MockMmioRegionBuilder`].
///
/// The underlying [`UnsafeMmio`] implementation is guaranteed to implement
/// [`Send`] and [`Sync`].
pub type MockMmioRegion = MmioRegion<MockUnsafeMmio, Arc<MockUnsafeMmio>>;

/// Strict thread-safe mock (interaction testing) for MMIO ([`MmioRegion`]).
///
/// The [`MmioRegion`] implementation imposes a global ordering on MMIO accesses
/// performed by multiple threads. If the code under test uses a non-thread-safe
/// [`MmioRegion`] implementation, the mock introduces cross-thread
/// synchronization points that do not exist in the code under test.
///
/// Expectations are verified after this builder and all the [`MockMmioRegion`]s
/// it produced are dropped. Tests do not need to keep the builder alive while
/// the code under test uses the regions. Conversely, dropping the builder does
/// not verify expectations while a region is still in use.
#[derive(Debug)]
pub struct MockMmioRegionBuilder {
    scoreboard: Arc<Scoreboard>,
}

impl MockMmioRegionBuilder {
    /// Creates a new mock MMIO region covering `region_size_bytes` bytes.
    pub fn new(region_size_bytes: usize) -> Self {
        Self { scoreboard: Arc::new(Scoreboard::new(region_size_bytes)) }
    }

    /// Returns the configured region size in bytes.
    pub fn region_size_bytes(&self) -> usize {
        self.scoreboard.region_size_bytes()
    }

    /// Posts expectations using the closure trace builder.
    ///
    /// [`ExpectedTraceBuilder`] implements the expectation API provided to the
    /// closure.
    ///
    /// Expectations posted by multiple calls are appended to a single global
    /// expectation list, in call order.
    pub fn expect<F: FnOnce(&mut ExpectedTraceBuilder<'_>)>(&self, f: F) {
        let mut builder = ExpectedTraceBuilder::new(&self.scoreboard);
        f(&mut builder);
    }

    /// Returns a [`MockUnsafeMmio`] directly connected to this mock's scoreboard.
    fn mock_unsafe_mmio(&self) -> MockUnsafeMmio {
        MockUnsafeMmio { scoreboard: Arc::clone(&self.scoreboard) }
    }

    /// Constructs a splittable, sendable [`MmioRegion`] wrapping this mock.
    ///
    /// The returned region, and any sub-region split off of it, keeps the
    /// expectation list alive. Expectations are verified when the last region
    /// and this builder are dropped.
    pub fn build(&self) -> MockMmioRegion {
        MmioRegion::new(self.mock_unsafe_mmio()).into_split_send()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmio::Mmio;

    #[fuchsia::test]
    fn test_mock_mmio_region_size_bytes() {
        let mock_builder = MockMmioRegionBuilder::new(0x2000);
        assert_eq!(mock_builder.region_size_bytes(), 0x2000);
    }

    #[fuchsia::test]
    fn test_mock_mmio_built_region_len() {
        let mock_builder = MockMmioRegionBuilder::new(0x2000);
        let mmio = mock_builder.build();
        assert_eq!(mmio.len(), 0x2000);
    }

    #[fuchsia::test]
    fn test_mock_mmio_built_region_operations() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.at(0x00).write8(0x12);
            t.at(0x02).write16(0x3456);
            t.at(0x04).write32(0x789a_bcde);
            t.at(0x08).write64(0x0123_4567_89ab_cdef);
            t.at(0x10).read8(0xaa);
            t.at(0x12).read16(0xbbcc);
            t.at(0x14).read32(0xddee_ff00);
            t.at(0x18).read64(0x1122_3344_5566_7788);
            t.write_barrier();
        });
        let mut mmio = mock_builder.build();
        mmio.store8(0x00, 0x12);
        mmio.store16(0x02, 0x3456);
        mmio.store32(0x04, 0x789a_bcde);
        mmio.store64(0x08, 0x0123_4567_89ab_cdef);
        assert_eq!(mmio.load8(0x10), 0xaa);
        assert_eq!(mmio.load16(0x12), 0xbbcc);
        assert_eq!(mmio.load32(0x14), 0xddee_ff00);
        assert_eq!(mmio.load64(0x18), 0x1122_3344_5566_7788);
        mmio.write_barrier();
    }

    #[fuchsia::test]
    fn test_mock_mmio_built_region_split_off() {
        use mmio::MmioSplit;

        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.at(0x0).write32(0x1111);
            t.at(0x100).write32(0x2222);
        });
        let mut mmio = mock_builder.build();
        let mut sub1 = mmio.split_off(0x100);
        assert_eq!(sub1.len(), 0x100);
        assert_eq!(mmio.len(), 0xf00);

        sub1.store32(0x0, 0x1111);
        mmio.store32(0x0, 0x2222);
    }

    #[fuchsia::test]
    #[should_panic(expected = "UNRETIRED MMIO EXPECTATIONS")]
    fn test_dropping_builder_and_region_verifies_expectations() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.at(0x0).read8(0x42);
        });
        let mmio = mock_builder.build();
        drop(mmio);
        drop(mock_builder);
    }

    #[fuchsia::test]
    fn test_dropping_builder_alone_does_not_verify_expectations() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.at(0x0).read8(0x42);
        });
        let mmio = mock_builder.build();

        // The builder is dropped while the code under test still holds a region.
        // Verification must be deferred until the region is dropped.
        drop(mock_builder);

        assert_eq!(mmio.load8(0x0), 0x42);
    }

    #[fuchsia::test]
    fn test_mock_unsafe_mmio_align_offset() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        let mock_unsafe_mmio = mock_builder.mock_unsafe_mmio();
        assert_eq!(mock_unsafe_mmio.align_offset(4), 0);
        assert_eq!(mock_unsafe_mmio.align_offset(8), 0);
    }

    #[fuchsia::test]
    fn test_mock_unsafe_mmio_load_unchecked() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.at(0x0).read8(0x12);
            t.at(0x2).read16(0x3456);
            t.at(0x4).read32(0x789a_bcde);
            t.at(0x8).read64(0x0123_4567_89ab_cdef);
        });
        let mock_unsafe = mock_builder.mock_unsafe_mmio();
        unsafe {
            assert_eq!(mock_unsafe.load8_unchecked(0x0), 0x12);
            assert_eq!(mock_unsafe.load16_unchecked(0x2), 0x3456);
            assert_eq!(mock_unsafe.load32_unchecked(0x4), 0x789a_bcde);
            assert_eq!(mock_unsafe.load64_unchecked(0x8), 0x0123_4567_89ab_cdef);
        }
    }

    #[fuchsia::test]
    fn test_mock_unsafe_mmio_store_unchecked() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.at(0x0).write8(0x12);
            t.at(0x2).write16(0x3456);
            t.at(0x4).write32(0x789a_bcde);
            t.at(0x8).write64(0x0123_4567_89ab_cdef);
        });
        let mock_unsafe = mock_builder.mock_unsafe_mmio();
        unsafe {
            mock_unsafe.store8_unchecked(0x0, 0x12);
            mock_unsafe.store16_unchecked(0x2, 0x3456);
            mock_unsafe.store32_unchecked(0x4, 0x789a_bcde);
            mock_unsafe.store64_unchecked(0x8, 0x0123_4567_89ab_cdef);
        }
    }

    #[fuchsia::test]
    fn test_mock_unsafe_mmio_write_barrier() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.write_barrier();
        });
        let mock_unsafe = mock_builder.mock_unsafe_mmio();
        mock_unsafe.write_barrier();
    }

    #[fuchsia::test]
    fn test_mock_unsafe_mmio_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MockUnsafeMmio>();

        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        let mock_unsafe_mmio = mock_builder.mock_unsafe_mmio();
        let handle = std::thread::spawn(move || {
            assert_eq!(mock_unsafe_mmio.len(), 0x1000);
        });
        handle.join().unwrap();
    }

    #[fuchsia::test]
    fn test_mock_unsafe_mmio_sync() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<MockUnsafeMmio>();

        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        let mock_unsafe_mmio = mock_builder.mock_unsafe_mmio();
        std::thread::scope(|s| {
            s.spawn(|| {
                assert_eq!(mock_unsafe_mmio.len(), 0x1000);
            });
            assert_eq!(mock_unsafe_mmio.len(), 0x1000);
        });
    }
}
