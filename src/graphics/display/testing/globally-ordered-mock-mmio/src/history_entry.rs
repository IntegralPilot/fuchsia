// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Records of successfully executed MMIO operations.

use crate::operation::CompletedOp;
use crate::source_info::ExpectationSourceInfo;

/// A record of a successfully executed MMIO operation.
///
/// Kept in the scoreboard's execution history buffer to format the recent
/// execution trace when an expectation mismatch or unexpected access failure
/// occurs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The operation, as it was completed.
    pub op: CompletedOp,

    /// Position, in the scoreboard's expectation list, of the expectation that
    /// completed this operation.
    ///
    /// Repeats across consecutive entries when a
    /// [`crate::expectation::ExpectationPattern::WhileMatches`] expectation
    /// completes several operations. Recording the expectation position (rather
    /// than the position in the history) keeps the indexes in a failure report
    /// monotonic and comparable with the indexes of pending expectations.
    pub expectation_index: usize,

    /// Source metadata of the expectation that completed this operation.
    pub source_info: ExpectationSourceInfo,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_access::{AccessData, AccessSize};
    use crate::source_info::RegisterRef;
    use core::panic::Location;

    #[fuchsia::test]
    fn test_history_entry_fields() {
        let register = RegisterRef::new("registers::Test");
        let entry = HistoryEntry {
            op: CompletedOp::Read { offset: 0x10, data: AccessData::new(AccessSize::U32, 0x100) },
            expectation_index: 3,
            source_info: ExpectationSourceInfo::new(Some(register), Location::caller()),
        };

        assert_eq!(entry.expectation_index, 3);
        assert_eq!(entry.source_info.register, Some(RegisterRef::new("registers::Test")));
    }

    #[fuchsia::test]
    fn test_history_entry_without_register() {
        let entry = HistoryEntry {
            op: CompletedOp::WriteBarrier,
            expectation_index: 0,
            source_info: ExpectationSourceInfo::new(None, Location::caller()),
        };

        assert_eq!(entry.source_info.register, None);
    }
}
