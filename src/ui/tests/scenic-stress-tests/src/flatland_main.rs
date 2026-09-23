// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod flatland_actor;
mod flatland_environment;
mod flatland_instance;
mod input_actor;
mod input_listener;
mod metrics_discarder;
mod pointer_state;

use argh::FromArgs;
use diagnostics_log::Severity;
use flatland_environment::FlatlandEnvironment;
use stress_test::run_test;

#[derive(Clone, Debug, FromArgs)]
/// Creates an instance of scenic and performs stressful operations on it
pub struct Args {
    /// number of operations to complete before exiting.
    #[argh(option, short = 'o')]
    num_operations: Option<u64>,

    /// filter logging by level (off, error, warn, info, debug, trace)
    #[argh(option, short = 'l')]
    log_filter: Option<Severity>,

    /// if set, the test runs for this time limit before exiting successfully.
    #[argh(option, short = 't')]
    time_limit_secs: Option<u64>,

    /// flag passed in by rust test runner
    #[argh(switch)]
    #[allow(unused)]
    nocapture: bool,
}

#[fuchsia::test(logging = false)]
async fn test() {
    // Get arguments from command line
    let args: Args = argh::from_env();

    let mut options = diagnostics_log::PublishOptions::default();
    if let Some(filter) = args.log_filter {
        options = options.minimum_severity(filter);
    }
    diagnostics_log::initialize(options).unwrap();

    // Setup the scenic environment
    let env = FlatlandEnvironment::new(args).await;

    // Run the test
    run_test(env).await;
}
