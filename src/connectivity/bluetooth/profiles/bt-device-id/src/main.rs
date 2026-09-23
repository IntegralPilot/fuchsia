// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_bluetooth_bredr::ProfileMarker;
use fuchsia_component::server::ServiceFs;
use futures::channel::mpsc;
use futures::future;
use log::{error, info, warn};
use std::pin::pin;

use crate::device_id::{DIRecord, DeviceIdServer};
use crate::fidl_service::run_services;

mod device_id;
mod error;
mod fidl_service;

/// The maximum number of simultaneous DI advertisements that this implementation supports.
pub const DEFAULT_MAX_DEVICE_ID_ADVERTISEMENTS: usize = 10;

#[fuchsia::main(logging_tags = ["bt-device-id"])]
async fn main() -> Result<(), Error> {
    let profile = fuchsia_component::client::connect_to_protocol::<ProfileMarker>()?;
    let default_config =
        DIRecord::try_from(bt_device_id_profile_config::Config::take_from_startup_handle());
    if default_config.is_err() {
        info!("Default DI configuration is invalid. Ignoring.");
    }
    let (device_id_request_sender, device_id_request_receiver) = mpsc::channel(1);
    let device_id_server = DeviceIdServer::new(
        DEFAULT_MAX_DEVICE_ID_ADVERTISEMENTS,
        default_config.ok(),
        profile,
        device_id_request_receiver,
    )
    .run();
    let device_id_server = pin!(device_id_server);

    let fs = ServiceFs::new();
    let services = pin!(run_services(fs, device_id_request_sender));

    info!("Device ID component running");

    match future::select(services, device_id_server).await {
        future::Either::Left((Ok(()), _)) => {
            warn!("Service FS directory handle closed. Exiting.");
        }
        future::Either::Left((Err(e), _)) => {
            error!("Error encountered running Service FS: {}. Exiting", e);
        }
        future::Either::Right((Ok(()), _)) => {
            warn!("All DeviceId related connections to this component have been disconnected. Exiting.");
        }
        future::Either::Right((Err(e), _)) => {
            error!("Error encountered running main DeviceId loop: {}. Exiting.", e);
        }
    }
    Ok(())
}
