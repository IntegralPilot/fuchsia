// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod topology_test_daemon;

use crate::topology_test_daemon::TopologyTestDaemon;
use anyhow::Result;
use fuchsia_inspect::health::Reporter;
use log::info;

#[fuchsia::main(logging_tags = ["topology-test-daemon"])]
async fn main() -> Result<()> {
    info!("started");

    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default());
    fuchsia_inspect::component::health().set_starting_up();

    let ttd = TopologyTestDaemon::new().await?;
    fuchsia_inspect::component::health().set_ok();

    // This future should never complete.
    let result = ttd.run().await;
    log::error!(result:?; "Unexpected exit");
    fuchsia_inspect::component::health().set_unhealthy(&format!("Unexpected exit: {:?}", result));
    result
}
