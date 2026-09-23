// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Diagnostic failure reports for the MMIO mock.
//!
//! Each report is a [`std::fmt::Display`] type that renders straight into the
//! panic payload. Reports include:
//! * The failing expectation's index and test source location (`file:line:column`).
//! * An Expected vs. Actual pair, resolving register names where available.
//! * A trace window showing completed accesses (`[OK]`), the failing
//!   expectation (`--> [ERR]`), and the expectations that come next (`[..]`).
//! * For access failures, the driver's runtime backtrace.
//!
//! Trace window rows are indexed by *expectation* position, including the rows
//! describing completed operations. A repeating expectation therefore produces
//! several rows sharing one index, and the indexes never move backwards.

use core::panic::Location;
use std::backtrace::Backtrace;
use std::fmt::{self, Display, Formatter};

use crate::data_access::AccessData;
use crate::expectation::Expectation;
use crate::history_entry::HistoryEntry;
use crate::operation::{CompletedOp, RequestedOp};
use crate::source_info::RegisterRef;

/// Width, in characters, of the horizontal rules that delimit a report.
const REPORT_WIDTH: usize = 76;

/// Minimum width, in characters, of the reproduction code column in trace
/// window rows.
///
/// Register names are fully qualified, so they routinely overflow this width.
/// [`write_trace_window`] widens the column to fit its longest row.
const CODE_COLUMN_WIDTH: usize = 50;

/// Maximum number of completed operations shown in a trace window.
///
/// The window keeps the *most recent* operations, and reports how many older
/// operations it dropped.
const MAX_HISTORY_ENTRIES: usize = 10;

/// Maximum number of pending expectations shown in a trace window.
///
/// The window keeps the expectations that will be matched *first*, and reports
/// how many later expectations it dropped.
const MAX_UPCOMING_ENTRIES: usize = 5;

/// A borrowed view of the MMIO mock scoreboard's state at a point in time.
#[derive(Clone, Debug)]
pub struct ScoreboardStateView<'a> {
    /// Expectations that have been retired, in posting order.
    pub retired_expectations: &'a [Expectation],

    /// Expectations that have not been retired, in posting order.
    ///
    /// The first entry, if any, is the expectation that incoming MMIO
    /// operations are currently matched against.
    pub pending_expectations: &'a [Expectation],

    /// Successfully completed MMIO operations, in completion order.
    pub completed_ops: &'a [HistoryEntry],
}

impl ScoreboardStateView<'_> {
    /// Position, in the full expectation list, of the current expectation.
    fn current_expectation_index(&self) -> usize {
        self.retired_expectations.len()
    }

    /// Number of pending expectations that would fail the test if never matched.
    ///
    /// Trailing repeating expectations are already satisfied, so they are
    /// excluded even when they are still pending.
    fn unsatisfied_count(&self) -> usize {
        self.pending_expectations.iter().filter(|e| !e.pattern.is_satisfied()).count()
    }

    /// Returns the register declared at `offset` by any expectation.
    ///
    /// Pending expectations are searched first, then retired expectations from
    /// most to least recently retired, so the declaration closest to the
    /// failure wins.
    fn register_at(&self, offset: usize) -> Option<&RegisterRef> {
        self.pending_expectations.iter().chain(self.retired_expectations.iter().rev()).find_map(
            |expectation| {
                let (expectation_offset, _) = expectation.pattern.op().data_access()?;
                (expectation_offset == offset)
                    .then_some(expectation.source_info.register.as_ref())
                    .flatten()
            },
        )
    }
}

/// Reports an MMIO operation that does not match the current expectation.
pub struct MismatchReport<'a> {
    pub view: ScoreboardStateView<'a>,
    pub expectation: &'a Expectation,
    pub actual: RequestedOp,
    pub backtrace: &'a Backtrace,
}

impl Display for MismatchReport<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write_banner(f, "MMIO EXPECTATION MISMATCH")?;

        let location = self.expectation.source_info.location;
        writeln!(f, "FAILED AT EXPECTATION #{}:", self.view.current_expectation_index())?;
        writeln!(f, "  --> {}:{}:{}", location.file(), location.line(), location.column())?;

        writeln!(f, "\nMISMATCH DETAILS:")?;
        writeln!(f, "  Expected: {}", ExpectedAccess(self.expectation))?;
        writeln!(f, "  Actual:   {}", ActualAccess { view: &self.view, requested: self.actual })?;

        writeln!(f, "\nRECENT TRACE HISTORY:")?;
        write_trace_window(f, &self.view, HighlightFailure::Yes)?;

        write_backtrace(f, self.backtrace)?;
        write_rule(f)
    }
}

/// Reports an MMIO operation performed while the expectation queue is empty.
pub struct UnexpectedAccessReport<'a> {
    pub view: ScoreboardStateView<'a>,
    pub actual: RequestedOp,
    pub backtrace: &'a Backtrace,
}

impl Display for UnexpectedAccessReport<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write_banner(f, "UNEXPECTED MMIO ACCESS")?;

        writeln!(f, "UNEXPECTED ACCESS (expectation queue is empty):")?;
        writeln!(f, "  Actual:   {}", ActualAccess { view: &self.view, requested: self.actual })?;

        writeln!(f, "\nRECENT TRACE HISTORY:")?;
        write_trace_window(f, &self.view, HighlightFailure::No)?;

        write_backtrace(f, self.backtrace)?;
        write_rule(f)
    }
}

/// Reports expectations that were never matched, at test completion.
pub struct UnretiredExpectationsReport<'a> {
    pub view: ScoreboardStateView<'a>,
}

impl Display for UnretiredExpectationsReport<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write_banner(f, "UNRETIRED MMIO EXPECTATIONS")?;

        writeln!(
            f,
            "{} expectation(s) were never matched at test completion.",
            self.view.unsatisfied_count()
        )?;

        writeln!(f, "\nRECENT TRACE HISTORY:")?;
        write_trace_window(f, &self.view, HighlightFailure::No)?;
        write_rule(f)
    }
}

/// Whether a trace window marks its first pending expectation as the failure.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum HighlightFailure {
    Yes,
    No,
}

/// Renders the target of an MMIO access: a register reference, or a raw offset.
struct AccessTarget<'a> {
    offset: usize,
    register: Option<&'a RegisterRef>,
}

impl Display for AccessTarget<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Some(register) = self.register else {
            return write!(f, "offset {:#06x}", self.offset);
        };
        match register.index {
            Some(index) => write!(f, "{}[{}] ({:#06x})", register.name, index, self.offset),
            None => write!(f, "{} ({:#06x})", register.name, self.offset),
        }
    }
}

/// Renders the access described by an expectation, for the `Expected:` line.
struct ExpectedAccess<'a>(&'a Expectation);

impl Display for ExpectedAccess<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let register = self.0.source_info.register.as_ref();
        match self.0.pattern.op() {
            CompletedOp::Read { offset, data } => {
                let target = AccessTarget { offset, register };
                write!(f, "READ  {} at {target} returning {data}", data.size())?;
            }
            CompletedOp::Write { offset, data } => {
                let target = AccessTarget { offset, register };
                write!(f, "WRITE {} at {target} with value {data}", data.size())?;
            }
            CompletedOp::WriteBarrier => write!(f, "WRITE_BARRIER")?,
        }
        if self.0.pattern.is_repeating() {
            write!(f, " (repeating)")?;
        }
        Ok(())
    }
}

/// Renders the access requested by the driver, for the `Actual:` line.
///
/// Resolves the register name from the expectation list, so that unexpected
/// accesses are still reported by register name where possible.
struct ActualAccess<'a> {
    view: &'a ScoreboardStateView<'a>,
    requested: RequestedOp,
}

impl Display for ActualAccess<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.requested {
            RequestedOp::Read { offset, size } => {
                let target = AccessTarget { offset, register: self.view.register_at(offset) };
                write!(f, "READ  {size} at {target}")
            }
            RequestedOp::Write { offset, data } => {
                let target = AccessTarget { offset, register: self.view.register_at(offset) };
                write!(f, "WRITE {} at {target} with value {data}", data.size())
            }
            RequestedOp::WriteBarrier => write!(f, "WRITE_BARRIER"),
        }
    }
}

/// Renders an operation as the `ExpectedTraceBuilder` call that declares it.
///
/// Register names are the fully qualified paths reported by
/// [`std::any::type_name`]. Drivers use nested register modules, so shortening
/// heuristics would produce ambiguous or wrong names.
struct ReproductionCode<'a> {
    op: CompletedOp,
    register: Option<&'a RegisterRef>,
    repeating: bool,
}

impl<'a> ReproductionCode<'a> {
    /// The call that would declare an expectation matching a completed operation.
    fn completed(entry: &'a HistoryEntry) -> Self {
        Self { op: entry.op, register: entry.source_info.register.as_ref(), repeating: false }
    }

    /// The call that declared `expectation`.
    fn expected(expectation: &'a Expectation) -> Self {
        Self {
            op: expectation.pattern.op(),
            register: expectation.source_info.register.as_ref(),
            repeating: expectation.pattern.is_repeating(),
        }
    }

    fn write_data_access(
        &self,
        f: &mut Formatter<'_>,
        verb: &str,
        offset: usize,
        data: AccessData,
    ) -> fmt::Result {
        let Some(register) = self.register else {
            return write!(f, "t.at({offset:#06x}).{verb}{}({data});", data.size().bits());
        };
        let name = register.name;
        match register.index {
            Some(index) => write!(f, "t.{verb}_indexed::<{name}>({index}, {data});"),
            None => write!(f, "t.{verb}::<{name}>({data});"),
        }
    }
}

impl Display for ReproductionCode<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.op {
            CompletedOp::Read { offset, data } => {
                self.write_data_access(f, "read", offset, data)?
            }
            CompletedOp::Write { offset, data } => {
                self.write_data_access(f, "write", offset, data)?
            }
            CompletedOp::WriteBarrier => write!(f, "t.write_barrier();")?,
        }
        if self.repeating {
            write!(f, " // repeating")?;
        }
        Ok(())
    }
}

/// Renders the horizontal rule that opens a report, with `title` centered.
fn write_banner(f: &mut Formatter<'_>, title: &str) -> fmt::Result {
    // One space separates the title from the rule on each side.
    let rule_width = REPORT_WIDTH.saturating_sub(title.len() + 2);
    let left_width = rule_width / 2;
    writeln!(f, "\n{} {title} {}\n", "═".repeat(left_width), "═".repeat(rule_width - left_width))
}

/// Renders the horizontal rule that closes a report.
fn write_rule(f: &mut Formatter<'_>) -> fmt::Result {
    writeln!(f, "{}", "═".repeat(REPORT_WIDTH))
}

/// Renders the driver's call stack at the moment of the failure.
fn write_backtrace(f: &mut Formatter<'_>, backtrace: &Backtrace) -> fmt::Result {
    writeln!(f, "\nDRIVER INVOCATION BACKTRACE:")?;
    writeln!(f, "{backtrace}")
}

/// One row of a trace window, with the code already materialized.
///
/// The code must be materialized before any row is written, because the code
/// column is as wide as the longest row.
struct TraceRow {
    arrow: &'static str,
    expectation_index: usize,
    status: &'static str,
    code: String,
    location: &'static Location<'static>,
}

impl TraceRow {
    fn write(&self, f: &mut Formatter<'_>, code_width: usize) -> fmt::Result {
        let Self { arrow, expectation_index, status, code, location } = self;
        writeln!(
            f,
            "{arrow} [#{expectation_index}] {status} {code:<code_width$} ({}:{})",
            location.file(),
            location.line()
        )
    }
}

/// Renders the recently completed accesses, then the upcoming expectations.
fn write_trace_window(
    f: &mut Formatter<'_>,
    view: &ScoreboardStateView<'_>,
    highlight_failure: HighlightFailure,
) -> fmt::Result {
    let omitted_ops = view.completed_ops.len().saturating_sub(MAX_HISTORY_ENTRIES);
    let mut rows: Vec<TraceRow> = view.completed_ops[omitted_ops..]
        .iter()
        .map(|entry| TraceRow {
            arrow: "   ",
            expectation_index: entry.expectation_index,
            status: "[OK] ",
            code: ReproductionCode::completed(entry).to_string(),
            location: entry.source_info.location,
        })
        .collect();

    let upcoming = view.pending_expectations.iter().take(MAX_UPCOMING_ENTRIES);
    rows.extend(upcoming.enumerate().map(|(position, expectation)| {
        let is_failure = highlight_failure == HighlightFailure::Yes && position == 0;
        let (arrow, status) = match (is_failure, expectation.pattern.is_satisfied()) {
            (true, _) => ("-->", "[ERR]"),
            // A satisfied expectation is still pending, but would auto-retire.
            (false, true) => ("   ", "[opt]"),
            (false, false) => ("   ", "[..] "),
        };
        TraceRow {
            arrow,
            expectation_index: view.current_expectation_index() + position,
            status,
            code: ReproductionCode::expected(expectation).to_string(),
            location: expectation.source_info.location,
        }
    }));

    // Fully qualified register paths can exceed the nominal column width. The
    // column grows to fit them, so the location column stays aligned.
    let code_width = rows
        .iter()
        .map(|row| row.code.len())
        .max()
        .unwrap_or(CODE_COLUMN_WIDTH)
        .max(CODE_COLUMN_WIDTH);

    if omitted_ops > 0 {
        writeln!(f, "    ... {omitted_ops} earlier access(es) omitted")?;
    }
    for row in &rows {
        row.write(f, code_width)?;
    }
    let omitted_expectations = view.pending_expectations.len().saturating_sub(MAX_UPCOMING_ENTRIES);
    if omitted_expectations > 0 {
        writeln!(f, "    ... and {omitted_expectations} more expectation(s)")?;
    }

    if rows.is_empty() {
        writeln!(f, "    (No accesses completed successfully yet)")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // Report tests compare against the entire string (`assert_eq!`) rather than
    // using `contains()`. The tests double as documentation of the exact output
    // produced by the mock's diagnostics, so they must not build their expected
    // values out of the production format strings.

    use super::*;
    use crate::data_access::AccessSize;
    use crate::expectation::ExpectationPattern;
    use crate::source_info::ExpectationSourceInfo;

    /// Source location shared by the fixtures below.
    fn location() -> &'static Location<'static> {
        Location::caller()
    }

    /// Renders `location()` the way trace rows do.
    fn location_row() -> String {
        format!("{}:{}", location().file(), location().line())
    }

    /// Renders `location()` the way report headers do.
    fn location_header() -> String {
        format!("{}:{}:{}", location().file(), location().line(), location().column())
    }

    fn expectation(pattern: ExpectationPattern, register: Option<RegisterRef>) -> Expectation {
        Expectation { pattern, source_info: ExpectationSourceInfo::new(register, location()) }
    }

    fn read_op(offset: usize, size: AccessSize, value: u64) -> CompletedOp {
        CompletedOp::Read { offset, data: AccessData::new(size, value) }
    }

    fn write_op(offset: usize, size: AccessSize, value: u64) -> CompletedOp {
        CompletedOp::Write { offset, data: AccessData::new(size, value) }
    }

    fn read_expectation(offset: usize, value: u64, register: Option<RegisterRef>) -> Expectation {
        expectation(
            ExpectationPattern::MustMatchOnce(read_op(offset, AccessSize::U32, value)),
            register,
        )
    }

    fn history(
        op: CompletedOp,
        expectation_index: usize,
        register: Option<RegisterRef>,
    ) -> HistoryEntry {
        HistoryEntry {
            op,
            expectation_index,
            source_info: ExpectationSourceInfo::new(register, location()),
        }
    }

    // -----------------------------------------------------------------------
    // AccessTarget
    // -----------------------------------------------------------------------

    #[fuchsia::test]
    fn test_access_target_raw_offset() {
        let target = AccessTarget { offset: 0x10, register: None };
        assert_eq!(target.to_string(), "offset 0x0010");
    }

    #[fuchsia::test]
    fn test_access_target_wide_offset_grows_past_four_digits() {
        let target = AccessTarget { offset: 0x1_2345, register: None };
        assert_eq!(target.to_string(), "offset 0x12345");
    }

    #[fuchsia::test]
    fn test_access_target_register() {
        let register = RegisterRef::new("registers::Status");
        let target = AccessTarget { offset: 0x10, register: Some(&register) };
        assert_eq!(target.to_string(), "registers::Status (0x0010)");
    }

    #[fuchsia::test]
    fn test_access_target_indexed_register() {
        let register = RegisterRef::indexed("registers::Data", 2);
        let target = AccessTarget { offset: 0x108, register: Some(&register) };
        assert_eq!(target.to_string(), "registers::Data[2] (0x0108)");
    }

    // -----------------------------------------------------------------------
    // ExpectedAccess
    // -----------------------------------------------------------------------

    #[fuchsia::test]
    fn test_expected_access_raw_read() {
        let expectation = read_expectation(0x10, 0x1234, None);
        assert_eq!(
            ExpectedAccess(&expectation).to_string(),
            "READ  32-bit at offset 0x0010 returning 0x00001234"
        );
    }

    #[fuchsia::test]
    fn test_expected_access_raw_write_widths() {
        for (size, value, rendered) in [
            (AccessSize::U8, 0x12, "READ  8-bit at offset 0x0020 returning 0x12"),
            (AccessSize::U16, 0x1234, "READ  16-bit at offset 0x0020 returning 0x1234"),
            (AccessSize::U32, 0x1234, "READ  32-bit at offset 0x0020 returning 0x00001234"),
            (
                AccessSize::U64,
                0x0123_4567_89ab_cdef,
                "READ  64-bit at offset 0x0020 returning 0x0123456789abcdef",
            ),
        ] {
            let expectation =
                expectation(ExpectationPattern::MustMatchOnce(read_op(0x20, size, value)), None);
            assert_eq!(ExpectedAccess(&expectation).to_string(), rendered);
        }
    }

    #[fuchsia::test]
    fn test_expected_access_named_write() {
        let expectation = expectation(
            ExpectationPattern::MustMatchOnce(write_op(0x20, AccessSize::U32, 0x5678)),
            Some(RegisterRef::new("registers::Command")),
        );
        assert_eq!(
            ExpectedAccess(&expectation).to_string(),
            "WRITE 32-bit at registers::Command (0x0020) with value 0x00005678"
        );
    }

    #[fuchsia::test]
    fn test_expected_access_indexed_read() {
        let expectation = expectation(
            ExpectationPattern::MustMatchOnce(read_op(0x108, AccessSize::U32, 0x30)),
            Some(RegisterRef::indexed("registers::Data", 2)),
        );
        assert_eq!(
            ExpectedAccess(&expectation).to_string(),
            "READ  32-bit at registers::Data[2] (0x0108) returning 0x00000030"
        );
    }

    #[fuchsia::test]
    fn test_expected_access_marks_repeating_expectations() {
        let expectation = expectation(
            ExpectationPattern::WhileMatches(read_op(0x10, AccessSize::U32, 0x1)),
            Some(RegisterRef::new("registers::Status")),
        );
        assert_eq!(
            ExpectedAccess(&expectation).to_string(),
            "READ  32-bit at registers::Status (0x0010) returning 0x00000001 (repeating)"
        );
    }

    #[fuchsia::test]
    fn test_expected_access_write_barrier() {
        let expectation =
            expectation(ExpectationPattern::MustMatchOnce(CompletedOp::WriteBarrier), None);
        assert_eq!(ExpectedAccess(&expectation).to_string(), "WRITE_BARRIER");
    }

    // -----------------------------------------------------------------------
    // ActualAccess
    // -----------------------------------------------------------------------

    #[fuchsia::test]
    fn test_actual_access_uses_the_same_phrasing_as_expected_access() {
        let expectation = read_expectation(0x10, 0x1234, None);
        let pending = [expectation];
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &pending,
            completed_ops: &[],
        };
        let requested = RequestedOp::Read { offset: 0x10, size: AccessSize::U32 };

        // Only the value clause differs, so the two lines diff cleanly.
        assert_eq!(
            ExpectedAccess(&pending[0]).to_string(),
            "READ  32-bit at offset 0x0010 returning 0x00001234"
        );
        assert_eq!(
            ActualAccess { view: &view, requested }.to_string(),
            "READ  32-bit at offset 0x0010"
        );
    }

    #[fuchsia::test]
    fn test_actual_access_resolves_register_names() {
        let pending = [
            read_expectation(0x20, 0, Some(RegisterRef::new("registers::Status"))),
            expectation(
                ExpectationPattern::WhileMatches(read_op(0x40, AccessSize::U32, 0)),
                Some(RegisterRef::new("registers::While")),
            ),
        ];
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &pending,
            completed_ops: &[],
        };

        let unnamed = RequestedOp::Read { offset: 0x10, size: AccessSize::U32 };
        assert_eq!(
            ActualAccess { view: &view, requested: unnamed }.to_string(),
            "READ  32-bit at offset 0x0010"
        );

        let named = RequestedOp::Read { offset: 0x20, size: AccessSize::U32 };
        assert_eq!(
            ActualAccess { view: &view, requested: named }.to_string(),
            "READ  32-bit at registers::Status (0x0020)"
        );

        let repeating =
            RequestedOp::Write { offset: 0x40, data: AccessData::new(AccessSize::U32, 0x5555) };
        assert_eq!(
            ActualAccess { view: &view, requested: repeating }.to_string(),
            "WRITE 32-bit at registers::While (0x0040) with value 0x00005555"
        );
    }

    #[fuchsia::test]
    fn test_actual_access_prefers_the_most_recently_retired_register() {
        let retired = [
            read_expectation(0x20, 0, Some(RegisterRef::new("registers::Old"))),
            read_expectation(0x20, 0, Some(RegisterRef::new("registers::New"))),
        ];
        let view = ScoreboardStateView {
            retired_expectations: &retired,
            pending_expectations: &[],
            completed_ops: &[],
        };
        let requested = RequestedOp::Read { offset: 0x20, size: AccessSize::U32 };

        assert_eq!(
            ActualAccess { view: &view, requested }.to_string(),
            "READ  32-bit at registers::New (0x0020)"
        );
    }

    #[fuchsia::test]
    fn test_actual_access_write_barrier() {
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &[],
            completed_ops: &[],
        };
        assert_eq!(
            ActualAccess { view: &view, requested: RequestedOp::WriteBarrier }.to_string(),
            "WRITE_BARRIER"
        );
    }

    // -----------------------------------------------------------------------
    // ReproductionCode
    // -----------------------------------------------------------------------

    #[fuchsia::test]
    fn test_reproduction_code_raw_offsets() {
        for (op, rendered) in [
            (read_op(0x10, AccessSize::U8, 0x12), "t.at(0x0010).read8(0x12);"),
            (read_op(0x10, AccessSize::U16, 0x1234), "t.at(0x0010).read16(0x1234);"),
            (read_op(0x10, AccessSize::U32, 0x1234), "t.at(0x0010).read32(0x00001234);"),
            (read_op(0x10, AccessSize::U64, 0x1234), "t.at(0x0010).read64(0x0000000000001234);"),
            (write_op(0x10, AccessSize::U32, 0x5678), "t.at(0x0010).write32(0x00005678);"),
        ] {
            let code = ReproductionCode { op, register: None, repeating: false };
            assert_eq!(code.to_string(), rendered);
        }
    }

    #[fuchsia::test]
    fn test_reproduction_code_registers() {
        let status = RegisterRef::new("registers::Status");
        let read = ReproductionCode {
            op: read_op(0x10, AccessSize::U32, 0x1234),
            register: Some(&status),
            repeating: false,
        };
        assert_eq!(read.to_string(), "t.read::<registers::Status>(0x00001234);");

        let command = RegisterRef::new("registers::Command");
        let write = ReproductionCode {
            op: write_op(0x10, AccessSize::U32, 0x5678),
            register: Some(&command),
            repeating: false,
        };
        assert_eq!(write.to_string(), "t.write::<registers::Command>(0x00005678);");
    }

    #[fuchsia::test]
    fn test_reproduction_code_indexed_registers_keep_the_index() {
        let data_2 = RegisterRef::indexed("registers::Data", 2);
        let read = ReproductionCode {
            op: read_op(0x108, AccessSize::U32, 0x30),
            register: Some(&data_2),
            repeating: false,
        };
        assert_eq!(read.to_string(), "t.read_indexed::<registers::Data>(2, 0x00000030);");

        let data_3 = RegisterRef::indexed("registers::Data", 3);
        let write = ReproductionCode {
            op: write_op(0x10c, AccessSize::U32, 0x40),
            register: Some(&data_3),
            repeating: false,
        };
        assert_eq!(write.to_string(), "t.write_indexed::<registers::Data>(3, 0x00000040);");
    }

    #[fuchsia::test]
    fn test_reproduction_code_keeps_fully_qualified_register_names() {
        // Driver register modules nest arbitrarily, so the rendered call must
        // name the register exactly as `std::any::type_name` reports it.
        let version = RegisterRef::new("my_driver::registers::Backend::DsiController::Version");
        let code = ReproductionCode {
            op: read_op(0x10, AccessSize::U32, 0x1),
            register: Some(&version),
            repeating: false,
        };
        assert_eq!(
            code.to_string(),
            "t.read::<my_driver::registers::Backend::DsiController::Version>(0x00000001);"
        );
    }

    #[fuchsia::test]
    fn test_reproduction_code_marks_repeating_expectations() {
        let status = RegisterRef::new("registers::Status");
        let code = ReproductionCode {
            op: read_op(0x10, AccessSize::U32, 0x1),
            register: Some(&status),
            repeating: true,
        };
        assert_eq!(code.to_string(), "t.read::<registers::Status>(0x00000001); // repeating");
    }

    #[fuchsia::test]
    fn test_reproduction_code_write_barrier() {
        let code =
            ReproductionCode { op: CompletedOp::WriteBarrier, register: None, repeating: false };
        assert_eq!(code.to_string(), "t.write_barrier();");
    }

    // -----------------------------------------------------------------------
    // Reports
    // -----------------------------------------------------------------------

    #[fuchsia::test]
    fn test_mismatch_report() {
        let retired = [read_expectation(0x10, 0x1234, None)];
        let completed_ops = [history(read_op(0x10, AccessSize::U32, 0x1234), 0, None)];
        let pending = [
            read_expectation(0x20, 0x5678, None),
            expectation(ExpectationPattern::MustMatchOnce(CompletedOp::WriteBarrier), None),
        ];
        let view = ScoreboardStateView {
            retired_expectations: &retired,
            pending_expectations: &pending,
            completed_ops: &completed_ops,
        };
        let report = MismatchReport {
            view,
            expectation: &pending[0],
            actual: RequestedOp::Write {
                offset: 0x20,
                data: AccessData::new(AccessSize::U32, 0x9999),
            },
            backtrace: &Backtrace::disabled(),
        };

        assert_eq!(
            report.to_string(),
            format!(
                concat!(
                    "\n",
                    "════════════════════════ MMIO EXPECTATION MISMATCH ═════════════════════════\n",
                    "\n",
                    "FAILED AT EXPECTATION #1:\n",
                    "  --> {header}\n",
                    "\n",
                    "MISMATCH DETAILS:\n",
                    "  Expected: READ  32-bit at offset 0x0020 returning 0x00005678\n",
                    "  Actual:   WRITE 32-bit at offset 0x0020 with value 0x00009999\n",
                    "\n",
                    "RECENT TRACE HISTORY:\n",
                    "    [#0] [OK]  t.at(0x0010).read32(0x00001234);                   ({row})\n",
                    "--> [#1] [ERR] t.at(0x0020).read32(0x00005678);                   ({row})\n",
                    "    [#2] [..]  t.write_barrier();                                 ({row})\n",
                    "\n",
                    "DRIVER INVOCATION BACKTRACE:\n",
                    "{backtrace}\n",
                    "════════════════════════════════════════════════════════════════════════════\n",
                ),
                header = location_header(),
                row = location_row(),
                backtrace = Backtrace::disabled(),
            )
        );
    }

    #[fuchsia::test]
    fn test_mismatch_report_marks_the_repeating_expectation() {
        let register = Some(RegisterRef::new("registers::Status"));
        let pending = [expectation(
            ExpectationPattern::WhileMatches(read_op(0x10, AccessSize::U32, 0x1)),
            register,
        )];
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &pending,
            completed_ops: &[],
        };
        let report = MismatchReport {
            view,
            expectation: &pending[0],
            actual: RequestedOp::Read { offset: 0x10, size: AccessSize::U16 },
            backtrace: &Backtrace::disabled(),
        };

        let rendered = report.to_string();
        assert!(
            rendered.contains(
                "  Expected: READ  32-bit at registers::Status (0x0010) \
                 returning 0x00000001 (repeating)\n"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("[ERR] t.read::<registers::Status>(0x00000001); // repeating"),
            "{rendered}"
        );
    }

    #[fuchsia::test]
    fn test_mismatch_report_indexes_repeated_completions_by_expectation() {
        // A repeating expectation completes several operations. All of the
        // resulting history rows carry that expectation's index, so the indexes
        // in the report never move backwards.
        let register = Some(RegisterRef::new("registers::Status"));
        let retired = [
            read_expectation(0x0, 0x0, None),
            expectation(
                ExpectationPattern::WhileMatches(read_op(0x10, AccessSize::U32, 0x1)),
                register.clone(),
            ),
        ];
        let completed_ops = [
            history(read_op(0x0, AccessSize::U32, 0x0), 0, None),
            history(read_op(0x10, AccessSize::U32, 0x1), 1, register.clone()),
            history(read_op(0x10, AccessSize::U32, 0x1), 1, register),
        ];
        let pending = [read_expectation(0x20, 0x2, None)];
        let view = ScoreboardStateView {
            retired_expectations: &retired,
            pending_expectations: &pending,
            completed_ops: &completed_ops,
        };
        let report = MismatchReport {
            view,
            expectation: &pending[0],
            actual: RequestedOp::Read { offset: 0x30, size: AccessSize::U32 },
            backtrace: &Backtrace::disabled(),
        };

        let rendered = report.to_string();
        let indexes: Vec<&str> = rendered
            .lines()
            .filter(|line| line.contains("[OK]") || line.contains("[ERR]"))
            .map(|line| line.split_whitespace().find(|word| word.starts_with("[#")).unwrap())
            .collect();
        assert_eq!(indexes, ["[#0]", "[#1]", "[#1]", "[#2]"]);
    }

    #[fuchsia::test]
    fn test_unexpected_access_report() {
        let completed_ops = [history(write_op(0x10, AccessSize::U32, 0x1234), 0, None)];
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &[],
            completed_ops: &completed_ops,
        };
        let report = UnexpectedAccessReport {
            view,
            actual: RequestedOp::Read { offset: 0x20, size: AccessSize::U32 },
            backtrace: &Backtrace::disabled(),
        };

        assert_eq!(
            report.to_string(),
            format!(
                concat!(
                    "\n",
                    "══════════════════════════ UNEXPECTED MMIO ACCESS ══════════════════════════\n",
                    "\n",
                    "UNEXPECTED ACCESS (expectation queue is empty):\n",
                    "  Actual:   READ  32-bit at offset 0x0020\n",
                    "\n",
                    "RECENT TRACE HISTORY:\n",
                    "    [#0] [OK]  t.at(0x0010).write32(0x00001234);                  ({row})\n",
                    "\n",
                    "DRIVER INVOCATION BACKTRACE:\n",
                    "{backtrace}\n",
                    "════════════════════════════════════════════════════════════════════════════\n",
                ),
                row = location_row(),
                backtrace = Backtrace::disabled(),
            )
        );
    }

    #[fuchsia::test]
    fn test_unexpected_access_report_without_history() {
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &[],
            completed_ops: &[],
        };
        let report = UnexpectedAccessReport {
            view,
            actual: RequestedOp::Read { offset: 0x20, size: AccessSize::U32 },
            backtrace: &Backtrace::disabled(),
        };

        assert!(
            report
                .to_string()
                .contains("RECENT TRACE HISTORY:\n    (No accesses completed successfully yet)\n"),
            "{report}"
        );
    }

    #[fuchsia::test]
    fn test_unretired_expectations_report() {
        let pending = [read_expectation(0x10, 0x1234, None)];
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &pending,
            completed_ops: &[],
        };

        assert_eq!(
            UnretiredExpectationsReport { view }.to_string(),
            format!(
                concat!(
                    "\n",
                    "═══════════════════════ UNRETIRED MMIO EXPECTATIONS ════════════════════════\n",
                    "\n",
                    "1 expectation(s) were never matched at test completion.\n",
                    "\n",
                    "RECENT TRACE HISTORY:\n",
                    "    [#0] [..]  t.at(0x0010).read32(0x00001234);                   ({row})\n",
                    "════════════════════════════════════════════════════════════════════════════\n",
                ),
                row = location_row(),
            )
        );
    }

    #[fuchsia::test]
    fn test_unretired_expectations_report_excludes_satisfied_expectations() {
        // A trailing repeating expectation would auto-retire, so it does not
        // count towards the failure, but it is still listed for context.
        let pending = [
            read_expectation(0x10, 0x1234, None),
            expectation(
                ExpectationPattern::WhileMatches(read_op(0x20, AccessSize::U32, 0x1)),
                None,
            ),
        ];
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &pending,
            completed_ops: &[],
        };

        let rendered = UnretiredExpectationsReport { view }.to_string();
        assert!(
            rendered.contains("1 expectation(s) were never matched at test completion.\n"),
            "{rendered}"
        );
        assert!(rendered.contains("[#0] [..]  t.at(0x0010).read32(0x00001234);"), "{rendered}");
        assert!(
            rendered.contains("[#1] [opt] t.at(0x0020).read32(0x00000001); // repeating"),
            "{rendered}"
        );
    }

    // -----------------------------------------------------------------------
    // Trace window truncation
    // -----------------------------------------------------------------------

    #[fuchsia::test]
    fn test_trace_window_reports_omitted_history() {
        let completed_ops: Vec<HistoryEntry> = (0..MAX_HISTORY_ENTRIES + 2)
            .map(|index| history(read_op(0x10, AccessSize::U32, index as u64), index, None))
            .collect();
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &[],
            completed_ops: &completed_ops,
        };
        let report = UnexpectedAccessReport {
            view,
            actual: RequestedOp::WriteBarrier,
            backtrace: &Backtrace::disabled(),
        };

        let rendered = report.to_string();
        assert!(rendered.contains("    ... 2 earlier access(es) omitted\n"), "{rendered}");
        // The oldest two rows are dropped; the newest ten are kept.
        assert!(!rendered.contains("0x00000001);"), "{rendered}");
        assert!(rendered.contains("0x00000002);"), "{rendered}");
        assert!(rendered.contains("0x0000000b);"), "{rendered}");
        assert_eq!(rendered.matches("[OK]").count(), MAX_HISTORY_ENTRIES);
    }

    #[fuchsia::test]
    fn test_trace_window_reports_omitted_expectations() {
        let pending: Vec<Expectation> = (0..MAX_UPCOMING_ENTRIES + 3)
            .map(|index| read_expectation(0x10, index as u64, None))
            .collect();
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &pending,
            completed_ops: &[],
        };

        let rendered = UnretiredExpectationsReport { view }.to_string();
        assert!(rendered.contains("    ... and 3 more expectation(s)\n"), "{rendered}");
        assert_eq!(rendered.matches("[..]").count(), MAX_UPCOMING_ENTRIES);
    }

    #[fuchsia::test]
    fn test_trace_window_keeps_the_location_column_aligned() {
        // Production register names come from `std::any::type_name`, which
        // produces fully qualified paths. They are rendered in full, so the
        // code column must grow to fit them.
        let register =
            Some(RegisterRef::new("my_driver::registers::Backend::DsiController::Version"));
        let pending = [read_expectation(0x10, 0x1234, register), read_expectation(0x20, 0x5, None)];
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &pending,
            completed_ops: &[],
        };

        let rendered = UnretiredExpectationsReport { view }.to_string();
        let columns: Vec<usize> = rendered
            .lines()
            .filter(|line| line.contains("[..]"))
            .map(|line| line.find(" (").expect("every row ends with a location"))
            .collect();
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0], columns[1], "rows are misaligned:\n{rendered}");
    }

    // -----------------------------------------------------------------------
    // Layout
    // -----------------------------------------------------------------------

    #[fuchsia::test]
    fn test_all_report_rules_have_the_same_width() {
        let view = ScoreboardStateView {
            retired_expectations: &[],
            pending_expectations: &[],
            completed_ops: &[],
        };
        let backtrace = Backtrace::disabled();
        let expectation = read_expectation(0x10, 0x1234, None);
        let pending = [expectation];
        let mismatch_view = ScoreboardStateView { pending_expectations: &pending, ..view.clone() };

        let reports = [
            MismatchReport {
                view: mismatch_view,
                expectation: &pending[0],
                actual: RequestedOp::WriteBarrier,
                backtrace: &backtrace,
            }
            .to_string(),
            UnexpectedAccessReport {
                view: view.clone(),
                actual: RequestedOp::WriteBarrier,
                backtrace: &backtrace,
            }
            .to_string(),
            UnretiredExpectationsReport { view }.to_string(),
        ];

        for report in reports {
            for rule in report.lines().filter(|line| line.starts_with('═') || line.contains(" ═"))
            {
                assert_eq!(
                    rule.chars().count(),
                    REPORT_WIDTH,
                    "rule is not {REPORT_WIDTH} columns wide: {rule:?}"
                );
            }
        }
    }
}
