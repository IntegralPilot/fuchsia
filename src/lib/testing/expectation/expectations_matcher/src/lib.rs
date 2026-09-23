// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides test expectations matching based on json5 expectations files.

/// Calculates the expected outcome for a test named `name` for `expectation`
/// given the current set of `cases_to_run`.
fn expected_single_outcome(
    name: &str,
    expectation: &ser::Expectation,
    cases_to_run: &ser::CasesToRun,
) -> Option<Outcome> {
    let (ser::Matchers { matchers }, outcome) = match expectation {
        ser::Expectation::Skip(a) => (a, Outcome::Skip),
        ser::Expectation::ExpectFailure(a) => match cases_to_run {
            ser::CasesToRun::WithErrLogs => (a, Outcome::Skip),
            _ => (a, Outcome::Fail),
        },
        ser::Expectation::ExpectPass(a) => match cases_to_run {
            ser::CasesToRun::WithErrLogs => (a, Outcome::Skip),
            _ => (a, Outcome::Pass),
        },
        ser::Expectation::ExpectFailureWithErrLogs(a) => match cases_to_run {
            ser::CasesToRun::NoErrLogs => (a, Outcome::Skip),
            _ => (a, Outcome::Fail),
        },
        ser::Expectation::ExpectPassWithErrLogs(a) => match cases_to_run {
            ser::CasesToRun::NoErrLogs => (a, Outcome::Skip),
            _ => (a, Outcome::Pass),
        },
    };
    matchers.iter().any(|matcher| matcher.matches(name)).then_some(outcome)
}

/// Calculates the expected outcome for a test named `name` for the given set of
/// `expectations`.
pub fn expected_outcome(name: &str, expectations: &ser::Expectations) -> Option<Outcome> {
    let ser::Expectations { expectations, cases_to_run } = expectations;
    expectations
        .iter()
        .rev()
        .find_map(|expectation| expected_single_outcome(name, expectation, cases_to_run))
}

/// The outcome of an expectation matching operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    Skip,
}
