// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Expectations: the rules that incoming MMIO operations are matched against.

use crate::operation::{CompletedOp, RequestedOp};
use crate::source_info::ExpectationSourceInfo;

/// Rules governing how an expectation matches and retires bus accesses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpectationPattern {
    /// Fails on a request that does not match a completed operation.
    ///
    /// A matching request is completed and retires the expectation. A
    /// non-matching request fails the test case.
    MustMatchOnce(CompletedOp),

    /// Matching requests are completed and do not retire the expectation.
    ///
    /// A mismatching request retires the expectation without completing the
    /// operation, allowing the scoreboard matching loop to continue against the
    /// next expectation in the list.
    WhileMatches(CompletedOp),
}

/// Result of evaluating a requested operation against an expectation pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Evaluation {
    /// The evaluation failed, resulting in a test failure.
    Fail,

    /// Continue evaluating the requested operation.
    ///
    /// When `retired` is true, the current expectation is retired. Incoming
    /// MMIO operations are matched against the rest of the expectation list.
    ///
    /// When `operation` is [`Some`], the requested operation is completed and
    /// its return value is returned to the driver. When `operation` is
    /// [`None`], the pattern was retired upon receiving a non-matching access
    /// (e.g. transitioning out of a [`ExpectationPattern::WhileMatches`]
    /// polling loop), so the matching loop continues against the next
    /// expectation in the list.
    Continue { retired: bool, operation: Option<CompletedOp> },
}

impl ExpectationPattern {
    /// Evaluates an MMIO operation against this pattern.
    ///
    /// The returned [`Evaluation`] will not have both `retired` set to false
    /// and `operation` set to [`None`]. An implementation that breaks this
    /// postcondition would cause an infinite evaluation loop.
    pub fn evaluate(&self, requested: RequestedOp) -> Evaluation {
        match *self {
            Self::MustMatchOnce(expected) => {
                if expected.matches(requested) {
                    Evaluation::Continue { retired: true, operation: Some(expected) }
                } else {
                    Evaluation::Fail
                }
            }
            Self::WhileMatches(expected) => {
                if expected.matches(requested) {
                    Evaluation::Continue { retired: false, operation: Some(expected) }
                } else {
                    Evaluation::Continue { retired: true, operation: None }
                }
            }
        }
    }

    /// Returns the underlying completed operation.
    pub fn op(&self) -> CompletedOp {
        match *self {
            Self::MustMatchOnce(op) | Self::WhileMatches(op) => op,
        }
    }

    /// Returns true if this pattern can be retired if
    /// no further matching accesses arrive at test completion.
    ///
    /// [`Self::WhileMatches`] patterns represent indefinite or trailing polling
    /// states and are considered satisfied. [`Self::MustMatchOnce`] patterns
    /// require an explicit match and are not satisfied until retired.
    pub fn is_satisfied(&self) -> bool {
        match self {
            Self::MustMatchOnce(_) => false,
            Self::WhileMatches(_) => true,
        }
    }

    /// True if matching requests do not retire this pattern.
    pub fn is_repeating(&self) -> bool {
        matches!(self, Self::WhileMatches(_))
    }
}

/// A recorded expectation in the timeline with diagnostic metadata.
///
/// Pairs an [`ExpectationPattern`] with [`ExpectationSourceInfo`] (captured
/// test caller source location and optional register reference) to generate
/// actionable error diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expectation {
    pub pattern: ExpectationPattern,
    pub source_info: ExpectationSourceInfo,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_access::{AccessData, AccessSize};

    #[fuchsia::test]
    fn test_expectation_pattern_op() {
        let read_op =
            CompletedOp::Read { offset: 0x10, data: AccessData::new(AccessSize::U32, 0x1234) };
        assert_eq!(ExpectationPattern::MustMatchOnce(read_op).op(), read_op);
        assert_eq!(ExpectationPattern::WhileMatches(read_op).op(), read_op);

        let barrier_op = CompletedOp::WriteBarrier;
        assert_eq!(ExpectationPattern::MustMatchOnce(barrier_op).op(), barrier_op);
        assert_eq!(ExpectationPattern::WhileMatches(barrier_op).op(), barrier_op);
    }

    #[fuchsia::test]
    fn test_must_match_once_read_evaluate() {
        let expected =
            CompletedOp::Read { offset: 0x10, data: AccessData::new(AccessSize::U32, 0x1234) };
        let pattern = ExpectationPattern::MustMatchOnce(expected);

        // Matching read
        assert_eq!(
            pattern.evaluate(RequestedOp::Read { offset: 0x10, size: AccessSize::U32 }),
            Evaluation::Continue { retired: true, operation: Some(expected) }
        );

        // Mismatched read: different offset
        assert_eq!(
            pattern.evaluate(RequestedOp::Read { offset: 0x20, size: AccessSize::U32 }),
            Evaluation::Fail
        );

        // Mismatched read: different size
        assert_eq!(
            pattern.evaluate(RequestedOp::Read { offset: 0x10, size: AccessSize::U16 }),
            Evaluation::Fail
        );

        // Mismatched access: write instead of read
        assert_eq!(
            pattern.evaluate(RequestedOp::Write {
                offset: 0x10,
                data: AccessData::new(AccessSize::U32, 0x1234),
            }),
            Evaluation::Fail
        );

        // Mismatched access: barrier instead of read
        assert_eq!(pattern.evaluate(RequestedOp::WriteBarrier), Evaluation::Fail);
    }

    #[fuchsia::test]
    fn test_must_match_once_write_evaluate() {
        let expected =
            CompletedOp::Write { offset: 0x10, data: AccessData::new(AccessSize::U32, 0x5678) };
        let pattern = ExpectationPattern::MustMatchOnce(expected);

        // Matching write
        assert_eq!(
            pattern.evaluate(RequestedOp::Write {
                offset: 0x10,
                data: AccessData::new(AccessSize::U32, 0x5678),
            }),
            Evaluation::Continue { retired: true, operation: Some(expected) }
        );

        // Mismatched write: wrong value
        assert_eq!(
            pattern.evaluate(RequestedOp::Write {
                offset: 0x10,
                data: AccessData::new(AccessSize::U32, 0x9999),
            }),
            Evaluation::Fail
        );

        // Mismatched write: wrong offset
        assert_eq!(
            pattern.evaluate(RequestedOp::Write {
                offset: 0x20,
                data: AccessData::new(AccessSize::U32, 0x5678),
            }),
            Evaluation::Fail
        );

        // Mismatched write: wrong size
        assert_eq!(
            pattern.evaluate(RequestedOp::Write {
                offset: 0x10,
                data: AccessData::new(AccessSize::U16, 0x5678),
            }),
            Evaluation::Fail
        );

        // Mismatched access: read instead of write
        assert_eq!(
            pattern.evaluate(RequestedOp::Read { offset: 0x10, size: AccessSize::U32 }),
            Evaluation::Fail
        );

        // Mismatched access: barrier instead of write
        assert_eq!(pattern.evaluate(RequestedOp::WriteBarrier), Evaluation::Fail);
    }

    #[fuchsia::test]
    fn test_must_match_once_barrier_evaluate() {
        let pattern = ExpectationPattern::MustMatchOnce(CompletedOp::WriteBarrier);

        assert_eq!(
            pattern.evaluate(RequestedOp::WriteBarrier),
            Evaluation::Continue { retired: true, operation: Some(CompletedOp::WriteBarrier) }
        );
        assert_eq!(
            pattern.evaluate(RequestedOp::Read { offset: 0, size: AccessSize::U8 }),
            Evaluation::Fail
        );
        assert_eq!(
            pattern.evaluate(RequestedOp::Write {
                offset: 0,
                data: AccessData::new(AccessSize::U8, 0),
            }),
            Evaluation::Fail
        );
    }

    #[fuchsia::test]
    fn test_while_matches_read_evaluate() {
        let expected =
            CompletedOp::Read { offset: 0x10, data: AccessData::new(AccessSize::U32, 0x42) };
        let pattern = ExpectationPattern::WhileMatches(expected);

        // Matching read returns Continue without retiring the expectation.
        assert_eq!(
            pattern.evaluate(RequestedOp::Read { offset: 0x10, size: AccessSize::U32 }),
            Evaluation::Continue { retired: false, operation: Some(expected) }
        );

        // Any non-matching access retires the expectation without completing.
        let retired = Evaluation::Continue { retired: true, operation: None };
        assert_eq!(
            pattern.evaluate(RequestedOp::Read { offset: 0x20, size: AccessSize::U32 }),
            retired
        );
        assert_eq!(
            pattern.evaluate(RequestedOp::Read { offset: 0x10, size: AccessSize::U16 }),
            retired
        );
        assert_eq!(
            pattern.evaluate(RequestedOp::Write {
                offset: 0x10,
                data: AccessData::new(AccessSize::U32, 0x42),
            }),
            retired
        );
        assert_eq!(pattern.evaluate(RequestedOp::WriteBarrier), retired);
    }

    #[fuchsia::test]
    fn test_while_matches_write_evaluate() {
        let expected =
            CompletedOp::Write { offset: 0x10, data: AccessData::new(AccessSize::U32, 0x5678) };
        let pattern = ExpectationPattern::WhileMatches(expected);

        // Matching write returns Continue without retiring the expectation.
        assert_eq!(
            pattern.evaluate(RequestedOp::Write {
                offset: 0x10,
                data: AccessData::new(AccessSize::U32, 0x5678),
            }),
            Evaluation::Continue { retired: false, operation: Some(expected) }
        );

        // Any non-matching access retires the expectation without completing.
        let retired = Evaluation::Continue { retired: true, operation: None };
        assert_eq!(
            pattern.evaluate(RequestedOp::Write {
                offset: 0x10,
                data: AccessData::new(AccessSize::U32, 0x9999),
            }),
            retired
        );
        assert_eq!(
            pattern.evaluate(RequestedOp::Read { offset: 0x10, size: AccessSize::U32 }),
            retired
        );
        assert_eq!(pattern.evaluate(RequestedOp::WriteBarrier), retired);
    }

    #[fuchsia::test]
    fn test_while_matches_barrier_evaluate() {
        let pattern = ExpectationPattern::WhileMatches(CompletedOp::WriteBarrier);

        assert_eq!(
            pattern.evaluate(RequestedOp::WriteBarrier),
            Evaluation::Continue { retired: false, operation: Some(CompletedOp::WriteBarrier) }
        );
        assert_eq!(
            pattern.evaluate(RequestedOp::Read { offset: 0, size: AccessSize::U8 }),
            Evaluation::Continue { retired: true, operation: None }
        );
    }

    #[fuchsia::test]
    fn test_is_satisfied() {
        let op = CompletedOp::Read { offset: 0x10, data: AccessData::new(AccessSize::U32, 1) };
        assert!(!ExpectationPattern::MustMatchOnce(op).is_satisfied());
        assert!(ExpectationPattern::WhileMatches(op).is_satisfied());

        assert!(!ExpectationPattern::MustMatchOnce(CompletedOp::WriteBarrier).is_satisfied());
        assert!(ExpectationPattern::WhileMatches(CompletedOp::WriteBarrier).is_satisfied());
    }

    #[fuchsia::test]
    fn test_is_repeating() {
        let op = CompletedOp::Read { offset: 0x10, data: AccessData::new(AccessSize::U32, 1) };
        assert!(!ExpectationPattern::MustMatchOnce(op).is_repeating());
        assert!(ExpectationPattern::WhileMatches(op).is_repeating());
    }
}
