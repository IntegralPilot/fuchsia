// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The expectation-declaring API handed to `expect()` closures.

use core::panic::Location;
use mmio::{
    IndexedRegister, ReadableIndexedRegister, ReadableRegister, WritableIndexedRegister,
    WritableRegister,
};

use crate::data_access::{AccessData, AccessSize};
use crate::expectation::ExpectationPattern;
use crate::mmio_operand_value;
use crate::operation::CompletedOp;
use crate::scoreboard::Scoreboard;
use crate::source_info::{ExpectationSourceInfo, RegisterRef};

/// Builder for recording expected MMIO access sequences within a closure.
///
/// Adopts a receiver-based closure architecture (`mock.expect(|t| { ...
/// })`) where expectations are posted to the [`Scoreboard`] via standard Rust
/// method calls.
///
/// The syntax is intended to work seamlessly with standard Rust tooling
/// (`rustfmt`, `rust-analyzer`, go-to-definition, and autocomplete). The raw
/// offset methods are spelled out one per access width, rather than generated
/// by a macro, so that autocomplete lists them.
///
/// Each expectation method automatically captures the caller source location
/// ([`Location::caller`]). This information is included in the diagnostic
/// information produced by test failures.
#[derive(Debug)]
pub struct ExpectedTraceBuilder<'a> {
    scoreboard: &'a Scoreboard,
}

impl<'a> ExpectedTraceBuilder<'a> {
    pub(crate) fn new(scoreboard: &'a Scoreboard) -> Self {
        Self { scoreboard }
    }

    // -------------------------------------------------------------------------
    // Typed register methods. Offset and width are derived from R.
    //
    // The methods take `&mut self` even though the scoreboard uses interior
    // mutability, so that a trace closure cannot interleave expectations from
    // two aliasing builders.
    // -------------------------------------------------------------------------

    /// Posts an expectation for a read from register `R`, returning `value`.
    ///
    /// The register byte offset (`R::OFFSET`) and access width
    /// (`AccessSize::of::<R::Value>()`) are automatically deduced from the
    /// [`ReadableRegister`] definition.
    #[track_caller]
    pub fn read<R: ReadableRegister>(&mut self, value: R::Value) {
        self.post_read(
            R::OFFSET,
            Self::register_data::<R::Value>(value),
            Some(RegisterRef::new(std::any::type_name::<R>())),
            Location::caller(),
        );
    }

    /// Posts an expectation for a write of `value` to register `R`.
    ///
    /// The register byte offset (`R::OFFSET`) and access width
    /// (`AccessSize::of::<R::Value>()`) are automatically deduced from the
    /// [`WritableRegister`] definition.
    #[track_caller]
    pub fn write<R: WritableRegister>(&mut self, value: R::Value) {
        self.post_write(
            R::OFFSET,
            Self::register_data::<R::Value>(value),
            Some(RegisterRef::new(std::any::type_name::<R>())),
            Location::caller(),
        );
    }

    /// Posts an expectation for reading `value` from register `R` at `index`.
    ///
    /// The byte offset is `R::BASE_OFFSET + index * R::STRIDE`.
    ///
    /// # Panics
    ///
    /// Panics if `index` is greater than or equal to `R::COUNT`.
    #[track_caller]
    pub fn read_indexed<R: ReadableIndexedRegister>(&mut self, index: usize, value: R::Value) {
        self.post_read(
            Self::indexed_offset::<R>(index),
            Self::register_data::<R::Value>(value),
            Some(RegisterRef::indexed(std::any::type_name::<R>(), index)),
            Location::caller(),
        );
    }

    /// Posts an expectation for a write of `value` to register `R` at `index`.
    ///
    /// The byte offset is `R::BASE_OFFSET + index * R::STRIDE`.
    ///
    /// # Panics
    ///
    /// Panics if `index` is greater than or equal to `R::COUNT`.
    #[track_caller]
    pub fn write_indexed<R: WritableIndexedRegister>(&mut self, index: usize, value: R::Value) {
        self.post_write(
            Self::indexed_offset::<R>(index),
            Self::register_data::<R::Value>(value),
            Some(RegisterRef::indexed(std::any::type_name::<R>(), index)),
            Location::caller(),
        );
    }

    /// Posts an expectation for successive reads to register `R` returning `values`.
    ///
    /// Models deterministic polling sequences (e.g. status polling until ready):
    /// * Requires the driver to read each value in the sequence at least once
    ///   in order.
    /// * Once the sequence is exhausted, subsequent reads return the last
    ///   value.
    /// * The expectation automatically retires when the driver performs the
    ///   next non-matching access (e.g., writing the next configuration
    ///   register).
    ///
    /// # Panics
    ///
    /// Panics if `values` is empty.
    #[track_caller]
    pub fn poll<R: ReadableRegister>(&mut self, values: impl IntoIterator<Item = R::Value>) {
        let values = Self::raw_values::<R::Value>(values, "poll()");
        self.post_poll(
            R::OFFSET,
            &values,
            AccessSize::of::<R::Value>(),
            Some(RegisterRef::new(std::any::type_name::<R>())),
            Location::caller(),
        );
    }

    /// Posts an expectation for reading `busy_value` from register `R` one or more times.
    ///
    /// Models indefinite polling retry loops (e.g. testing driver timeout and error recovery):
    ///
    /// * Requires the driver to read `busy_value` at least once.
    /// * Returns `busy_value` for any subsequent reads while the driver executes its retry loop.
    /// * The expectation automatically retires when the driver ceases polling and initiates
    ///   timeout recovery (e.g., writing a reset command).
    #[track_caller]
    pub fn poll_indefinitely<R: ReadableRegister>(&mut self, busy_value: R::Value) {
        self.post_poll_indefinitely(
            R::OFFSET,
            Self::register_data::<R::Value>(busy_value),
            Some(RegisterRef::new(std::any::type_name::<R>())),
            Location::caller(),
        );
    }

    /// Posts an expectation for reading values` from register `R` at `index`.
    ///
    /// The byte offset is `R::BASE_OFFSET + index * R::STRIDE`.
    ///
    /// # Panics
    ///
    /// Panics if `index` is greater than or equal to `R::COUNT`, or if `values` is empty.
    #[track_caller]
    pub fn poll_indexed<R: ReadableIndexedRegister>(
        &mut self,
        index: usize,
        values: impl IntoIterator<Item = R::Value>,
    ) {
        let offset = Self::indexed_offset::<R>(index);
        let values = Self::raw_values::<R::Value>(values, "poll_indexed()");
        self.post_poll(
            offset,
            &values,
            AccessSize::of::<R::Value>(),
            Some(RegisterRef::indexed(std::any::type_name::<R>(), index)),
            Location::caller(),
        );
    }

    /// Posts an expectation for reading `busy_value` from register `R` at `index` one or more times.
    ///
    /// The byte offset is `R::BASE_OFFSET + index * R::STRIDE`.
    ///
    /// # Panics
    ///
    /// Panics if `index` is greater than or equal to `R::COUNT`.
    #[track_caller]
    pub fn poll_indefinitely_indexed<R: ReadableIndexedRegister>(
        &mut self,
        index: usize,
        busy_value: R::Value,
    ) {
        self.post_poll_indefinitely(
            Self::indexed_offset::<R>(index),
            Self::register_data::<R::Value>(busy_value),
            Some(RegisterRef::indexed(std::any::type_name::<R>(), index)),
            Location::caller(),
        );
    }

    // -------------------------------------------------------------------------
    // Raw offset methods, with an explicit access width.
    //
    // Ergonomic fallbacks for interacting with un-typed register spaces or raw
    // offsets where formal Register definitions are not available.
    // -------------------------------------------------------------------------

    /// Selects a raw byte `offset` for declaring expectations on un-typed MMIO
    /// registers.
    ///
    /// Returns a [`RawOffsetTraceBuilder`] providing width-annotated access
    /// methods (`read8`..`read64`, `write8`..`write64`, `poll8`..`poll64`, and
    /// `poll_indefinitely8`..`poll_indefinitely64`).
    pub fn at(&mut self, offset: usize) -> RawOffsetTraceBuilder<'a, '_> {
        RawOffsetTraceBuilder { trace_builder: self, offset }
    }

    /// Posts an expectation for an MMIO memory write barrier ([`mmio::Mmio::write_barrier`]).
    #[track_caller]
    pub fn write_barrier(&mut self) {
        self.scoreboard.post_expectation(
            ExpectationPattern::MustMatchOnce(CompletedOp::WriteBarrier),
            ExpectationSourceInfo::new(None, Location::caller()),
        );
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    /// Returns the byte offset of element `index` of the indexed register `R`.
    ///
    /// # Panics
    ///
    /// Panics if `index` is greater than or equal to `R::COUNT`.
    #[track_caller]
    fn indexed_offset<R: IndexedRegister>(index: usize) -> usize {
        assert!(index < R::COUNT, "Register index {index} out of bounds (count: {})", R::COUNT);
        R::BASE_OFFSET + index * R::STRIDE
    }

    /// Returns the data transferred by an access to a register holding a `T`.
    fn register_data<T: mmio::MmioOperand>(value: T) -> AccessData {
        AccessData::new(AccessSize::of::<T>(), mmio_operand_value::mmio_operand_to_u64(value))
    }

    /// Collects the return values of a polling sequence.
    ///
    /// # Panics
    ///
    /// Panics if `values` is empty, naming `caller` in the message.
    #[track_caller]
    fn raw_values<T: mmio::MmioOperand>(
        values: impl IntoIterator<Item = T>,
        caller: &str,
    ) -> Vec<u64> {
        let values: Vec<u64> =
            values.into_iter().map(mmio_operand_value::mmio_operand_to_u64).collect();
        assert!(!values.is_empty(), "{caller} requires at least one return value");
        values
    }

    #[track_caller]
    fn post_read(
        &mut self,
        offset: usize,
        data: AccessData,
        register: Option<RegisterRef>,
        location: &'static Location<'static>,
    ) {
        self.scoreboard.post_expectation(
            ExpectationPattern::MustMatchOnce(CompletedOp::Read { offset, data }),
            ExpectationSourceInfo::new(register, location),
        );
    }

    #[track_caller]
    fn post_write(
        &mut self,
        offset: usize,
        data: AccessData,
        register: Option<RegisterRef>,
        location: &'static Location<'static>,
    ) {
        self.scoreboard.post_expectation(
            ExpectationPattern::MustMatchOnce(CompletedOp::Write { offset, data }),
            ExpectationSourceInfo::new(register, location),
        );
    }

    /// Expands a polling sequence into one expectation per value, plus a
    /// trailing repeating expectation for the last value.
    ///
    /// # Panics
    ///
    /// Panics if `values` is empty.
    #[track_caller]
    fn post_poll(
        &mut self,
        offset: usize,
        values: &[u64],
        size: AccessSize,
        register: Option<RegisterRef>,
        location: &'static Location<'static>,
    ) {
        let (&last_value, _) = values.split_last().expect("values must not be empty");

        // Sequence polling requires each value in the sequence to be read at
        // least once, in the specified order.
        for &value in values {
            self.post_read(offset, AccessData::new(size, value), register.clone(), location);
        }

        // Once the sequence completes, subsequent reads return the last value
        // until the driver performs a different access, which retires this
        // repeating expectation.
        self.scoreboard.post_expectation(
            ExpectationPattern::WhileMatches(CompletedOp::Read {
                offset,
                data: AccessData::new(size, last_value),
            }),
            ExpectationSourceInfo::new(register, location),
        );
    }

    /// Expands an indefinite poll into one required read plus a repeating
    /// expectation for the same value.
    #[track_caller]
    fn post_poll_indefinitely(
        &mut self,
        offset: usize,
        data: AccessData,
        register: Option<RegisterRef>,
        location: &'static Location<'static>,
    ) {
        // The driver must read the busy value at least once before timing out.
        self.post_read(offset, data, register.clone(), location);

        // Subsequent reads keep returning the busy value until the driver
        // ceases polling and initiates recovery (e.g. reset), which retires
        // this repeating expectation.
        self.scoreboard.post_expectation(
            ExpectationPattern::WhileMatches(CompletedOp::Read { offset, data }),
            ExpectationSourceInfo::new(register, location),
        );
    }
}

/// Builder for recording expected MMIO accesses at a fixed raw byte offset.
///
/// Returned by [`ExpectedTraceBuilder::at`].
///
/// Operations on [`RawOffsetTraceBuilder`] return `&mut Self`, allowing
/// read-modify-write sequences at the same offset to be chained:
///
/// ```
/// # use globally_ordered_mock_mmio::MockMmioRegionBuilder;
/// # use mmio::Mmio;
/// let mock_builder = MockMmioRegionBuilder::new(0x1000);
/// mock_builder.expect(|t| {
///     t.at(0x08).read32(0x0000_0001).write32(0x0000_0003);
/// });
/// let mut mmio = mock_builder.build();
/// assert_eq!(mmio.load32(0x08), 0x0000_0001);
/// mmio.store32(0x08, 0x0000_0003);
/// ```
#[derive(Debug)]
pub struct RawOffsetTraceBuilder<'a, 'b> {
    trace_builder: &'b mut ExpectedTraceBuilder<'a>,
    offset: usize,
}

impl<'a, 'b> RawOffsetTraceBuilder<'a, 'b> {
    /// Posts an expectation for an 8-bit read from this offset, returning `value`.
    #[track_caller]
    pub fn read8(&mut self, value: u8) -> &mut Self {
        let data = AccessData::new(AccessSize::U8, u64::from(value));
        self.trace_builder.post_read(self.offset, data, None, Location::caller());
        self
    }

    /// Posts an expectation for an 8-bit write of `value` to this offset.
    #[track_caller]
    pub fn write8(&mut self, value: u8) -> &mut Self {
        let data = AccessData::new(AccessSize::U8, u64::from(value));
        self.trace_builder.post_write(self.offset, data, None, Location::caller());
        self
    }

    /// Posts an expectation for a 16-bit read from this offset, returning `value`.
    #[track_caller]
    pub fn read16(&mut self, value: u16) -> &mut Self {
        let data = AccessData::new(AccessSize::U16, u64::from(value));
        self.trace_builder.post_read(self.offset, data, None, Location::caller());
        self
    }

    /// Posts an expectation for a 16-bit write of `value` to this offset.
    #[track_caller]
    pub fn write16(&mut self, value: u16) -> &mut Self {
        let data = AccessData::new(AccessSize::U16, u64::from(value));
        self.trace_builder.post_write(self.offset, data, None, Location::caller());
        self
    }

    /// Posts an expectation for a 32-bit read from this offset, returning `value`.
    #[track_caller]
    pub fn read32(&mut self, value: u32) -> &mut Self {
        let data = AccessData::new(AccessSize::U32, u64::from(value));
        self.trace_builder.post_read(self.offset, data, None, Location::caller());
        self
    }

    /// Posts an expectation for a 32-bit write of `value` to this offset.
    #[track_caller]
    pub fn write32(&mut self, value: u32) -> &mut Self {
        let data = AccessData::new(AccessSize::U32, u64::from(value));
        self.trace_builder.post_write(self.offset, data, None, Location::caller());
        self
    }

    /// Posts an expectation for a 64-bit read from this offset, returning `value`.
    #[track_caller]
    pub fn read64(&mut self, value: u64) -> &mut Self {
        let data = AccessData::new(AccessSize::U64, value);
        self.trace_builder.post_read(self.offset, data, None, Location::caller());
        self
    }

    /// Posts an expectation for a 64-bit write of `value` to this offset.
    #[track_caller]
    pub fn write64(&mut self, value: u64) -> &mut Self {
        let data = AccessData::new(AccessSize::U64, value);
        self.trace_builder.post_write(self.offset, data, None, Location::caller());
        self
    }

    /// Posts an expectation for successive 8-bit reads from this offset returning `values`.
    ///
    /// See [`ExpectedTraceBuilder::poll`] for the polling semantics.
    ///
    /// # Panics
    ///
    /// Panics if `values` is empty.
    #[track_caller]
    pub fn poll8(&mut self, values: impl IntoIterator<Item = u8>) -> &mut Self {
        let values = ExpectedTraceBuilder::raw_values::<u8>(values, "poll8()");
        self.trace_builder.post_poll(
            self.offset,
            &values,
            AccessSize::U8,
            None,
            Location::caller(),
        );
        self
    }

    /// Posts an expectation for successive 16-bit reads from this offset returning `values`.
    ///
    /// See [`ExpectedTraceBuilder::poll`] for the polling semantics.
    ///
    /// # Panics
    ///
    /// Panics if `values` is empty.
    #[track_caller]
    pub fn poll16(&mut self, values: impl IntoIterator<Item = u16>) -> &mut Self {
        let values = ExpectedTraceBuilder::raw_values::<u16>(values, "poll16()");
        self.trace_builder.post_poll(
            self.offset,
            &values,
            AccessSize::U16,
            None,
            Location::caller(),
        );
        self
    }

    /// Posts an expectation for successive 32-bit reads from this offset returning `values`.
    ///
    /// See [`ExpectedTraceBuilder::poll`] for the polling semantics.
    ///
    /// # Panics
    ///
    /// Panics if `values` is empty.
    #[track_caller]
    pub fn poll32(&mut self, values: impl IntoIterator<Item = u32>) -> &mut Self {
        let values = ExpectedTraceBuilder::raw_values::<u32>(values, "poll32()");
        self.trace_builder.post_poll(
            self.offset,
            &values,
            AccessSize::U32,
            None,
            Location::caller(),
        );
        self
    }

    /// Posts an expectation for successive 64-bit reads from this offset returning `values`.
    ///
    /// See [`ExpectedTraceBuilder::poll`] for the polling semantics.
    ///
    /// # Panics
    ///
    /// Panics if `values` is empty.
    #[track_caller]
    pub fn poll64(&mut self, values: impl IntoIterator<Item = u64>) -> &mut Self {
        let values = ExpectedTraceBuilder::raw_values::<u64>(values, "poll64()");
        self.trace_builder.post_poll(
            self.offset,
            &values,
            AccessSize::U64,
            None,
            Location::caller(),
        );
        self
    }

    /// Posts an expectation for indefinite 8-bit reads from this offset returning `busy_value`.
    ///
    /// See [`ExpectedTraceBuilder::poll_indefinitely`] for the polling semantics.
    #[track_caller]
    pub fn poll_indefinitely8(&mut self, busy_value: u8) -> &mut Self {
        let data = AccessData::new(AccessSize::U8, u64::from(busy_value));
        self.trace_builder.post_poll_indefinitely(self.offset, data, None, Location::caller());
        self
    }

    /// Posts an expectation for indefinite 16-bit reads from this offset returning `busy_value`.
    ///
    /// See [`ExpectedTraceBuilder::poll_indefinitely`] for the polling semantics.
    #[track_caller]
    pub fn poll_indefinitely16(&mut self, busy_value: u16) -> &mut Self {
        let data = AccessData::new(AccessSize::U16, u64::from(busy_value));
        self.trace_builder.post_poll_indefinitely(self.offset, data, None, Location::caller());
        self
    }

    /// Posts an expectation for indefinite 32-bit reads from this offset returning `busy_value`.
    ///
    /// See [`ExpectedTraceBuilder::poll_indefinitely`] for the polling semantics.
    #[track_caller]
    pub fn poll_indefinitely32(&mut self, busy_value: u32) -> &mut Self {
        let data = AccessData::new(AccessSize::U32, u64::from(busy_value));
        self.trace_builder.post_poll_indefinitely(self.offset, data, None, Location::caller());
        self
    }

    /// Posts an expectation for indefinite 64-bit reads from this offset returning `busy_value`.
    ///
    /// See [`ExpectedTraceBuilder::poll_indefinitely`] for the polling semantics.
    #[track_caller]
    pub fn poll_indefinitely64(&mut self, busy_value: u64) -> &mut Self {
        let data = AccessData::new(AccessSize::U64, busy_value);
        self.trace_builder.post_poll_indefinitely(self.offset, data, None, Location::caller());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmio::Register;

    mod registers {
        use mmio::register;

        register! {
            #[register(offset = 0x10, mode = RW)]
            pub struct Test(u32);

            #[indexed_register(offset = 0x100, stride = 4, count = 4, mode = RW)]
            pub struct TestIndexed(u32);
        }
    }

    #[fuchsia::test]
    fn test_raw_reads_and_writes() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.at(0x00).write8(0x12);
        builder.at(0x01).read8(0x34);
        builder.at(0x02).write16(0x1234);
        builder.at(0x04).read16(0x5678);
        builder.at(0x08).write32(0x1234_5678);
        builder.at(0x0c).read32(0x9abc_def0);
        builder.at(0x10).write64(0x0123_4567_89ab_cdef);
        builder.at(0x18).read64(0xfedc_ba98_7654_3210);

        scoreboard.store(0x00, AccessSize::U8, 0x12);
        assert_eq!(scoreboard.load(0x1, AccessSize::U8), 0x34);
        scoreboard.store(0x02, AccessSize::U16, 0x1234);
        assert_eq!(scoreboard.load(0x4, AccessSize::U16), 0x5678);
        scoreboard.store(0x08, AccessSize::U32, 0x1234_5678);
        assert_eq!(scoreboard.load(0xc, AccessSize::U32), 0x9abc_def0);
        scoreboard.store(0x10, AccessSize::U64, 0x0123_4567_89ab_cdef);
        assert_eq!(scoreboard.load(0x18, AccessSize::U64), 0xfedc_ba98_7654_3210);
    }

    #[fuchsia::test]
    fn test_raw_offset_rmw_chaining() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.at(0x8).read32(0x0000_0001).write32(0x0000_0003);

        assert_eq!(scoreboard.load(0x8, AccessSize::U32), 0x0000_0001);
        scoreboard.store(0x8, AccessSize::U32, 0x0000_0003);
    }

    #[fuchsia::test]
    fn test_write_barrier() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.write_barrier();

        scoreboard.write_barrier();
    }

    #[fuchsia::test]
    fn test_raw_polls() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.at(0x0).poll8([1u8, 2u8]);
        builder.at(0x2).poll16([0x100u16, 0x200u16]);
        builder.at(0x4).poll32([1u32, 2u32]);
        builder.at(0x8).poll64([10u64, 20u64]);
        builder.at(0x1).write8(3);

        // The last value of each sequence repeats until the next access.
        assert_eq!(scoreboard.load(0x0, AccessSize::U8), 1);
        assert_eq!(scoreboard.load(0x0, AccessSize::U8), 2);
        assert_eq!(scoreboard.load(0x0, AccessSize::U8), 2);
        assert_eq!(scoreboard.load(0x2, AccessSize::U16), 0x100);
        assert_eq!(scoreboard.load(0x2, AccessSize::U16), 0x200);
        assert_eq!(scoreboard.load(0x4, AccessSize::U32), 1);
        assert_eq!(scoreboard.load(0x4, AccessSize::U32), 2);
        assert_eq!(scoreboard.load(0x8, AccessSize::U64), 10);
        assert_eq!(scoreboard.load(0x8, AccessSize::U64), 20);
        scoreboard.store(0x1, AccessSize::U8, 3);
    }

    #[fuchsia::test]
    fn test_raw_indefinite_polls() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.at(0x0).poll_indefinitely8(0xaa);
        builder.at(0x2).poll_indefinitely16(0xaaaa);
        builder.at(0x4).poll_indefinitely32(0xaaaa_bbbb);
        builder.at(0x8).poll_indefinitely64(0xaaaa_bbbb_cccc_dddd);
        builder.at(0x1).write8(0xff);

        assert_eq!(scoreboard.load(0x0, AccessSize::U8), 0xaa);
        assert_eq!(scoreboard.load(0x0, AccessSize::U8), 0xaa);
        assert_eq!(scoreboard.load(0x2, AccessSize::U16), 0xaaaa);
        assert_eq!(scoreboard.load(0x4, AccessSize::U32), 0xaaaa_bbbb);
        assert_eq!(scoreboard.load(0x8, AccessSize::U64), 0xaaaa_bbbb_cccc_dddd);
        scoreboard.store(0x1, AccessSize::U8, 0xff);
    }

    #[fuchsia::test]
    fn test_typed_read_write() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.write::<registers::Test>(0x1234);
        builder.read::<registers::Test>(0x5678);

        scoreboard.store(registers::Test::OFFSET, AccessSize::U32, 0x1234);
        assert_eq!(scoreboard.load(registers::Test::OFFSET, AccessSize::U32), 0x5678);
    }

    #[fuchsia::test]
    fn test_typed_poll() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.poll::<registers::Test>([1, 2]);
        builder.write::<registers::Test>(3);

        assert_eq!(scoreboard.load(registers::Test::OFFSET, AccessSize::U32), 1);
        assert_eq!(scoreboard.load(registers::Test::OFFSET, AccessSize::U32), 2);
        scoreboard.store(registers::Test::OFFSET, AccessSize::U32, 3);
    }

    #[fuchsia::test]
    fn test_typed_poll_indefinitely() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.poll_indefinitely::<registers::Test>(0xaa);
        builder.write::<registers::Test>(0xbb);

        assert_eq!(scoreboard.load(registers::Test::OFFSET, AccessSize::U32), 0xaa);
        scoreboard.store(registers::Test::OFFSET, AccessSize::U32, 0xbb);
    }

    #[fuchsia::test]
    fn test_indexed_read_write() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.write_indexed::<registers::TestIndexed>(0, 0x10);
        builder.read_indexed::<registers::TestIndexed>(1, 0x20);

        scoreboard.store(0x100, AccessSize::U32, 0x10);
        assert_eq!(scoreboard.load(0x104, AccessSize::U32), 0x20);
    }

    #[fuchsia::test]
    fn test_indexed_poll() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.poll_indexed::<registers::TestIndexed>(2, [1, 2]);
        builder.write_indexed::<registers::TestIndexed>(3, 3);

        assert_eq!(scoreboard.load(0x108, AccessSize::U32), 1);
        assert_eq!(scoreboard.load(0x108, AccessSize::U32), 2);
        scoreboard.store(0x10c, AccessSize::U32, 3);
    }

    #[fuchsia::test]
    fn test_indexed_poll_indefinitely() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.poll_indefinitely_indexed::<registers::TestIndexed>(1, 0x55);
        builder.write_indexed::<registers::TestIndexed>(2, 0x66);

        assert_eq!(scoreboard.load(0x104, AccessSize::U32), 0x55);
        scoreboard.store(0x108, AccessSize::U32, 0x66);
    }

    /// Checks that the register index reaches the failure report.
    ///
    /// The report's layout is covered by the tests in the `formatting` module.
    /// Fuchsia tests are compiled with `panic = "abort"`, so the panic message
    /// can only be matched via `#[should_panic]`.
    #[fuchsia::test]
    #[should_panic(expected = "TestIndexed[2] (0x0108)")]
    fn test_indexed_expectations_record_the_index() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.read_indexed::<registers::TestIndexed>(2, 0x30);

        // Mismatched access. The report must name the index, not just the offset.
        let _ = scoreboard.load(0x0, AccessSize::U32);
    }

    #[fuchsia::test]
    #[should_panic(expected = "Register index 4 out of bounds")]
    fn test_read_indexed_out_of_bounds_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.read_indexed::<registers::TestIndexed>(4, 0x10);
    }

    #[fuchsia::test]
    #[should_panic(expected = "Register index 4 out of bounds")]
    fn test_write_indexed_out_of_bounds_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.write_indexed::<registers::TestIndexed>(4, 0x10);
    }

    #[fuchsia::test]
    #[should_panic(expected = "Register index 4 out of bounds")]
    fn test_poll_indexed_out_of_bounds_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.poll_indexed::<registers::TestIndexed>(4, [1]);
    }

    #[fuchsia::test]
    #[should_panic(expected = "Register index 4 out of bounds")]
    fn test_poll_indefinitely_indexed_out_of_bounds_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.poll_indefinitely_indexed::<registers::TestIndexed>(4, 1);
    }

    #[fuchsia::test]
    #[should_panic(expected = "poll() requires at least one return value")]
    fn test_poll_empty_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.poll::<registers::Test>([]);
    }

    #[fuchsia::test]
    #[should_panic(expected = "poll_indexed() requires at least one return value")]
    fn test_poll_indexed_empty_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.poll_indexed::<registers::TestIndexed>(0, []);
    }

    #[fuchsia::test]
    #[should_panic(expected = "poll8() requires at least one return value")]
    fn test_poll8_empty_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.at(0).poll8([]);
    }

    #[fuchsia::test]
    #[should_panic(expected = "poll16() requires at least one return value")]
    fn test_poll16_empty_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.at(0).poll16([]);
    }

    #[fuchsia::test]
    #[should_panic(expected = "poll32() requires at least one return value")]
    fn test_poll32_empty_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.at(0).poll32([]);
    }

    #[fuchsia::test]
    #[should_panic(expected = "poll64() requires at least one return value")]
    fn test_poll64_empty_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        let mut builder = ExpectedTraceBuilder::new(&scoreboard);
        builder.at(0).poll64([]);
    }
}
