// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Types used to model a single MMIO operation.
//!
//! Accesses are modeled in two phases:
//! * [`RequestedOp`]: An operation initiated by the driver under test (e.g. `load32(0x10)`).
//!   Reads only specify offset and access size; writes specify offset and data.
//! * [`CompletedOp`]: An expected operation recorded by the test author. For reads, this
//!   provides the simulated return value to be passed back to the driver.

use crate::data_access::{AccessData, AccessSize};

/// Describes an operation requested by an MMIO method call.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RequestedOp {
    /// Read an element of size `size` from `offset`.
    Read { offset: usize, size: AccessSize },

    /// Write `data` at `offset`.
    Write { offset: usize, data: AccessData },

    /// Memory write barrier ([`mmio::Mmio::write_barrier`]).
    WriteBarrier,
}

/// Describes a completed MMIO operation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CompletedOp {
    /// Read `data` from `offset`.
    Read { offset: usize, data: AccessData },

    /// Write `data` at `offset`.
    Write { offset: usize, data: AccessData },

    /// Memory write barrier.
    WriteBarrier,
}

impl CompletedOp {
    /// Returns the byte offset and transferred data if this is an MMIO data access.
    ///
    /// Returns [`None`] for operations that do not touch the MMIO region, such
    /// as memory barriers.
    pub fn data_access(self) -> Option<(usize, AccessData)> {
        match self {
            Self::Read { offset, data } | Self::Write { offset, data } => Some((offset, data)),
            Self::WriteBarrier => None,
        }
    }

    /// Returns true if `requested` meets this expectation.
    ///
    /// Match conditions:
    ///
    /// * Reads: the requested byte offset and access width are equal
    /// * Writes: the byte offset, access width, and written value are all equal
    /// * Write barriers: no extra conditions
    pub fn matches(self, requested: RequestedOp) -> bool {
        match (self, requested) {
            (
                Self::Read { offset: completed_offset, data },
                RequestedOp::Read { offset: requested_offset, size },
            ) => completed_offset == requested_offset && data.size() == size,
            (
                Self::Write { offset: completed_offset, data: completed_data },
                RequestedOp::Write { offset: requested_offset, data: requested_data },
            ) => completed_offset == requested_offset && completed_data == requested_data,
            (Self::WriteBarrier, RequestedOp::WriteBarrier) => true,
            _ => false,
        }
    }

    /// Returns the value returned by the MMIO call requesting this operation.
    ///
    /// Reads return the data value. Writes and memory barriers return 0.
    pub fn return_value(self) -> u64 {
        match self {
            Self::Read { data, .. } => data.value(),
            Self::Write { .. } | Self::WriteBarrier => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_completed_op_data_access() {
        let read_data = AccessData::new(AccessSize::U32, 0x1234);
        let read_op = CompletedOp::Read { offset: 0x10, data: read_data };
        assert_eq!(read_op.data_access(), Some((0x10, read_data)));

        let write_data = AccessData::new(AccessSize::U16, 0x5678);
        let write_op = CompletedOp::Write { offset: 0x20, data: write_data };
        assert_eq!(write_op.data_access(), Some((0x20, write_data)));

        assert_eq!(CompletedOp::WriteBarrier.data_access(), None);
    }

    #[fuchsia::test]
    fn test_completed_read_matches_requested_read() {
        let completed =
            CompletedOp::Read { offset: 0x10, data: AccessData::new(AccessSize::U32, 0x1234_5678) };

        // Matches when offset and size match, regardless of completed data value.
        assert!(completed.matches(RequestedOp::Read { offset: 0x10, size: AccessSize::U32 }));

        // Does not match when offset differs.
        assert!(!completed.matches(RequestedOp::Read { offset: 0x14, size: AccessSize::U32 }));

        // Does not match when size differs.
        assert!(!completed.matches(RequestedOp::Read { offset: 0x10, size: AccessSize::U16 }));

        // Does not match other operation types.
        assert!(!completed.matches(RequestedOp::Write {
            offset: 0x10,
            data: AccessData::new(AccessSize::U32, 0x1234_5678),
        }));
        assert!(!completed.matches(RequestedOp::WriteBarrier));
    }

    #[fuchsia::test]
    fn test_completed_write_matches_requested_write() {
        let completed = CompletedOp::Write {
            offset: 0x20,
            data: AccessData::new(AccessSize::U32, 0xdead_beef),
        };

        // Matches when offset, size, and value match.
        assert!(completed.matches(RequestedOp::Write {
            offset: 0x20,
            data: AccessData::new(AccessSize::U32, 0xdead_beef),
        }));

        // Does not match when offset differs.
        assert!(!completed.matches(RequestedOp::Write {
            offset: 0x24,
            data: AccessData::new(AccessSize::U32, 0xdead_beef),
        }));

        // Does not match when value differs.
        assert!(!completed.matches(RequestedOp::Write {
            offset: 0x20,
            data: AccessData::new(AccessSize::U32, 0xcafe_babe),
        }));

        // Does not match when size differs.
        assert!(!completed.matches(RequestedOp::Write {
            offset: 0x20,
            data: AccessData::new(AccessSize::U16, 0xbeef),
        }));

        // Does not match other operation types.
        assert!(!completed.matches(RequestedOp::Read { offset: 0x20, size: AccessSize::U32 }));
        assert!(!completed.matches(RequestedOp::WriteBarrier));
    }

    #[fuchsia::test]
    fn test_completed_write_barrier_matches_requested_write_barrier() {
        let completed = CompletedOp::WriteBarrier;

        assert!(completed.matches(RequestedOp::WriteBarrier));
        assert!(!completed.matches(RequestedOp::Read { offset: 0, size: AccessSize::U8 }));
        assert!(
            !completed.matches(RequestedOp::Write {
                offset: 0,
                data: AccessData::new(AccessSize::U8, 0),
            })
        );
    }

    #[fuchsia::test]
    fn test_completed_op_read_return_value() {
        for (size, value) in [
            (AccessSize::U8, 0x5a),
            (AccessSize::U16, 0x1234),
            (AccessSize::U32, 0x1234_5678),
            (AccessSize::U64, 0x0123_4567_89ab_cdef),
        ] {
            let read_op = CompletedOp::Read { offset: 0x10, data: AccessData::new(size, value) };
            assert_eq!(read_op.return_value(), value);
        }
    }

    #[fuchsia::test]
    fn test_completed_op_write_and_barrier_return_zero() {
        let write_op =
            CompletedOp::Write { offset: 0x10, data: AccessData::new(AccessSize::U32, 0x5678) };
        assert_eq!(write_op.return_value(), 0);
        assert_eq!(CompletedOp::WriteBarrier.return_value(), 0);
    }
}
