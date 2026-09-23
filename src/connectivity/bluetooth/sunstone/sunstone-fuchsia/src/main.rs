// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! sunstone-fuchsia contains the main() function for the bt-host component. It is responsible for
//! connecting to the bt-gap component and vendor drivers and initializing and running the
//! Bluetooth Host.

mod host_server;
mod vendor;

use core::pin::pin;
use fidl_fuchsia_bluetooth_host as fidl_host;
use fuchsia_async as _;
use futures_util::future::FutureExt as _;
use futures_util::select_biased;
use tracing::{info, warn};

use crate::host_server::HostServer;
use crate::vendor::Vendor;

#[fuchsia::main]
async fn main() {
    let config = bt_host_config::Config::take_from_startup_handle();
    info!("Starting Rust bt-host (device_path={})", config.device_path);

    let vendor = match Vendor::connect(&config.device_path).await {
        Ok(vendor) => vendor,
        Err(e) => {
            warn!("Failed to connect to Vendor protocol at {}: {e:?}", config.device_path);
            return;
        }
    };

    // Connect to fuchsia.bluetooth.host.Receiver
    let receiver =
        match fuchsia_component::client::connect_to_protocol::<fidl_host::ReceiverMarker>() {
            Ok(proxy) => proxy,
            Err(e) => {
                warn!("Failed to connect to Receiver protocol: {e:?}");
                return;
            }
        };

    let (host_client, host_server) = fidl::endpoints::create_endpoints::<fidl_host::HostMarker>();

    if let Err(e) = receiver.add_host(host_client) {
        warn!("Failed to call add_host on Receiver: {e:?}");
        return;
    }

    let mut host_server = HostServer::new(host_server.into_stream());

    let dev_closed = async {
        vendor.on_closed().await;
    }
    .fuse();
    let mut dev_closed = pin!(dev_closed);

    let host_server_fut = host_server.run().fuse();
    let mut host_server_fut = pin!(host_server_fut);

    select_biased! {
        _ = &mut dev_closed => {
            info!("HCI device node closed; shutting down bt-host");
        }
        res = &mut host_server_fut => {
            match res {
                Ok(()) => info!("Host server finished"),
                Err(e) => warn!("Host request stream error: {e:?}"),
            }
        }
    }
}
