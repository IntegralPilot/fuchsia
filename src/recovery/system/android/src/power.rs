// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_hardware_power_statecontrol as fpower_statecontrol;
use fuchsia_component::client::connect_to_protocol;

pub async fn shutdown(action: fpower_statecontrol::ShutdownAction) -> Result<(), Error> {
    let proxy = connect_to_protocol::<fpower_statecontrol::AdminMarker>()?;
    log::info!("Shutting down system with action {:?}", action);
    proxy
        .shutdown(&fpower_statecontrol::ShutdownOptions {
            action: Some(action),
            reasons: Some(vec![fpower_statecontrol::ShutdownReason::DeveloperRequest]),
            ..Default::default()
        })
        .await?
        .map_err(zx::Status::err_from_raw)?;
    Ok(())
}
