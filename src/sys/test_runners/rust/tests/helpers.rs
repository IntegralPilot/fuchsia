// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fidl_fuchsia_test_manager::RunSuiteOptions;
use test_manager_test_lib::RunEvent;

pub async fn run_test(
    test_url: &str,
    run_disabled_tests: bool,
    parallel: Option<u16>,
    arguments: Vec<String>,
) -> Result<(Vec<RunEvent>, Vec<String>), Error> {
    let suite_runner = test_runners_test_lib::connect_to_suite_runner().await?;
    let runner = test_manager_test_lib::SuiteRunner::new(suite_runner);
    let suite_instance = runner
        .start_suite_run(
            test_url,
            RunSuiteOptions {
                run_disabled_tests: Some(run_disabled_tests),
                max_concurrent_test_case_runs: parallel,
                arguments: Some(arguments),
                ..Default::default()
            },
        )
        .context("suite runner execution failed")?;
    let ret = test_runners_test_lib::process_events(suite_instance, true).await?;
    Ok(ret)
}
