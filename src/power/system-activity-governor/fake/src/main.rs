// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod system_activity_governor_control;

use crate::system_activity_governor_control::SystemActivityGovernorControl;
use anyhow::{Context, Result};
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;

#[fuchsia::main]
async fn main() -> Result<()> {
    log::info!("started");

    let sagctrl = SystemActivityGovernorControl::new().await;

    let mut service_fs = ServiceFs::new_local();

    sagctrl.run(&mut service_fs).await;

    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    // This future should never complete.
    service_fs.collect::<()>().await;

    log::error!("Unexpected exit");
    Ok(())
}
