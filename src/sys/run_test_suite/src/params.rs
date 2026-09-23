// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_data::Severity;
use flex_fuchsia_diagnostics::LogInterestSelector;
use flex_fuchsia_test_manager as ftest_manager;
use std::sync::Arc;
use test_list::TestTag;

/// Parameters that specify how a single test suite should be executed.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TestParams {
    /// Test URL.
    pub test_url: String,

    /// Provided test realm.
    pub realm: Arc<Option<crate::realm::Realm>>,

    /// Test timeout. Must be more than zero.
    pub timeout_seconds: Option<std::num::NonZeroU32>,

    /// Filter tests based on glob pattern(s).
    pub test_filters: Option<Vec<String>>,

    /// Whether an empty test case set means success or failure.
    pub no_cases_equals_success: Option<bool>,

    /// Run disabled tests.
    pub also_run_disabled_tests: bool,

    /// Test concurrency count.
    pub parallel: Option<u16>,

    /// Arguments to pass to test using command line.
    pub test_args: Vec<String>,

    /// Maximum allowable log severity for the test.
    pub max_severity_logs: Option<Severity>,

    /// Minimum requested log severity to print for the test.
    pub min_severity_logs: Vec<LogInterestSelector>,

    /// List of tags to associate with this test's output.
    pub tags: Vec<TestTag>,

    /// Stop the test suite on the first test failure so a debugger can attach.
    pub break_on_failure: bool,

    /// Don't create exception channels, which may conflict with the exception channels created
    /// by the test.
    pub no_exception_channel: bool,
}

/// Parameters that specify how the overall test run should be executed.
#[derive(Debug, PartialEq)]
pub struct RunParams {
    /// The behavior of the test run if a suite times out.
    pub timeout_behavior: TimeoutBehavior,

    /// Time in seconds to wait for events to drain after timeout.
    pub timeout_grace_seconds: u32,

    /// If set, stop executing tests after this number of normal test failures occur.
    pub stop_after_failures: Option<std::num::NonZeroU32>,

    /// Whether or not to merge debug data from previous runs with new debug data collected
    /// for this test run.
    pub accumulate_debug_data: bool,

    /// If set, set the protocol used to retrieve logs. If not set, an appropriate default
    /// will be chosen by the implementation.
    pub log_protocol: Option<ftest_manager::LogsIteratorType>,

    /// If set, specifies the minimum log severity to report. As it is an
    /// option for output, it will likely soon be moved to a reporter.
    pub min_severity_logs: Vec<LogInterestSelector>,

    /// If true, shows the full moniker in logs.
    pub show_full_moniker: bool,
}

/// Sets the behavior of the overall run if a suite terminates with a timeout.
#[derive(Debug, PartialEq)]
pub enum TimeoutBehavior {
    /// Immediately terminate any suites that haven't started.
    TerminateRemaining,
    /// Continue executing any suites that haven't finished.
    Continue,
}
