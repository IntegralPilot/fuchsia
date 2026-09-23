// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_test_manager as ftest_manager;
use test_manager_test_lib::RunEvent;

pub async fn run_test(test_url: &str) -> Result<(Vec<RunEvent>, Vec<String>), anyhow::Error> {
    let suite_runner =
        fuchsia_component::client::connect_to_protocol::<ftest_manager::SuiteRunnerMarker>()
            .expect("connect to test manager protocol");
    let runner = test_manager_test_lib::SuiteRunner::new(suite_runner);

    let suite_instance =
        runner.start_suite_run(test_url, ftest_manager::RunSuiteOptions::default())?;

    test_runners_test_lib::process_events(suite_instance, true).await
}
