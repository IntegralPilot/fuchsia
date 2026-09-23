// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_component::server::ServiceObj;
use log::{debug, error};
use profile_client::ProfileClient;

use crate::peers::Peers;

mod descriptor;
mod peer_info;
mod peer_task;
mod peers;
mod protocol;
mod sdp_data;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    debug!("Started HID component.");

    let (profile_proxy, profile_client) = register_profile()?;

    let inspector = fuchsia_inspect::Inspector::default();
    let _inspect_server_task =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());

    let mut peers = Peers::new(profile_client, profile_proxy);
    peers.run().await;

    debug!("Exiting.");
    Ok(())
}

fn register_profile() -> anyhow::Result<(bredr::ProfileProxy, ProfileClient)> {
    debug!("Registering profile.");

    let proxy = fuchsia_component::client::connect_to_protocol::<bredr::ProfileMarker>()
        .context("Failed to connect to Bluetooth Profile service")?;

    // Create a profile that does no advertising.
    let mut client = ProfileClient::new(proxy.clone());

    // Register a search for remote peers that support BR/EDR HID.
    client
        .add_search(bredr::ServiceClassProfileIdentifier::HumanInterfaceDevice, &[])
        .context("Failed to search for peers supporting HID.")?;

    Ok((proxy, client))
}
