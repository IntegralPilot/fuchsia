// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use flex_fuchsia_test_manager as ftest_manager;
use std::borrow::Borrow;
use std::io::{Error, Write};
use std::marker::PhantomData;
use std::path::Path;
use test_list::TestTag;

mod directory;
mod directory_with_stdout;
mod line;
mod memory;
mod mux;
mod noop;
mod shell;

pub use directory::{DirectoryReporter, SchemaVersion};
pub use directory_with_stdout::DirectoryWithStdoutReporter;
use line::AnsiFilterReporter;
pub use memory::{InMemoryArtifact, InMemoryDirectoryWriter, InMemoryReporter};
pub use mux::MultiplexedReporter;
pub use noop::NoopReporter;
pub use shell::{ShellReporter, ShellWriterView};

pub(crate) type DynArtifact = dyn 'static + Write + Send + Sync;
pub(crate) type DynDirectoryArtifact = dyn 'static + DirectoryWrite + Send + Sync;
pub(crate) type DynReporter = dyn 'static + Reporter + Send + Sync;

pub struct EntityReporter<E, T: Borrow<DynReporter>> {
    reporter: T,
    entity: EntityId,
    _entity_type: std::marker::PhantomData<E>,
}

/// A reporter for structured results scoped to a test run.
/// To report results and artifacts for a test run, a user should create a `RunReporter`,
/// and create child `SuiteReporter`s and `CaseReporter`s beneath it to report results and
/// artifacts scoped to suites and cases. When the run is finished, the user should call
/// `record` to ensure the results are persisted.
///
/// `RunReporter`, `SuiteReporter`, and `CaseReporter` are wrappers around `Reporter`
/// implementations that statically ensure that some assumptions made by `Reporter` implemtnations
/// are upheld (for example, that an entity marked finished at most once).
pub type RunReporter = EntityReporter<(), Box<DynReporter>>;
/// A reporter for structured results scoped to a test suite. Note this may not outlive
/// the `RunReporter` from which it is created.
pub type SuiteReporter<'a> = EntityReporter<(), &'a DynReporter>;

/// A reporter for structured results scoped to a single test case. Note this may not outlive
/// the `SuiteReporter` from which it is created.
pub type CaseReporter<'a> = EntityReporter<CaseId, &'a DynReporter>;

impl<E, T: Borrow<DynReporter>> EntityReporter<E, T> {
    /// Create a new artifact scoped to the suite.
    pub fn new_artifact(&self, artifact_type: &ArtifactType) -> Result<Box<DynArtifact>, Error> {
        self.reporter.borrow().new_artifact(&self.entity, artifact_type)
    }

    /// Create a new directory artifact scoped to the suite.
    pub fn new_directory_artifact(
        &self,
        artifact_type: &DirectoryArtifactType,
        component_moniker: Option<String>,
    ) -> Result<Box<DynDirectoryArtifact>, Error> {
        self.reporter.borrow().new_directory_artifact(
            &self.entity,
            artifact_type,
            component_moniker,
        )
    }

    /// Record that the suite has started.
    pub fn started(&self, timestamp: Timestamp) -> Result<(), Error> {
        self.reporter.borrow().entity_started(&self.entity, timestamp)
    }

    /// Record the outcome of the suite.
    pub fn stopped(&self, outcome: &ReportedOutcome, timestamp: Timestamp) -> Result<(), Error> {
        self.reporter.borrow().entity_stopped(&self.entity, outcome, timestamp)
    }
    /// Finalize and persist the test run.
    pub fn finished(self) -> Result<(), Error> {
        self.reporter.borrow().entity_finished(&self.entity)
    }
}

impl RunReporter {
    /// Create a `RunReporter`, where stdout, stderr, syslog, and restricted log artifact types
    /// are sanitized for ANSI escape sequences before being passed along to |reporter|.
    pub fn new_ansi_filtered<R: 'static + Reporter + Send + Sync>(reporter: R) -> Self {
        Self::new(AnsiFilterReporter::new(reporter))
    }

    /// Create a `RunReporter`.
    pub fn new<R: 'static + Reporter + Send + Sync>(reporter: R) -> Self {
        let reporter = Box::new(reporter);
        Self { reporter, entity: EntityId::TestRun, _entity_type: PhantomData }
    }

    /// Set the number of expected suites for the run.
    pub fn set_expected_suites(&self, expected_suites: u32) {
        self.reporter.set_entity_info(
            &self.entity,
            &EntityInfo { expected_children: Some(expected_suites), ..EntityInfo::default() },
        )
    }

    /// Record a new suite under the test run.
    pub fn new_suite(&self, url: &str) -> Result<SuiteReporter<'_>, Error> {
        let entity = EntityId::Suite;
        self.reporter.new_entity(&entity, url)?;
        Ok(SuiteReporter { reporter: self.reporter.borrow(), entity, _entity_type: PhantomData })
    }
}

impl<'a> SuiteReporter<'a> {
    /// Record a new suite under the suite.
    pub fn new_case(&self, name: &str, case_id: &CaseId) -> Result<CaseReporter<'_>, Error> {
        let entity = EntityId::Case { case: *case_id };
        self.reporter.new_entity(&entity, name)?;
        Ok(CaseReporter { reporter: self.reporter, entity, _entity_type: PhantomData })
    }

    /// Set the tags for this suite.
    pub fn set_tags(&self, tags: Vec<TestTag>) {
        self.reporter.set_entity_info(
            &self.entity,
            &EntityInfo { tags: Some(tags), ..EntityInfo::default() },
        );
    }
}

/// An enumeration of different known artifact types.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum ArtifactType {
    Stdout,
    Stderr,
    Syslog,
    RestrictedLog,
}

/// An enumeration of different known artifact types consisting of multiple files.
#[derive(Clone, Copy, Debug)]
pub enum DirectoryArtifactType {
    /// An arbitrary set of custom files stored by the test in a directory.
    Custom,
    /// A collection of debug files. For example: coverage and profiling.
    Debug,
}

/// Common outcome type for test results, suites, and test cases.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ReportedOutcome {
    Passed,
    Failed,
    Inconclusive,
    Timedout,
    Error,
    Skipped,
    Cancelled,
    DidNotFinish,
}

impl std::fmt::Display for ReportedOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let repr = match self {
            Self::Passed => "PASSED",
            Self::Failed => "FAILED",
            Self::Inconclusive => "INCONCLUSIVE",
            Self::Timedout => "TIMED_OUT",
            Self::Error => "ERROR",
            Self::Skipped => "SKIPPED",
            Self::Cancelled => "CANCELLED",
            Self::DidNotFinish => "DID_NOT_FINISH",
        };
        write!(f, "{}", repr)
    }
}

impl From<ftest_manager::TestCaseResult> for ReportedOutcome {
    fn from(status: ftest_manager::TestCaseResult) -> Self {
        match status {
            ftest_manager::TestCaseResult::Passed => Self::Passed,
            ftest_manager::TestCaseResult::Failed => Self::Failed,
            ftest_manager::TestCaseResult::TimedOut => Self::Timedout,
            ftest_manager::TestCaseResult::Skipped => Self::Skipped,
            // Test case 'Error' indicates the test failed to report a result, not internal error.
            ftest_manager::TestCaseResult::Error => Self::DidNotFinish,
            ftest_manager::TestCaseResultUnknown!() => {
                panic!("unrecognized case status");
            }
        }
    }
}

impl From<crate::Outcome> for ReportedOutcome {
    fn from(outcome: crate::Outcome) -> Self {
        match outcome {
            crate::Outcome::Passed => Self::Passed,
            crate::Outcome::Failed => Self::Failed,
            crate::Outcome::Inconclusive => Self::Inconclusive,
            crate::Outcome::Cancelled => Self::Cancelled,
            crate::Outcome::DidNotFinish => Self::DidNotFinish,
            crate::Outcome::Timedout => Self::Timedout,
            crate::Outcome::Error { .. } => Self::Error,
        }
    }
}

impl Into<test_output_directory::ArtifactType> for ArtifactType {
    fn into(self) -> test_output_directory::ArtifactType {
        match self {
            Self::Stdout => test_output_directory::ArtifactType::Stdout,
            Self::Stderr => test_output_directory::ArtifactType::Stderr,
            Self::Syslog => test_output_directory::ArtifactType::Syslog,
            Self::RestrictedLog => test_output_directory::ArtifactType::RestrictedLog,
        }
    }
}

impl Into<test_output_directory::ArtifactType> for DirectoryArtifactType {
    fn into(self) -> test_output_directory::ArtifactType {
        match self {
            Self::Custom => test_output_directory::ArtifactType::Custom,
            Self::Debug => test_output_directory::ArtifactType::Debug,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CaseId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EntityId {
    TestRun,
    Suite,
    Case { case: CaseId },
}

#[derive(Default)]
pub struct EntityInfo {
    pub expected_children: Option<u32>,
    pub tags: Option<Vec<TestTag>>,
}

/// A trait for structs that report test results.
/// Implementations of `Reporter` serve as the backend powering `RunReporter`.
///
/// As with `std::io::Write`, `Reporter` implementations are intended to be composable.
/// An implementation of `Reporter` may contain other `Reporter` implementors and delegate
/// calls to them.
pub trait Reporter: Send + Sync {
    /// Record a new test suite or test case. This should be called once per entity.
    fn new_entity(&self, entity: &EntityId, name: &str) -> Result<(), Error>;

    /// Add additional info for an entity.
    fn set_entity_info(&self, entity: &EntityId, info: &EntityInfo);

    /// Record that a test run, suite or case has started.
    fn entity_started(&self, entity: &EntityId, timestamp: Timestamp) -> Result<(), Error>;

    /// Record that a test run, suite, or case has stopped.
    fn entity_stopped(
        &self,
        entity: &EntityId,
        outcome: &ReportedOutcome,
        timestamp: Timestamp,
    ) -> Result<(), Error>;

    /// Record that a test run, suite, or case has stopped. After this method is called for
    /// an entity, no additional events or artifacts may be added to the entity.
    /// Implementations of `Reporter` may assume that `entity_finished` will be called no more
    /// than once for any entity.
    fn entity_finished(&self, entity: &EntityId) -> Result<(), Error>;

    /// Create a new artifact scoped to the referenced entity.
    fn new_artifact(
        &self,
        entity: &EntityId,
        artifact_type: &ArtifactType,
    ) -> Result<Box<DynArtifact>, Error>;

    /// Create a new artifact consisting of multiple files.
    fn new_directory_artifact(
        &self,
        entity: &EntityId,
        artifact_type: &DirectoryArtifactType,
        component_moniker: Option<String>,
    ) -> Result<Box<DynDirectoryArtifact>, Error>;
}

/// A trait for writing artifacts that consist of multiple files organized in a directory.
pub trait DirectoryWrite {
    /// Create a new file within the directory. `path` must be a relative path with no parent
    /// segments.
    fn new_file(&self, path: &Path) -> Result<Box<DynArtifact>, Error>;
}

/// A wrapper around Fuchsia's representation of time.
/// This is added as fuchsia-zircon is not available on host.
#[derive(Clone, Copy)]
pub struct ZxTime(i64);

impl ZxTime {
    pub const fn from_nanos(nanos: i64) -> Self {
        ZxTime(nanos)
    }

    pub fn checked_sub(&self, rhs: Self) -> Option<std::time::Duration> {
        let nanos = self.0 - rhs.0;
        if nanos < 0 { None } else { Some(std::time::Duration::from_nanos(nanos as u64)) }
    }
}

#[derive(Clone, Copy)]
pub enum Timestamp {
    Unknown,
    Given(ZxTime),
}

impl Timestamp {
    pub fn from_nanos(nanos: Option<i64>) -> Self {
        match nanos {
            None => Self::Unknown,
            Some(n) => Self::Given(ZxTime::from_nanos(n)),
        }
    }
}
