// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The scoreboard: the shared state that matches MMIO operations to expectations.

use fuchsia_sync::Mutex;
use std::backtrace::Backtrace;

#[cfg(doc)]
use mmio::region::MmioRegion;

use crate::data_access::{AccessData, AccessSize};
use crate::expectation::{Evaluation, Expectation, ExpectationPattern};
use crate::formatting::{
    MismatchReport, ScoreboardStateView, UnexpectedAccessReport, UnretiredExpectationsReport,
};
use crate::history_entry::HistoryEntry;
use crate::operation::RequestedOp;
use crate::source_info::ExpectationSourceInfo;

#[derive(Debug, Default)]
struct ScoreboardState {
    /// Points into [`Self::expectations`].
    ///
    /// A posted MMIO request is matched against the current expectation.
    current_expectation_index: usize,

    /// Expectations, listed in posting order.
    expectations: Vec<Expectation>,

    /// Successfully completed MMIO operations, listed in completion order.
    ///
    /// Used to provide a rich diagnostic report on test failures.
    completed_ops: Vec<HistoryEntry>,
}

impl ScoreboardState {
    fn view(&self) -> ScoreboardStateView<'_> {
        ScoreboardStateView::from(self)
    }

    /// Implements [`Scoreboard::post_expectation`].
    fn post_expectation(&mut self, expectation: Expectation) {
        self.expectations.push(expectation);
    }

    /// Implements [`Scoreboard::post_request`].
    fn post_request(&mut self, requested: RequestedOp) -> u64 {
        loop {
            let Some(expectation) = self.expectations.get(self.current_expectation_index) else {
                self.fail_unexpected_access(requested);
            };

            match expectation.pattern.evaluate(requested) {
                Evaluation::Fail => self.fail_mismatch(expectation, requested),
                Evaluation::Continue { retired, operation } => {
                    assert!(
                        retired || operation.is_some(),
                        "Evaluation about to enter infinite loop"
                    );

                    let expectation_index = self.current_expectation_index;
                    if let Some(completed) = operation {
                        let source_info = expectation.source_info.clone();
                        if retired {
                            self.current_expectation_index += 1;
                        }
                        self.completed_ops.push(HistoryEntry {
                            op: completed,
                            expectation_index,
                            source_info,
                        });
                        return completed.return_value();
                    }
                    if retired {
                        self.current_expectation_index += 1;
                    }
                    // `operation` is [`None`] when the current expectation was
                    // retired without completing the request (example: a
                    // [`ExpectationPattern::WhileMatches`] pattern is retired
                    // after receiving a non-matching operation). The loop
                    // continues matching the posted request against the next
                    // expectation in the list.
                }
            }
        }
    }

    /// Implements [`Scoreboard::check_all_replayed`].
    fn check_all_replayed(&mut self) {
        while let Some(front) = self.expectations.get(self.current_expectation_index) {
            if !front.pattern.is_satisfied() {
                break;
            }
            self.current_expectation_index += 1;
        }
        if self.current_expectation_index < self.expectations.len() {
            self.fail_unretired_expectations();
        }
    }

    fn fail_mismatch(&self, expectation: &Expectation, actual: RequestedOp) -> ! {
        // Capture the driver's runtime call stack at the moment of the mismatch.
        let backtrace = Backtrace::force_capture();
        panic!(
            "{}",
            MismatchReport { view: self.view(), expectation, actual, backtrace: &backtrace }
        );
    }

    fn fail_unexpected_access(&self, actual: RequestedOp) -> ! {
        // Capture the driver's runtime call stack at the moment of the access.
        let backtrace = Backtrace::force_capture();
        panic!("{}", UnexpectedAccessReport { view: self.view(), actual, backtrace: &backtrace });
    }

    fn fail_unretired_expectations(&self) -> ! {
        panic!("{}", UnretiredExpectationsReport { view: self.view() });
    }
}

impl<'a> From<&'a ScoreboardState> for ScoreboardStateView<'a> {
    fn from(state: &'a ScoreboardState) -> Self {
        ScoreboardStateView {
            retired_expectations: &state.expectations[..state.current_expectation_index],
            pending_expectations: &state.expectations[state.current_expectation_index..],
            completed_ops: &state.completed_ops,
        }
    }
}

/// The globally-ordered MMIO replay scoreboard.
///
/// Expectations and MMIO operation requests (issued by the code under test) are
/// both posted to the `Scoreboard`.
///
/// The expectations list is append-only. Posting an expectation appends it to
/// the list.
///
/// A posted request is repeatedly matched against the current expectation,
/// until the request is completed. If the current expectation is retired, the
/// matching continues against the next expectation in the list.
///
/// Dropping the `Scoreboard` verifies that all expectations were replayed. The
/// mock builder and every [`MmioRegion`] it produces hold a reference, so
/// verification happens once the test is done with all of them.
#[derive(Debug)]
pub struct Scoreboard {
    region_size_bytes: usize,
    state: Mutex<ScoreboardState>,
}

impl Scoreboard {
    /// Creates a new scoreboard covering `region_size_bytes` bytes.
    pub fn new(region_size_bytes: usize) -> Self {
        Self { region_size_bytes, state: Mutex::new(ScoreboardState::default()) }
    }

    /// Returns the region size in bytes.
    pub fn region_size_bytes(&self) -> usize {
        self.region_size_bytes
    }

    /// Extends the scoreboard's list of expected MMIO operations.
    ///
    /// # Panics
    ///
    /// Panics if the expectation specifies an MMIO operation that would not
    /// meet [`MmioRegion`] API requirements.
    pub fn post_expectation(
        &self,
        pattern: ExpectationPattern,
        source_info: ExpectationSourceInfo,
    ) {
        if let Some((offset, data)) = pattern.op().data_access() {
            let location = source_info.location;
            let size = data.size();
            let size_bytes = size.size_bytes();
            assert!(
                offset.checked_add(size_bytes).is_some_and(|end| end <= self.region_size_bytes),
                "MMIO expectation at {location} exceeds region size: {size} access at offset \
                 {offset:#x} exceeds region size {:#x}",
                self.region_size_bytes
            );
            assert!(
                offset % size_bytes == 0,
                "MMIO expectation at {location} is unaligned: offset {offset:#x} is not aligned \
                 to {size}"
            );
        }

        self.state.lock().post_expectation(Expectation { pattern, source_info });
    }

    /// Processes an MMIO operation request posted by the code under test.
    ///
    /// A posted request is repeatedly matched against the current expectation,
    /// until the request is completed. If the current expectation is retired, the
    /// matching continues against the next expectation in the list.
    ///
    /// # Panics
    ///
    /// Panics (resulting in a test failure) if the MMIO operation does not match
    /// the recorded expectations.
    pub fn post_request(&self, requested: RequestedOp) -> u64 {
        // Execution only passes tests if it matches expectations, so MMIO
        // bounds and alignment checks are shifted left to expectation posting
        // (`post_expectation`). Any out-of-bounds or unaligned access attempted
        // by the driver will either fail to match an expectation (reporting a
        // mismatch) or occur when no expectation is queued (reporting
        // unexpected access).
        self.state.lock().post_request(requested)
    }

    /// Posts an MMIO read operation request of width `size` at `offset`.
    pub fn load(&self, offset: usize, size: AccessSize) -> u64 {
        self.post_request(RequestedOp::Read { offset, size })
    }

    /// Posts an MMIO write operation request of width `size` at `offset` with `actual_value`.
    pub fn store(&self, offset: usize, size: AccessSize, actual_value: u64) {
        self.post_request(RequestedOp::Write { offset, data: AccessData::new(size, actual_value) });
    }

    /// Posts an MMIO memory write barrier request.
    pub fn write_barrier(&self) {
        self.post_request(RequestedOp::WriteBarrier);
    }

    /// Verifies that all expected MMIO operations have been executed.
    ///
    /// Trailing expectations that are already satisfied (example:
    /// [`ExpectationPattern::WhileMatches`] trailing poll steps) are
    /// automatically retired.
    ///
    /// # Panics
    ///
    /// Panics (resulting in a test failure) if any expectation was never matched.
    pub fn check_all_replayed(&self) {
        self.state.lock().check_all_replayed();
    }
}

impl Drop for Scoreboard {
    fn drop(&mut self) {
        // Avoid secondary panics during unwinding to prevent aborting the
        // process or masking the primary assertion failure that triggered the
        // unwind.
        if std::thread::panicking() {
            return;
        }

        self.check_all_replayed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::CompletedOp;
    use crate::source_info::RegisterRef;
    use core::panic::Location;

    /// Posts `pattern` with a throwaway source location.
    fn post(scoreboard: &Scoreboard, pattern: ExpectationPattern) {
        scoreboard.post_expectation(pattern, ExpectationSourceInfo::new(None, Location::caller()));
    }

    fn read_once(offset: usize, size: AccessSize, value: u64) -> ExpectationPattern {
        ExpectationPattern::MustMatchOnce(CompletedOp::Read {
            offset,
            data: AccessData::new(size, value),
        })
    }

    fn write_once(offset: usize, size: AccessSize, value: u64) -> ExpectationPattern {
        ExpectationPattern::MustMatchOnce(CompletedOp::Write {
            offset,
            data: AccessData::new(size, value),
        })
    }

    fn read_while(offset: usize, size: AccessSize, value: u64) -> ExpectationPattern {
        ExpectationPattern::WhileMatches(CompletedOp::Read {
            offset,
            data: AccessData::new(size, value),
        })
    }

    /// Retires every expectation so the scoreboard's [`Drop`] check passes.
    fn drain(scoreboard: Scoreboard) {
        scoreboard.check_all_replayed();
    }

    #[fuchsia::test]
    #[should_panic(expected = "exceeds region size")]
    fn test_expectation_out_of_bounds_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_once(0x1000, AccessSize::U8, 0));
    }

    #[fuchsia::test]
    #[should_panic(expected = "exceeds region size")]
    fn test_expectation_offset_overflow_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_once(usize::MAX, AccessSize::U8, 0));
    }

    #[fuchsia::test]
    #[should_panic(expected = "exceeds region size")]
    fn test_expectation_spanning_out_of_bounds_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_once(0x0ffc, AccessSize::U64, 0));
    }

    #[fuchsia::test]
    #[should_panic(expected = "is unaligned")]
    fn test_expectation_unaligned_u16_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_once(1, AccessSize::U16, 0));
    }

    #[fuchsia::test]
    #[should_panic(expected = "is unaligned")]
    fn test_expectation_unaligned_u32_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_once(1, AccessSize::U32, 0));
    }

    #[fuchsia::test]
    #[should_panic(expected = "is unaligned")]
    fn test_expectation_unaligned_u64_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_once(4, AccessSize::U64, 0));
    }

    #[fuchsia::test]
    #[should_panic(expected = "is unaligned")]
    fn test_expectation_unaligned_write_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, write_once(1, AccessSize::U32, 0));
    }

    #[fuchsia::test]
    fn test_region_size_bytes() {
        let scoreboard = Scoreboard::new(0x1000);
        assert_eq!(scoreboard.region_size_bytes(), 0x1000);
        drain(scoreboard);
    }

    #[fuchsia::test]
    fn test_check_all_replayed_accepts_an_empty_scoreboard() {
        let scoreboard = Scoreboard::new(0x1000);
        scoreboard.check_all_replayed();
    }

    #[fuchsia::test]
    fn test_check_all_replayed_retires_trailing_satisfied_expectations() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_while(0x10, AccessSize::U32, 1));
        post(&scoreboard, read_while(0x20, AccessSize::U32, 2));
        scoreboard.check_all_replayed();
    }

    #[fuchsia::test]
    #[should_panic(expected = "UNRETIRED MMIO EXPECTATIONS")]
    fn test_check_all_replayed_rejects_unsatisfied_expectations() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_once(0x10, AccessSize::U32, 1));
        scoreboard.check_all_replayed();
    }

    #[fuchsia::test]
    #[should_panic(expected = "UNRETIRED MMIO EXPECTATIONS")]
    fn test_check_all_replayed_rejects_satisfied_then_unsatisfied() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_while(0x10, AccessSize::U32, 1));
        post(&scoreboard, read_once(0x20, AccessSize::U32, 2));
        scoreboard.check_all_replayed();
    }

    #[fuchsia::test]
    #[should_panic(expected = "UNRETIRED MMIO EXPECTATIONS")]
    fn test_drop_verifies_expectations() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_once(0x10, AccessSize::U32, 1));
        drop(scoreboard);
    }

    #[fuchsia::test]
    #[should_panic(expected = "primary panic")]
    fn test_drop_during_panic_does_not_panic_again() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_once(0x10, AccessSize::U32, 1));
        panic!("primary panic");
    }

    #[fuchsia::test]
    #[should_panic(expected = "MMIO EXPECTATION MISMATCH")]
    fn test_mismatch_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_once(0x10, AccessSize::U32, 0x42));
        let _ = scoreboard.load(0x20, AccessSize::U32);
    }

    #[fuchsia::test]
    #[should_panic(expected = "UNEXPECTED MMIO ACCESS")]
    fn test_unexpected_access_panics() {
        let scoreboard = Scoreboard::new(0x1000);
        let _ = scoreboard.load(0x10, AccessSize::U32);
    }

    #[fuchsia::test]
    fn test_load_and_store_round_trip() {
        for (size, written, read) in [
            (AccessSize::U8, 0x12, 0x34),
            (AccessSize::U16, 0x1234, 0x5678),
            (AccessSize::U32, 0x1234_5678, 0x9abc_def0),
            (AccessSize::U64, 0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210),
        ] {
            let scoreboard = Scoreboard::new(0x1000);
            post(&scoreboard, write_once(0x0, size, written));
            post(&scoreboard, read_once(0x8, size, read));

            scoreboard.store(0x0, size, written);
            assert_eq!(scoreboard.load(0x8, size), read);
            drain(scoreboard);
        }
    }

    #[fuchsia::test]
    fn test_write_barrier() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, ExpectationPattern::MustMatchOnce(CompletedOp::WriteBarrier));
        scoreboard.write_barrier();
        drain(scoreboard);
    }

    #[fuchsia::test]
    fn test_must_match_once_retires_on_match() {
        let scoreboard = Scoreboard::new(0x1000);
        assert_eq!(scoreboard.state.lock().current_expectation_index, 0);

        post(&scoreboard, read_once(0x10, AccessSize::U32, 0x42));
        // Posting an expectation does not advance the current expectation.
        assert_eq!(scoreboard.state.lock().current_expectation_index, 0);

        assert_eq!(scoreboard.load(0x10, AccessSize::U32), 0x42);
        assert_eq!(scoreboard.state.lock().current_expectation_index, 1);
        drain(scoreboard);
    }

    #[fuchsia::test]
    fn test_while_matches_retires_on_the_first_non_matching_access() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_while(0x10, AccessSize::U32, 0x42));
        post(&scoreboard, write_once(0x20, AccessSize::U32, 0x99));

        // Matching reads do not retire the repeating expectation.
        assert_eq!(scoreboard.load(0x10, AccessSize::U32), 0x42);
        assert_eq!(scoreboard.load(0x10, AccessSize::U32), 0x42);
        assert_eq!(scoreboard.state.lock().current_expectation_index, 0);

        // The non-matching write retires it, then matches the next expectation.
        scoreboard.store(0x20, AccessSize::U32, 0x99);
        assert_eq!(scoreboard.state.lock().current_expectation_index, 2);
        drain(scoreboard);
    }

    #[fuchsia::test]
    fn test_consecutive_while_matches_transitions() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_while(0x10, AccessSize::U32, 0x1));
        post(&scoreboard, read_while(0x20, AccessSize::U32, 0x2));
        post(&scoreboard, write_once(0x30, AccessSize::U32, 0x3));

        assert_eq!(scoreboard.load(0x10, AccessSize::U32), 0x1);
        assert_eq!(scoreboard.load(0x10, AccessSize::U32), 0x1);

        assert_eq!(scoreboard.load(0x20, AccessSize::U32), 0x2);
        assert_eq!(scoreboard.load(0x20, AccessSize::U32), 0x2);

        scoreboard.store(0x30, AccessSize::U32, 0x3);
        drain(scoreboard);
    }

    #[fuchsia::test]
    fn test_history_records_the_matching_expectation_index() {
        let scoreboard = Scoreboard::new(0x1000);
        post(&scoreboard, read_while(0x10, AccessSize::U32, 0x1));
        post(&scoreboard, write_once(0x20, AccessSize::U32, 0x2));

        assert_eq!(scoreboard.load(0x10, AccessSize::U32), 0x1);
        assert_eq!(scoreboard.load(0x10, AccessSize::U32), 0x1);
        scoreboard.store(0x20, AccessSize::U32, 0x2);

        let state = scoreboard.state.lock();
        let indexes: Vec<usize> =
            state.completed_ops.iter().map(|entry| entry.expectation_index).collect();
        assert_eq!(indexes, [0, 0, 1]);
        drop(state);
        drain(scoreboard);
    }

    #[fuchsia::test]
    fn test_state_view_partitions_the_expectation_list() {
        let scoreboard = Scoreboard::new(0x1000);
        scoreboard.post_expectation(
            read_once(0x10, AccessSize::U32, 0x42),
            ExpectationSourceInfo::new(
                Some(RegisterRef::new("registers::Test")),
                Location::caller(),
            ),
        );

        {
            let state = scoreboard.state.lock();
            let view = state.view();
            assert_eq!(view.retired_expectations.len(), 0);
            assert_eq!(view.pending_expectations.len(), 1);
            assert_eq!(view.completed_ops.len(), 0);
        }

        assert_eq!(scoreboard.load(0x10, AccessSize::U32), 0x42);

        {
            let state = scoreboard.state.lock();
            let view = state.view();
            assert_eq!(view.retired_expectations.len(), 1);
            assert_eq!(view.pending_expectations.len(), 0);
            assert_eq!(view.completed_ops.len(), 1);
        }
        drain(scoreboard);
    }
}
