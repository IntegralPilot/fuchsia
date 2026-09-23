// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod system_activity_governor_controller;

use crate::system_activity_governor_controller::SystemActivityGovernorController;
use anyhow::Result;
use fuchsia_inspect::health::Reporter;
use log::info;

#[fuchsia::main(logging_tags = ["system-activity-governor-controller"])]
async fn main() -> Result<()> {
    info!("started");

    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default());
    fuchsia_inspect::component::health().set_starting_up();

    let controller = SystemActivityGovernorController::new();
    fuchsia_inspect::component::health().set_ok();

    // This future should never complete.
    let result = controller.run().await;
    log::error!(result:?; "Unexpected exit");
    fuchsia_inspect::component::health().set_unhealthy(&format!("Unexpected exit: {:?}", result));
    result
}
