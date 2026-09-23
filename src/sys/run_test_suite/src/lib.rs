// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(feature = "fdomain")]
extern crate component_debug_fdomain as component_debug;
#[cfg(feature = "fdomain")]
extern crate fuchsia_fs_fdomain as fuchsia_fs;

mod artifacts;
mod cancel;
mod connector;
pub mod diagnostics;
mod outcome;
pub mod output;
mod params;
mod realm;
mod run;
mod running_suite;
mod stream_util;
mod trace;

pub use artifacts::copy_debug_data;
pub use connector::{SingleRunConnector, SuiteRunnerConnector};
pub use outcome::{ConnectionError, Outcome, RunTestSuiteError, UnexpectedEventError};
pub use params::{RunParams, TestParams, TimeoutBehavior};
pub use realm::parse_provided_realm;
pub use run::{DirectoryReporterOptions, create_reporter, run_test_and_get_outcome};
