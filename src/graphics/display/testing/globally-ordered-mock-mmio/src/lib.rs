// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Globally-ordered mock for MMIO driver testing in Rust.
//!
//! Built on top of the MMIO region abstractions in
//! `//sdk/lib/driver/mmio/rust`, this crate provides a high-density, readable,
//! trace-like API for testing drivers that interact with memory-mapped I/O
//! (MMIO).
//!
//! # Usage guide
//!
//! ## Initialize the test double
//!
//! Create a [`MockMmioRegionBuilder`] instance covering the expected MMIO
//! region size in bytes.
//!
//! ```
//! use globally_ordered_mock_mmio::MockMmioRegionBuilder;
//!
//! let mock_builder = MockMmioRegionBuilder::new(0x1000);
//! ```
//!
//! ## Declare expectations
//!
//! Record expected MMIO operations using the [`ExpectedTraceBuilder`] API.
//!
//! ### Typed register operations
//!
//! Use [`ExpectedTraceBuilder::read`] and [`ExpectedTraceBuilder::write`] for
//! expectations on register types defined with [`mmio::register!`]. The methods
//! deduce byte offsets and access widths.
//!
//! ```
//! # mod registers {
//! #     use mmio::register;
//! #     register! {
//! #         #[register(offset = 0x00, mode = RO)]
//! #         pub struct Status(u32);
//! #         #[register(offset = 0x04, mode = WO)]
//! #         pub struct Command(u32);
//! #     }
//! # }
//! use globally_ordered_mock_mmio::MockMmioRegionBuilder;
//! use mmio::{ReadableRegister, WritableRegister};
//!
//! let mock_builder = MockMmioRegionBuilder::new(0x1000);
//! mock_builder.expect(|t| {
//!     // Expect a read from Status returning 0x0000_0001
//!     t.read::<registers::Status>(0x0000_0001);
//!
//!     // Expect a write to Command with value 0x0000_0002
//!     t.write::<registers::Command>(0x0000_0002);
//! });
//!
//! // Performed by the code under test.
//! let mut mmio = mock_builder.build();
//! assert_eq!(registers::Status::read(&mmio).0, 0x0000_0001);
//! registers::Command(0x0000_0002).write(&mut mmio);
//! ```
//!
//! ### Indexed register operations
//!
//! Use [`ExpectedTraceBuilder::read_indexed`] and
//! [`ExpectedTraceBuilder::write_indexed`] for expectations on indexed register
//! types defined with `#[indexed_register(...)]`.
//!
//! ```
//! # mod registers {
//! #     use mmio::register;
//! #     register! {
//! #         #[indexed_register(offset = 0x100, stride = 4, count = 4, mode = RW)]
//! #         pub struct IndexedData(u32);
//! #     }
//! # }
//! use globally_ordered_mock_mmio::MockMmioRegionBuilder;
//! use mmio::{ReadableIndexedRegister, WritableIndexedRegister};
//!
//! let mock_builder = MockMmioRegionBuilder::new(0x1000);
//! mock_builder.expect(|t| {
//!     // Expect a read from IndexedData at index 2 returning 0x30
//!     t.read_indexed::<registers::IndexedData>(2, 0x30);
//!
//!     // Expect a write to IndexedData at index 3 with value 0x40
//!     t.write_indexed::<registers::IndexedData>(3, 0x40);
//! });
//!
//! // Performed by the code under test.
//! let mut mmio = mock_builder.build();
//! assert_eq!(registers::IndexedData::read_index(&mmio, 2).0, 0x30);
//! registers::IndexedData(0x40).write_index(&mut mmio, 3);
//! ```
//!
//! ### Successful polling
//!
//! Use [`ExpectedTraceBuilder::poll`] to model a status register that changes
//! value across successive reads.
//!
//! ```
//! # mod registers {
//! #     use mmio::register;
//! #     register! {
//! #         #[register(offset = 0x00, mode = RO)]
//! #         pub struct Status(u32);
//! #     }
//! # }
//! use globally_ordered_mock_mmio::MockMmioRegionBuilder;
//! use mmio::ReadableRegister;
//!
//! let mock_builder = MockMmioRegionBuilder::new(0x1000);
//! mock_builder.expect(|t| {
//!     // First two reads return 0x01 (busy); subsequent reads return 0x02 (ready)
//!     t.poll::<registers::Status>([0x0000_0001, 0x0000_0001, 0x0000_0002]);
//! });
//!
//! // Performed by the code under test.
//! let mmio = mock_builder.build();
//! while registers::Status::read(&mmio).0 != 0x0000_0002 {}
//! ```
//!
//! ### Timed out polling
//!
//! Use [`ExpectedTraceBuilder::poll_indefinitely`] to model hardware remaining
//! busy until the driver times out and executes recovery.
//!
//! ```
//! # mod registers {
//! #     use mmio::register;
//! #     register! {
//! #         #[register(offset = 0x00, mode = RO)]
//! #         pub struct Status(u32);
//! #         #[register(offset = 0x04, mode = WO)]
//! #         pub struct Command(u32);
//! #     }
//! # }
//! use globally_ordered_mock_mmio::MockMmioRegionBuilder;
//! use mmio::{ReadableRegister, WritableRegister};
//!
//! let mock_builder = MockMmioRegionBuilder::new(0x1000);
//! mock_builder.expect(|t| {
//!     // Returns 0x01 indefinitely until the driver executes a non-matching access
//!     t.poll_indefinitely::<registers::Status>(0x0000_0001);
//!     t.write::<registers::Command>(0x0000_0001); // Reset command after timeout
//! });
//!
//! // Performed by the code under test.
//! let mut mmio = mock_builder.build();
//! for _ in 0..10 {
//!     assert_eq!(registers::Status::read(&mmio).0, 0x0000_0001);
//! }
//! registers::Command(0x0000_0001).write(&mut mmio);
//! ```
//!
//! ### Raw numeric offsets
//!
//! When the reference test data is a trace containing raw MMIO offsets and
//! values, pass the raw offset to [`ExpectedTraceBuilder::at`] and express the
//! operation and value using the [`RawOffsetTraceBuilder`] methods, such as
//! [`RawOffsetTraceBuilder::read32`] and [`RawOffsetTraceBuilder::write32`].
//!
//! ```
//! use globally_ordered_mock_mmio::MockMmioRegionBuilder;
//! use mmio::Mmio;
//!
//! let mock_builder = MockMmioRegionBuilder::new(0x1000);
//! mock_builder.expect(|t| {
//!     t.at(0x0).read8(0x12);
//!     t.at(0x1).write8(0x34);
//!     t.at(0x8).read32(0x1234_5678);
//!     t.at(0xc).write32(0x9abc_def0);
//! });
//!
//! // Performed by the code under test.
//! let mut mmio = mock_builder.build();
//! assert_eq!(mmio.load8(0x0), 0x12);
//! mmio.store8(0x1, 0x34);
//! assert_eq!(mmio.load32(0x8), 0x1234_5678);
//! mmio.store32(0xc, 0x9abc_def0);
//! ```
//!
//! ### Memory barriers
//!
//! Use [`ExpectedTraceBuilder::write_barrier`] to expect a write barrier
//! between two MMIO accesses.
//!
//! ```
//! use globally_ordered_mock_mmio::MockMmioRegionBuilder;
//! use mmio::Mmio;
//!
//! let mock_builder = MockMmioRegionBuilder::new(0x1000);
//! mock_builder.expect(|t| {
//!     t.at(0x0).write32(0x1234_5678);
//!     t.write_barrier();
//!     t.at(0x4).write32(0x9abc_def0);
//! });
//!
//! // Performed by the code under test.
//! let mut mmio = mock_builder.build();
//! mmio.store32(0x0, 0x1234_5678);
//! mmio.write_barrier();
//! mmio.store32(0x4, 0x9abc_def0);
//! ```
//!
//! ## Obtain and use the MMIO region
//!
//! Call [`MockMmioRegionBuilder::build`] to obtain a [`MockMmioRegion`], which
//! is an [`mmio::region::MmioRegion`] implementation.
//!
//! The returned region implements [`mmio::Mmio`] and [`mmio::MmioSplit`],
//! allowing driver code to split sub-regions across multiple threads. Accesses
//! from all threads are checked against a single global expectation list, in
//! the order in which the accesses reach the mock.
//!
//! The mock does not impose any ordering on accesses issued by different
//! threads. Tests that exercise concurrent driver code must establish their own
//! ordering, for example by having the threads synchronize on a channel.
//!
//! ## Verification and teardown
//!
//! Expectations are verified on [`Drop`], after the [`MockMmioRegionBuilder`]
//! and all the [`MockMmioRegion`]s it produced are dropped. Any unretired
//! expectation results in a test failure. No explicit verification call is
//! needed.
//!
//! Verification is skipped while the thread is already panicking, so that the
//! failure that caused the panic is not masked.

// The module structure is private to the crate.

mod data_access;
mod expectation;
mod formatting;
mod history_entry;
mod mmio_operand_value;
mod mock_builder;
mod operation;
mod scoreboard;
mod source_info;
mod trace_builder;

// The public API is exposed below. Keep to a minimum.

pub use mmio_operand_value::mmio_operand_to_u64;
pub use mock_builder::{MockMmioRegion, MockMmioRegionBuilder};
pub use trace_builder::{ExpectedTraceBuilder, RawOffsetTraceBuilder};

#[cfg(test)]
mod tests {
    use super::*;
    use mmio::{
        Mmio, MmioSplit, ReadableIndexedRegister, ReadableRegister, WritableIndexedRegister,
        WritableRegister,
    };

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
    fn test_public_api_raw_accesses() {
        let mock_builder = MockMmioRegionBuilder::new(0x100);
        mock_builder.expect(|t| {
            t.at(0x0).write8(0x42);
            t.at(0x1).read8(0x5a);
        });
        let mut mmio = mock_builder.build();
        mmio.store8(0x0, 0x42);
        assert_eq!(mmio.load8(0x1), 0x5a);
    }

    #[fuchsia::test]
    fn test_public_api_typed_registers() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.read::<registers::Test>(0x1234);
            t.write::<registers::Test>(0x5678);
        });
        let mut mmio = mock_builder.build();
        assert_eq!(registers::Test::read(&mmio).value(), 0x1234);
        registers::Test(0x5678).write(&mut mmio);
    }

    #[fuchsia::test]
    fn test_public_api_indexed_registers() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.read_indexed::<registers::TestIndexed>(2, 0x30);
            t.write_indexed::<registers::TestIndexed>(3, 0x40);
        });
        let mut mmio = mock_builder.build();
        assert_eq!(registers::TestIndexed::read_index(&mmio, 2).value(), 0x30);
        registers::TestIndexed(0x40).write_index(&mut mmio, 3);
    }

    #[fuchsia::test]
    fn test_public_api_polling_sequences() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.poll::<registers::Test>([0x1, 0x1, 0x2]);
        });
        let mmio = mock_builder.build();
        assert_eq!(registers::Test::read(&mmio).value(), 0x1);
        assert_eq!(registers::Test::read(&mmio).value(), 0x1);
        assert_eq!(registers::Test::read(&mmio).value(), 0x2);
    }

    #[fuchsia::test]
    fn test_public_api_indefinite_poll_and_recovery() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.poll_indefinitely::<registers::Test>(0x1);
            t.write::<registers::Test>(0x2);
        });
        let mut mmio = mock_builder.build();
        for _ in 0..5 {
            assert_eq!(registers::Test::read(&mmio).value(), 0x1);
        }
        registers::Test(0x2).write(&mut mmio);
    }

    #[fuchsia::test]
    fn test_public_api_mmio_operand_to_u64() {
        assert_eq!(mmio_operand_to_u64(0x12u8), 0x12);
        assert_eq!(mmio_operand_to_u64(0x1234u16), 0x1234);
        assert_eq!(mmio_operand_to_u64(0x1234_5678u32), 0x1234_5678);
        assert_eq!(mmio_operand_to_u64(0x1234_5678_9abc_def0u64), 0x1234_5678_9abc_def0);
    }

    #[fuchsia::test]
    fn test_public_api_write_barrier_and_split() {
        let mock_builder = MockMmioRegionBuilder::new(0x1000);
        mock_builder.expect(|t| {
            t.write_barrier();
            t.at(0x0).write8(0x11);
            t.at(0x20).write8(0x22);
        });
        let mut mmio = mock_builder.build();
        mmio.write_barrier();
        let mut r1 = mmio.split_off(0x20);
        let mut r2 = mmio.split_off(0x20);
        r1.store8(0x0, 0x11);
        r2.store8(0x0, 0x22);
    }
}
